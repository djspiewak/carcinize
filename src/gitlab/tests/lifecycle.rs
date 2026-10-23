use super::*;
use crate::{Config, http::Http, tests::Server};
use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::any,
};
use std::{
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::{Notify, oneshot};

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Normal,
    PatchFailure,
    LostPatch,
    BadRange,
    Pending,
    PendingForever,
    RemoteCancel,
    CleanupFailure,
    CleanupUnavailable,
    CleanupRateLimited,
    CleanupTransport,
    CleanupMissing,
    CleanupForbidden,
    RegistrationUnavailable,
    InvalidRegistration,
    Conflict,
    VerifyFailure,
    StopRegistration,
    StopAssignment,
    HeartbeatCancel,
    AlreadyCanceled,
    LostCompletion,
    NullRegistration,
    MalformedRegistration,
    StopPatch,
    LostPatchExhausted,
    StopFinal,
    StopFinalCancelFailure,
    HeartbeatRetry,
    HeartbeatExhausted,
    HeartbeatTransport,
    StopPatchSignalError,
    StopFinalSignalError,
}

struct Event {
    method: Method,
    path: String,
    headers: HeaderMap,
    body: Value,
}
struct Data {
    jobs: VecDeque<Value>,
    events: Vec<Event>,
    traces: BTreeMap<u64, Vec<u8>>,
    patches: usize,
    polls: usize,
    finals: usize,
    heartbeats: usize,
    canceled: bool,
    stop: Option<oneshot::Sender<()>>,
}
struct Fake {
    mode: Mode,
    data: Mutex<Data>,
    heartbeat: Notify,
}

impl Fake {
    fn stop(data: &mut Data) {
        if let Some(stop) = data.stop.take() {
            let _ = stop.send(());
        }
    }
}

fn response(status: u16, job_state: &str) -> Response {
    (
        StatusCode::from_u16(status).expect("status"),
        [("Job-Status", job_state)],
    )
        .into_response()
}

async fn handle(
    State(fake): State<Arc<Fake>>,
    method: Method,
    uri: Uri,
    headers: HeaderMap,
    bytes: Bytes,
) -> Response {
    let patch = method == Method::PATCH;
    let response = respond(&fake, method, uri, headers, bytes);
    let first_patch = patch && fake.data.lock().expect("lock").patches == 1;
    if first_patch
        && matches!(
            fake.mode,
            Mode::HeartbeatCancel
                | Mode::HeartbeatRetry
                | Mode::HeartbeatExhausted
                | Mode::HeartbeatTransport
                | Mode::StopPatch
        )
    {
        fake.heartbeat.notified().await;
    }
    response
}

