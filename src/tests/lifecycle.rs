use super::*;
use crate::{Config, auth::Identity, http::Http, runner};
use axum::{
    Json,
    body::Bytes,
    extract::{
        State,
        ws::{Message, WebSocketUpgrade},
    },
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::get,
};
use futures_util::StreamExt;
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Notify, oneshot};

#[derive(Clone, Copy, PartialEq)]
enum Mode {
    Success,
    BrokenBlob,
    Cancel,
    BadStep,
    BadSession,
    Renewal,
    LostLease,
}

type Events = Arc<Mutex<Vec<(String, Option<String>, Value)>>>;

#[derive(Clone)]
struct Fake {
    mode: Mode,
    events: Events,
    counters: Arc<Mutex<(usize, usize)>>,
    renewals: Arc<std::sync::atomic::AtomicUsize>,
    renewed: Arc<Notify>,
    shutdown: Arc<Mutex<Option<oneshot::Sender<std::io::Result<()>>>>>,
    feed_received: Arc<Notify>,
    marker: String,
}

impl Fake {
    fn record(&self, name: String, headers: &HeaderMap, body: Value) {
        self.events.lock().expect("events").push((
            name,
            headers
                .get("authorization")
                .map(|h| h.to_str().expect("header").to_owned()),
            body,
        ));
    }
}

async fn feed(State(fake): State<Fake>, headers: HeaderMap, ws: WebSocketUpgrade) -> Response {
    fake.record("feed-connect".into(), &headers, Value::Null);
    ws.on_upgrade(move |mut socket| async move {
        while let Some(Ok(message)) = socket.next().await {
            if let Message::Text(text) = message {
                fake.record(
                    "feed-lines".into(),
                    &HeaderMap::new(),
                    serde_json::from_str(&text).expect("feed JSON"),
                );
                fake.feed_received.notify_one();
            }
        }
    })
}

async fn api(
    State(fake): State<Fake>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    let path = uri.path();
    let body = serde_json::from_slice::<Value>(&bytes)
        .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()));
    fake.record(format!("{method} {path}"), &headers, body.clone());
    let base = format!("http://{}/", headers["host"].to_str().expect("host"));
    let response = if path.ends_with("registration-token") {
        json!({"token":"registration-secret"})
    } else if path == "/actions/runner-registration" {
        json!({"use_v2_flow":true,"token":"unused-tenant-secret","url":format!("{base}tenant/"),"token_schema":"OAuthAccessToken"})
    } else if path.ends_with("runner-groups") {
        json!({"runner_groups":[{"id":1,"default":true}]})
    } else if path == "/actions/runners/register" {
        json!({"id":42,"authorization":{"client_id":"test-client","authorization_url":format!("{base}oauth"),"server_url":format!("{base}broker/")}})
    } else if path == "/oauth" {
        json!({"access_token":"runner-secret","expires_in":3600})
    } else if path == "/broker/session" && method == Method::POST {
        if fake.mode == Mode::BadSession {
            return StatusCode::FORBIDDEN.into_response();
        }
        json!({"sessionId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"})
    } else if path == "/broker/message" {
        let busy = uri.query().unwrap_or("").contains("status=Busy");
        if busy && fake.mode == Mode::Cancel {
            fake.feed_received.notified().await;
            json!({"messageType":"JobCancellation","body":json!({"jobId":"33333333-3333-4333-8333-333333333333"}).to_string()})
        } else if !busy {
            let next = {
                let mut counters = fake.counters.lock().expect("counters");
                if counters.0 < if fake.mode == Mode::Success { 2 } else { 1 } {
                    counters.0 += 1;
                    Some(counters.0)
                } else {
                    None
                }
            };
            if let Some(next) = next {
                json!({"messageType":"RunnerJobRequest","body":json!({"runner_request_id":format!("request-{next}"),
                    "should_acknowledge":true,"run_service_url":format!("{base}acquisition/"),"billing_owner_id":"billing"}).to_string()})
            } else {
                return std::future::pending().await;
            }
        } else {
            return std::future::pending().await;
        }
    } else if path == "/acquisition/acquirejob" {
        let reference = if fake.mode == Mode::BadStep { 99 } else { 3 };
        let job_id = if body["jobMessageId"] == "request-2" {
            "55555555-5555-4555-8555-555555555555"
        } else {
            "33333333-3333-4333-8333-333333333333"
        };
        json!({"messageType":"RunnerJobRequest","jobId":job_id,
            "jobName":"demo","jobDisplayName":"Demo","plan":{"planId":"44444444-4444-4444-8444-444444444444"},
            "billingOwnerId":"billing","variables":{"system.github.results_endpoint":{"value":format!("{base}results/")},
                "sensitive":{"value":"job-secret-value","isSecret":true}},
            "resources":{"endpoints":[{"name":"SystemVssConnection","url":format!("{base}run/"),
                "authorization":{"parameters":{"AccessToken":"job-token"}},"data":{"FeedStreamUrl":format!("{base}feed")}}]},
            "steps":[{"type":4,"id":"11111111-1111-4111-8111-111111111111","displayName":"Declared step",
                "reference":{"type":reference},"condition":"success() && false",
                "inputs":{"type":2,"map":[{"Key":"script","Value":format!("touch '{}'\necho job-secret-value\nexit 97",fake.marker)}]}}]})
    } else if path == "/run/renewjob" {
        let previous = fake
            .renewals
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if previous > 0 {
            if fake.mode == Mode::LostLease {
                return StatusCode::NOT_FOUND.into_response();
            }
            fake.renewed.notify_one();
        }
        json!({"lockedUntil":"2099-01-01T00:00:00Z"})
    } else if path.ends_with("GetStepLogsSignedBlobURL")
        || path.ends_with("GetJobLogsSignedBlobURL")
    {
        if fake.mode == Mode::Cancel && path.ends_with("GetStepLogsSignedBlobURL") {
            return std::future::pending().await;
        }
        if matches!(fake.mode, Mode::Renewal | Mode::LostLease)
            && path.ends_with("GetStepLogsSignedBlobURL")
        {
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(2)).await;
            tokio::time::resume();
            fake.renewed.notified().await;
        }
        let suffix = if path.ends_with("GetStepLogsSignedBlobURL") {
            "step"
        } else {
            "job"
        };
        json!({"logs_url":format!("{base}blob/{suffix}?sig=sas-secret"),"blob_storage_type":"BLOB_STORAGE_TYPE_AZURE"})
    } else if path.starts_with("/blob/") {
        if fake.mode == Mode::BrokenBlob {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        return StatusCode::CREATED.into_response();
    } else if path == "/run/completejob" {
        let completed = {
            let mut counters = fake.counters.lock().expect("counters");
            counters.1 += 1;
            counters.1
        };
        if completed == if fake.mode == Mode::Success { 2 } else { 1 }
            && let Some(sender) = fake.shutdown.lock().expect("shutdown").take()
        {
            sender.send(Ok(())).expect("send shutdown");
        }
        json!({})
    } else if path == "/broker/acknowledge" || method == Method::DELETE {
        json!({})
    } else if path.starts_with("/results/") {
        json!({"ok":true})
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };
    Json(response).into_response()
}

async fn exercise(
    mode: Mode,
) -> (
    Fake,
    std::result::Result<runner::RunSummary, crate::error::Failure>,
) {
    let directory = tempfile::tempdir().expect("temporary directory");
    let marker = directory.path().join("must-not-exist");
    let (tx, rx) = oneshot::channel();
    let fake = Fake {
        mode,
        events: Default::default(),
        counters: Default::default(),
        renewals: Default::default(),
        renewed: Default::default(),
        shutdown: Arc::new(Mutex::new(Some(tx))),
        feed_received: Default::default(),
        marker: marker.to_string_lossy().into_owned(),
    };
    let server = Server::start(
        Router::new()
            .route("/feed", get(feed))
            .fallback(api)
            .with_state(fake.clone()),
    )
    .await;
    let config = Config::new(
        "example/project",
        vec!["carcinize".into()],
        Some("fake-test".into()),
        "administrative-secret".into(),
    )
    .expect("config");
    let result = tokio::time::timeout(
        Duration::from_secs(30),
        runner::run_with(
            config,
            Http::new().expect("client"),
            server.base.clone(),
            Identity::test_identity(),
            async { rx.await.expect("shutdown sender") },
            Duration::from_secs(if matches!(mode, Mode::Renewal | Mode::LostLease) {
                1
            } else {
                60
            }),
        ),
    )
    .await
    .expect("runner finishes");
    assert!(
        !marker.exists(),
        "script declaration must never be executed"
    );
    (fake, result)
}