fn respond(fake: &Fake, method: Method, uri: Uri, headers: HeaderMap, bytes: Bytes) -> Response {
    let path = uri
        .path()
        .strip_prefix("/gitlab/api/v4/")
        .expect("deployment prefix")
        .to_owned();
    let body: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    let mut data = fake.data.lock().expect("lock");
    data.events.push(Event {
        method: method.clone(),
        path: path.clone(),
        headers: headers.clone(),
        body: body.clone(),
    });
    if path == "projects/group%2Fsub%2Fproject" {
        assert_eq!(headers["PRIVATE-TOKEN"], "synthetic-admin");
        return Json(json!({"id":42})).into_response();
    }
    if path == "user/runners" {
        assert_eq!(headers["PRIVATE-TOKEN"], "synthetic-admin");
        assert_eq!(body["project_id"], 42);
        assert_eq!(body["run_untagged"], false);
        assert_eq!(body["locked"], true);
        if fake.mode == Mode::RegistrationUnavailable {
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        if fake.mode == Mode::StopRegistration {
            Fake::stop(&mut data);
        }
        let token = match fake.mode {
            Mode::NullRegistration => Value::Null,
            Mode::MalformedRegistration => json!({"unexpected":true}),
            Mode::InvalidRegistration => json!(""),
            _ => json!("synthetic-runner"),
        };
        return (StatusCode::CREATED, Json(json!({"id":7,"token":token}))).into_response();
    }
    if path == "runners/7" && method == Method::DELETE {
        assert_eq!(headers["PRIVATE-TOKEN"], "synthetic-admin");
        let attempts = data
            .events
            .iter()
            .filter(|e| e.method == Method::DELETE)
            .count();
        if attempts == 1 {
            match fake.mode {
                Mode::CleanupUnavailable => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
                Mode::CleanupRateLimited => return StatusCode::TOO_MANY_REQUESTS.into_response(),
                Mode::CleanupTransport => {
                    let body = axum::body::Body::from_stream(futures_util::stream::once(async {
                        Err::<Bytes, _>(std::io::Error::other("synthetic disconnect"))
                    }));
                    return Response::new(body);
                }
                _ => {}
            }
        }
        return if fake.mode == Mode::CleanupFailure {
            StatusCode::INTERNAL_SERVER_ERROR
        } else if fake.mode == Mode::CleanupForbidden {
            StatusCode::FORBIDDEN
        } else if fake.mode == Mode::CleanupMissing {
            StatusCode::NOT_FOUND
        } else {
            StatusCode::NO_CONTENT
        }
        .into_response();
    }
    if path == "runners/verify" {
        assert_eq!(body["token"], "synthetic-runner");
        assert!(
            body["system_id"]
                .as_str()
                .expect("system id")
                .starts_with("r_")
        );
        return if fake.mode == Mode::VerifyFailure {
            StatusCode::FORBIDDEN
        } else {
            StatusCode::OK
        }
        .into_response();
    }
    if path == "jobs/request" {
        assert_eq!(body["token"], "synthetic-runner");
        assert!(!headers.contains_key("PRIVATE-TOKEN"));
        data.polls += 1;
        if fake.mode == Mode::Conflict && data.polls == 1 {
            return StatusCode::CONFLICT.into_response();
        }
        if let Some(job) = data.jobs.pop_front() {
            if fake.mode == Mode::StopAssignment {
                Fake::stop(&mut data);
            }
            return (StatusCode::CREATED, Json(job)).into_response();
        }
        Fake::stop(&mut data);
        return (
            StatusCode::NO_CONTENT,
            [("X-GitLab-Last-Update", "queue-cursor")],
        )
            .into_response();
    }
    if path.ends_with("/cancel") {
        assert_eq!(path, "projects/42/jobs/101/cancel");
        assert_eq!(headers["PRIVATE-TOKEN"], "synthetic-admin");
        if fake.mode == Mode::StopFinalCancelFailure {
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }
        data.canceled = true;
        return Json(json!({"id":101,"status":"canceling"})).into_response();
    }
    if method == Method::GET && path == "projects/42/jobs/101" {
        assert_eq!(headers["PRIVATE-TOKEN"], "synthetic-admin");
        return Json(json!({"status": if fake.mode == Mode::AlreadyCanceled || data.canceled { "canceled" } else { "success" }})).into_response();
    }
    let id = path
        .split('/')
        .nth(1)
        .and_then(|id| id.parse::<u64>().ok())
        .expect("job route");
    if method == Method::PATCH {
        assert_eq!(headers["JOB-TOKEN"], "synthetic-job-token");
        assert!(!headers.contains_key("PRIVATE-TOKEN"));
        data.patches += 1;
        if fake.mode == Mode::PatchFailure {
            return response(500, "running");
        }
        if fake.mode == Mode::BadRange {
            return (StatusCode::RANGE_NOT_SATISFIABLE, [("Range", "0-9999999")]).into_response();
        }
        if fake.mode == Mode::AlreadyCanceled {
            return response(202, "canceled");
        }
        if fake.mode == Mode::LostPatchExhausted && data.patches > 1 && data.finals == 0 {
            return response(500, "running");
        }
        let (start, end) = headers["Content-Range"]
            .to_str()
            .expect("range")
            .split_once('-')
            .expect("pair");
        let start = start.parse::<usize>().expect("start");
        let end = end.parse::<usize>().expect("end");
        assert_eq!(end + 1 - start, bytes.len());
        let trace = data.traces.entry(id).or_default();
        if start != trace.len() {
            return (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [("Range", format!("0-{}", trace.len()))],
            )
                .into_response();
        }
        trace.extend_from_slice(&bytes);
        if matches!(fake.mode, Mode::LostPatch | Mode::LostPatchExhausted) && data.patches == 1 {
            return response(500, "running");
        }
        if fake.mode == Mode::StopPatch && data.patches == 1 {
            Fake::stop(&mut data);
        }
        return response(
            202,
            if fake.mode == Mode::RemoteCancel || data.canceled {
                "canceling"
            } else {
                "running"
            },
        );
    }
    assert_eq!(method, Method::PUT);
    assert_eq!(body["token"], "synthetic-job-token");
    if body["state"] == "running" {
        data.heartbeats += 1;
        if fake.mode == Mode::HeartbeatExhausted {
            return response(503, "running");
        }
        if fake.mode == Mode::HeartbeatRetry {
            match data.heartbeats {
                1 => return response(429, "running"),
                2 => return response(503, "running"),
                _ => fake.heartbeat.notify_one(),
            }
        }
        if fake.mode == Mode::HeartbeatTransport {
            if data.heartbeats == 1 {
                // A body stream failure disconnects the HTTP response at the boundary.
                let body = axum::body::Body::from_stream(futures_util::stream::once(async {
                    Err::<Bytes, _>(std::io::Error::other("synthetic disconnect"))
                }));
                return Response::new(body);
            }
            fake.heartbeat.notify_one();
        }
        if fake.mode == Mode::HeartbeatCancel {
            fake.heartbeat.notify_one();
            return response(200, "canceling");
        }
        return response(200, "running");
    }
    data.finals += 1;
    fake.heartbeat.notify_one();
    if fake.mode == Mode::AlreadyCanceled {
        return StatusCode::FORBIDDEN.into_response();
    }
    if fake.mode == Mode::LostCompletion && data.finals > 1 {
        return StatusCode::FORBIDDEN.into_response();
    }
    if fake.mode == Mode::PendingForever || (fake.mode == Mode::Pending && data.finals == 1) {
        return response(202, "running");
    }
    let trace = data.traces.get(&id).map(Vec::as_slice).unwrap_or_default();
    if body["output"]["bytesize"] != trace.len()
        || body["output"]["checksum"] != format!("crc32:{:08x}", crc32fast::hash(trace))
    {
        return response(
            412,
            if data.canceled {
                "canceling"
            } else {
                "running"
            },
        );
    }
    if (fake.mode == Mode::StopFinal && !data.canceled)
        || (fake.mode == Mode::StopFinalCancelFailure
            && !data.events.iter().any(|e| e.path.ends_with("/cancel")))
    {
        Fake::stop(&mut data);
        return response(202, "running");
    }
    if data.canceled && body["state"] == "success" {
        return response(202, "canceling");
    }
    let result = if data.canceled
        || matches!(
            fake.mode,
            Mode::RemoteCancel
                | Mode::AlreadyCanceled
                | Mode::StopAssignment
                | Mode::HeartbeatCancel
        ) {
        "canceled"
    } else if body["state"] == "success" {
        "success"
    } else {
        "failed"
    };
    if fake.mode == Mode::LostCompletion {
        response(500, "running")
    } else {
        response(200, result)
    }
}

async fn exercise(
    jobs: Vec<Value>,
    mode: Mode,
) -> (crate::error::Result<crate::RunSummary>, Arc<Fake>) {
    let signal_failure = matches!(
        mode,
        Mode::StopPatchSignalError | Mode::StopFinalSignalError
    );
    let mode = match mode {
        Mode::StopPatchSignalError => Mode::StopPatch,
        Mode::StopFinalSignalError => Mode::StopFinal,
        mode => mode,
    };
    let (stop, stopped) = oneshot::channel();
    let fake = Arc::new(Fake {
        mode,
        data: Mutex::new(Data {
            jobs: jobs.into(),
            events: vec![],
            traces: BTreeMap::new(),
            patches: 0,
            polls: 0,
            finals: 0,
            heartbeats: 0,
            canceled: false,
            stop: Some(stop),
        }),
        heartbeat: Notify::new(),
    });
    let server = Server::start(
        Router::new()
            .route("/{*path}", any(handle))
            .with_state(fake.clone()),
    )
    .await;
    let config = Config::new_gitlab(
        "https://configured.test/gitlab",
        "group/sub/project",
        vec!["inspect-only".into()],
        Some("test".into()),
        "synthetic-admin".into(),
    )
    .expect("config");
    let host = server.base.join("gitlab/").expect("prefix");
    let api =
        api::Api::new(Http::with_user_agent(api::USER_AGENT).expect("http"), &host).expect("api");
    let timing = reporting::Timing {
        idle: Duration::ZERO,
        retry: Duration::ZERO,
        heartbeat: Duration::from_millis(5),
        finish: Duration::from_millis(200),
    };
    let result = runner::run_with(
        config,
        api,
        async {
            stopped.await.expect("shutdown");
            if signal_failure {
                Err(std::io::Error::other("synthetic shutdown failure"))
            } else {
                Ok(())
            }
        },
        timing,
    )
    .await;
    (result, fake)
}

#[tokio::test]
async fn inspects_multiple_jobs_then_removes_only_its_runner() {
    let directory = tempfile::tempdir().expect("sentinel directory");
    let sentinel = directory.path().join("must-not-exist");
    let mut script = job();
    script["steps"][0]["script"]
        .as_array_mut()
        .expect("scripts")
        .push(json!(format!("touch {}", sentinel.display())));
    let mut native = native_job();
    native["id"] = json!(102);
    let (result, fake) = exercise(vec![script, native], Mode::Normal).await;
    let summary = result.expect("runner");
    assert_eq!(summary.jobs_inspected, 2);
    assert!(!sentinel.exists());
    let data = fake.data.lock().expect("lock");
    let all = data
        .traces
        .values()
        .flat_map(|b| b.iter().copied())
        .collect::<Vec<_>>();
    let text = String::from_utf8(all).expect("utf8");
    assert!(text.contains("exit 97"));
    assert!(!text.contains("native-password"));
    assert_eq!(data.events.last().expect("cleanup").path, "runners/7");
    assert_eq!(
        data.events
            .iter()
            .filter(|e| e.method == Method::DELETE)
            .count(),
        1
    );
    let registration = data
        .events
        .iter()
        .find(|e| e.path == "user/runners")
        .expect("registration");
    assert_eq!(registration.body["tag_list"], json!(["inspect-only"]));
    assert!(
        data.events
            .iter()
            .filter(|e| e.path.starts_with("jobs/"))
            .all(|e| !e.headers.contains_key("PRIVATE-TOKEN"))
    );
}

#[tokio::test]
async fn reconciles_lost_trace_acknowledgments_without_duplicate_bytes() {
    let (normal, expected) = exercise(vec![job()], Mode::Normal).await;
    normal.expect("baseline");
    let (result, actual) = exercise(vec![job()], Mode::LostPatch).await;
    assert_eq!(result.expect("recovery").jobs_inspected, 1);
    // Section timestamps are injected independently, so compare decoded declaration occurrences.
    for fake in [expected, actual] {
        let data = fake.data.lock().expect("lock");
        let text = String::from_utf8(data.traces[&101].clone()).expect("utf8");
        assert_eq!(text.matches("exit 97").count(), 1);
    }
}

#[tokio::test]
async fn waits_for_terminal_acknowledgment_after_accepted_completion() {
    let (result, fake) = exercise(vec![job()], Mode::Pending).await;
    assert_eq!(result.expect("completion").jobs_inspected, 1);
    assert_eq!(fake.data.lock().expect("lock").finals, 2);
}

#[tokio::test]
async fn pending_completion_is_bounded_and_cleanup_still_runs() {
    let (result, fake) = exercise(vec![job()], Mode::PendingForever).await;
    assert!(result.is_err());
    assert_eq!(
        fake.data
            .lock()
            .expect("lock")
            .events
            .last()
            .expect("cleanup")
            .path,
        "runners/7"
    );
}

#[tokio::test]
async fn reporting_or_decoding_failures_cannot_report_success() {
    for mode in [Mode::PatchFailure, Mode::BadRange] {
        let (result, _) = exercise(vec![job()], mode).await;
        assert!(result.is_err(), "unreconcilable trace cannot complete");
    }
    let mut malformed = job();
    malformed["steps"][0]["when"] = json!("unknown");
    let (result, _) = exercise(vec![malformed], Mode::Normal).await;
    assert_eq!(result.expect("failed decode acknowledged").jobs_failed, 1);
}

#[tokio::test]
async fn honors_remote_and_local_cancellation_without_inspecting_successfully() {
    for mode in [
        Mode::RemoteCancel,
        Mode::AlreadyCanceled,
        Mode::StopAssignment,
        Mode::HeartbeatCancel,
    ] {
        let (result, fake) = exercise(vec![job()], mode).await;
        let summary = result.expect("cancelled job");
        assert_eq!(summary.jobs_cancelled, 1);
        assert_eq!(summary.jobs_inspected, 0);
        assert_eq!(
            fake.data
                .lock()
                .expect("lock")
                .events
                .last()
                .expect("cleanup")
                .path,
            "runners/7"
        );
    }
}

#[tokio::test]
async fn cleans_up_failed_or_interrupted_registration() {
    for mode in [
        Mode::InvalidRegistration,
        Mode::VerifyFailure,
        Mode::StopRegistration,
    ] {
        let (result, fake) = exercise(vec![], mode).await;
        assert_eq!(result.is_ok(), mode == Mode::StopRegistration);
        assert_eq!(
            fake.data
                .lock()
                .expect("lock")
                .events
                .last()
                .expect("cleanup")
                .path,
            "runners/7"
        );
    }
    let (result, _) = exercise(vec![job()], Mode::CleanupFailure).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn owned_runner_deletion_retries_transient_failures() {
    for mode in [
        Mode::CleanupUnavailable,
        Mode::CleanupRateLimited,
        Mode::CleanupTransport,
    ] {
        let (result, fake) = exercise(vec![], mode).await;
        result.expect("cleanup recovered");
        let data = fake.data.lock().expect("lock");
        assert_eq!(
            data.events
                .iter()
                .filter(|e| e.method == Method::DELETE)
                .count(),
            2
        );
        assert_eq!(
            data.events
                .iter()
                .filter(|e| e.path == "user/runners")
                .count(),
            1
        );
    }
}

#[tokio::test]
async fn owned_runner_deletion_is_bounded_and_accepts_already_absent() {
    for (mode, attempts, status) in [
        (Mode::CleanupFailure, 4, Some(500)),
        (Mode::CleanupForbidden, 1, Some(403)),
        (Mode::CleanupMissing, 1, None),
    ] {
        let (result, fake) = exercise(vec![], mode).await;
        if let Some(status) = status {
            let error = result.expect_err("cleanup failed");
            assert!(error.is_status(status));
            assert!(error.to_string().contains("delete GitLab runner"));
        } else {
            result.expect("already absent");
        }
        let data = fake.data.lock().expect("lock");
        assert_eq!(
            data.events
                .iter()
                .filter(|e| e.method == Method::DELETE)
                .count(),
            attempts
        );
    }
}

#[tokio::test]
async fn transient_runner_creation_failure_is_never_retried() {
    let (result, fake) = exercise(vec![], Mode::RegistrationUnavailable).await;
    assert!(result.expect_err("ambiguous creation").is_status(503));
    let data = fake.data.lock().expect("lock");
    assert_eq!(
        data.events
            .iter()
            .filter(|e| e.path == "user/runners")
            .count(),
        1
    );
    assert!(!data.events.iter().any(|e| e.method == Method::DELETE));
}

#[tokio::test]
async fn assignment_conflicts_are_retried_and_foreign_projects_are_rejected() {
    let (result, _) = exercise(vec![job()], Mode::Conflict).await;
    assert_eq!(result.expect("retry").jobs_inspected, 1);
    let mut foreign = job();
    foreign["job_info"]["project_id"] = json!(999);
    let (result, fake) = exercise(vec![foreign], Mode::Normal).await;
    assert!(result.is_err());
    assert!(
        !fake
            .data
            .lock()
            .expect("lock")
            .events
            .iter()
            .any(|e| e.path.ends_with("/cancel"))
    );
}

#[tokio::test]
async fn confirms_outcome_after_losing_completion_and_job_token_revocation() {
    let (result, fake) = exercise(vec![job()], Mode::LostCompletion).await;
    assert_eq!(result.expect("confirmed completion").jobs_inspected, 1);
    assert!(
        fake.data
            .lock()
            .expect("lock")
            .events
            .iter()
            .any(|e| e.path == "projects/42/jobs/101")
    );
}

#[tokio::test]
async fn retains_runner_identity_when_registration_token_is_malformed() {
    for mode in [Mode::NullRegistration, Mode::MalformedRegistration] {
        let (result, fake) = exercise(vec![], mode).await;
        assert!(result.is_err());
        let data = fake.data.lock().expect("lock");
        assert_eq!(data.events.last().expect("cleanup").path, "runners/7");
        assert!(!data.events.iter().any(|e| e.path == "runners/verify"));
    }
}

#[tokio::test]
async fn malformed_metadata_keeps_job_identity_and_reports_failure() {
    for (key, value) in [
        ("variables", json!([{"key":"bad", "value":42}])),
        ("credentials", json!([{"username":"user", "password":null}])),
        ("dependencies", json!([{"token":false}])),
    ] {
        let mut payload = job();
        payload[key] = value;
        let (result, fake) = exercise(vec![payload], Mode::Normal).await;
        assert_eq!(result.expect("failure acknowledged").jobs_failed, 1);
        let data = fake.data.lock().expect("lock");
        assert!(
            data.events
                .iter()
                .any(|e| e.path == "jobs/101" && e.body["state"] == "failed")
        );
        assert_eq!(data.events.last().expect("cleanup").path, "runners/7");
    }
}

#[tokio::test]
async fn canceled_and_failed_traces_reconcile_bytes_accepted_without_acknowledgment() {
    for mode in [Mode::StopPatch, Mode::LostPatchExhausted] {
        let (result, fake) = exercise(vec![job()], mode).await;
        let summary = result.expect("terminal checksum reconciled");
        if mode == Mode::StopPatch {
            assert_eq!(summary.jobs_cancelled, 1);
        } else {
            assert_eq!(summary.jobs_failed, 1);
        }
        let data = fake.data.lock().expect("lock");
        assert!(data.finals >= 2, "412 must trigger reconciliation");
        let text = String::from_utf8(data.traces[&101].clone()).expect("utf8");
        assert_eq!(text.matches("exit 97").count(), 1);
        assert!(data.events.iter().any(|e| {
            e.method == Method::PATCH
                && e.headers["Content-Range"]
                    .to_str()
                    .expect("range")
                    .starts_with("0-")
        }));
    }
}

#[tokio::test]
async fn shutdown_during_pending_terminal_update_cancels_then_drains_completion() {
    let (result, fake) = exercise(vec![job()], Mode::StopFinal).await;
    assert_eq!(result.expect("canceled completion").jobs_cancelled, 1);
    let data = fake.data.lock().expect("lock");
    let cancel = data
        .events
        .iter()
        .position(|e| e.path.ends_with("/cancel"))
        .expect("cancel");
    assert!(
        data.events
            .iter()
            .skip(cancel + 1)
            .any(|e| e.body["state"] == "failed")
    );
    assert_eq!(data.events.last().expect("cleanup").path, "runners/7");
}

#[tokio::test]
async fn heartbeat_retries_transient_statuses_and_transport_failures() {
    for mode in [Mode::HeartbeatRetry, Mode::HeartbeatTransport] {
        let (result, fake) = exercise(vec![job()], mode).await;
        assert_eq!(result.expect("heartbeat recovered").jobs_inspected, 1);
        let data = fake.data.lock().expect("lock");
        assert!(data.heartbeats >= if mode == Mode::HeartbeatRetry { 3 } else { 2 });
    }
}

#[tokio::test]
async fn heartbeat_retry_exhaustion_fails_without_unbounded_retries() {
    let (result, fake) = exercise(vec![job()], Mode::HeartbeatExhausted).await;
    assert_eq!(result.expect("failure acknowledged").jobs_failed, 1);
    assert_eq!(fake.data.lock().expect("lock").heartbeats, 4);
}

#[tokio::test]
async fn oversized_masking_work_is_rejected_with_owned_job_completion() {
    let mut payload = job();
    payload["variables"] = json!((0..3000).map(|n| json!({"key":format!("KEY{n}"), "value":format!("sensitive-{n}"), "masked":true})).collect::<Vec<_>>());
    let (result, fake) = exercise(vec![payload], Mode::Normal).await;
    assert_eq!(result.expect("budget failure reported").jobs_failed, 1);
    let data = fake.data.lock().expect("lock");
    let text = String::from_utf8(data.traces[&101].clone()).expect("utf8");
    assert!(!text.contains("sensitive-"));
}

#[tokio::test]
async fn shutdown_cancellation_error_is_returned_even_if_finalization_succeeds() {
    let (result, fake) = exercise(vec![job()], Mode::StopFinalCancelFailure).await;
    assert!(
        result
            .expect_err("cancellation failure must be returned")
            .is_status(500)
    );
    let data = fake.data.lock().expect("lock");
    assert!(data.events.iter().any(|e| e.path.ends_with("/cancel")));
    assert_eq!(
        data.events.last().expect("runner cleanup").path,
        "runners/7"
    );
}

#[tokio::test]
async fn signal_errors_during_inspection_and_finalization_survive_cleanup() {
    for mode in [Mode::StopPatchSignalError, Mode::StopFinalSignalError] {
        let (result, fake) = exercise(vec![job()], mode).await;
        crate::tests::contracts::shutdown_error(result);
        let data = fake.data.lock().expect("lock");
        assert!(data.events.iter().any(|e| e.path.ends_with("/cancel")));
        assert_eq!(data.events.last().expect("cleanup").path, "runners/7");
    }
}

#[tokio::test]
async fn escaped_secrets_are_masked_before_trace_publication() {
    let mut payload = job();
    payload["variables"] = json!([
        {"key":"FIRST", "value":"masked-first\nmasked-second", "masked":true},
        {"key":"SECOND", "value":"quote\"secret\"", "masked":true}
    ]);
    payload["steps"][0]["script"] = json!(["masked-first\nmasked-second quote\"secret\"\nexit 97"]);
    let (result, fake) = exercise(vec![payload], Mode::Normal).await;
    assert_eq!(result.expect("inspection").jobs_inspected, 1);
    let data = fake.data.lock().expect("lock");
    let trace = String::from_utf8(data.traces[&101].clone()).expect("UTF-8");
    crate::tests::contracts::output(
        &trace,
        &["masked-first", "masked-second", "quote"],
        &["exit 97"],
    );
}