#[tokio::test]
async fn handles_two_jobs_with_live_and_archived_logs_then_removes_its_runner() {
    let (fake, result) = exercise(Mode::Success).await;
    assert_eq!(result.expect("successful runner").jobs_inspected, 2);
    let events = fake.events.lock().expect("events");
    assert_eq!(
        events
            .iter()
            .filter(|(name, _, _)| name == "POST /actions/runners/register")
            .count(),
        1
    );
    assert_eq!(
        events
            .iter()
            .filter(|(name, _, _)| name == "POST /run/completejob")
            .count(),
        2
    );
    let feed = events
        .iter()
        .position(|(name, _, _)| name == "feed-lines")
        .expect("live lines");
    let completion = events
        .iter()
        .position(|(name, _, _)| name == "POST /run/completejob")
        .expect("completion");
    assert!(feed < completion);
    for (name, auth, body) in events.iter() {
        if name.starts_with("PUT /blob/") {
            assert_eq!(
                auth, &None,
                "signed URLs must not receive bearer credentials"
            );
            let text = body.as_str().expect("log body");
            assert!(text.contains("exit 97"));
            assert!(!text.contains("job-secret-value"));
            assert!(text.contains("***"));
        }
        if name == "feed-connect"
            || name.starts_with("POST /results/")
            || name.starts_with("POST /run/")
        {
            assert_eq!(auth.as_deref(), Some("Bearer job-token"));
        }
        if name == "POST /run/completejob" {
            assert_eq!(body["conclusion"], 0);
        }
    }
    assert!(events.iter().any(|(n, _, _)| n == "DELETE /broker/session"));
    assert_eq!(
        events.last().expect("cleanup").0,
        "DELETE /repos/example/project/actions/runners/42"
    );
}

#[tokio::test]
async fn failed_log_upload_cannot_report_success() {
    let (fake, _) = exercise(Mode::BrokenBlob).await;
    let events = fake.events.lock().expect("events");
    let body = &events
        .iter()
        .find(|(n, _, _)| n == "POST /run/completejob")
        .expect("failed completion")
        .2;
    assert_eq!(body["conclusion"], 2);
}

#[tokio::test]
async fn unknown_step_semantics_fail_the_acquired_job() {
    let (fake, _) = exercise(Mode::BadStep).await;
    let events = fake.events.lock().expect("events");
    let body = &events
        .iter()
        .find(|(n, _, _)| n == "POST /run/completejob")
        .expect("failed completion")
        .2;
    assert_eq!(body["conclusion"], 2);
}

#[tokio::test]
async fn session_failure_still_removes_the_new_registration() {
    let (fake, result) = exercise(Mode::BadSession).await;
    assert!(result.is_err());
    assert_eq!(
        fake.events
            .lock()
            .expect("events")
            .last()
            .expect("cleanup")
            .0,
        "DELETE /repos/example/project/actions/runners/42"
    );
}

#[tokio::test]
async fn cancellation_interrupts_reporting_and_completes_as_cancelled() {
    let (fake, _) = exercise(Mode::Cancel).await;
    let events = fake.events.lock().expect("events");
    let body = &events
        .iter()
        .find(|(n, _, _)| n == "POST /run/completejob")
        .expect("cancelled completion")
        .2;
    assert_eq!(body["conclusion"], 3);
}

#[tokio::test]
async fn renews_periodically_while_reporting_is_still_in_progress() {
    let (fake, result) = exercise(Mode::Renewal).await;
    assert_eq!(result.expect("runner").jobs_inspected, 1);
    assert!(fake.renewals.load(std::sync::atomic::Ordering::SeqCst) >= 2);
}

#[tokio::test]
async fn losing_the_lease_interrupts_reporting_and_prevents_success() {
    let (fake, result) = exercise(Mode::LostLease).await;
    assert_eq!(result.expect("runner").jobs_failed, 1);
    let events = fake.events.lock().expect("events");
    let body = &events
        .iter()
        .find(|(name, _, _)| name == "POST /run/completejob")
        .expect("failed completion")
        .2;
    assert_eq!(body["conclusion"], 2);
}
