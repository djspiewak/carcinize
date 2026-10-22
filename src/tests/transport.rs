use super::*;
use crate::{
    auth::{Identity, RunnerAuth, RunnerAuthorization},
    broker::Broker,
    http::Http,
};
use axum::{
    Json,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test]
async fn unauthorized_poll_refreshes_the_runner_token_once() {
    let calls = Arc::new(AtomicUsize::new(0));
    let polls = Arc::new(AtomicUsize::new(0));
    let refreshes = calls.clone();
    let attempts = polls.clone();
    let server = Server::start(
        Router::new()
            .route(
                "/oauth",
                post(move || {
                    let calls = refreshes.clone();
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        Json(json!({"access_token":"runner-secret","expires_in":3600}))
                    }
                }),
            )
            .route(
                "/session",
                post(|| async {
                    Json(json!({"sessionId":"aaaaaaaa-aaaa-4aaa-8aaa-aaaaaaaaaaaa"}))
                }),
            )
            .route(
                "/message",
                get(move || {
                    let polls = attempts.clone();
                    async move {
                        if polls.fetch_add(1, Ordering::SeqCst) == 0 {
                            StatusCode::UNAUTHORIZED
                        } else {
                            StatusCode::ACCEPTED
                        }
                    }
                }),
            ),
    )
    .await;
    let http = Http::new().expect("HTTP");
    let authorization = RunnerAuthorization {
        client_id: "client".into(),
        authorization_url: server.base.join("oauth").expect("URL").to_string(),
        server_url: server.base.to_string(),
    };
    let auth = RunnerAuth::new(Identity::test_identity(), &authorization).expect("auth");
    let mut broker = Broker::connect(http, auth, server.base.clone(), 42, "test")
        .await
        .expect("session");
    assert!(broker.poll(false).await.expect("recovered poll").is_none());
    assert!(broker.poll(false).await.expect("cached token").is_none());
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn error_bodies_and_signed_urls_are_not_diagnostic_text() {
    let server = Server::start(
        Router::new().fallback(|| async { (StatusCode::FORBIDDEN, "reflected-super-secret") }),
    )
    .await;
    let http = Http::new().expect("HTTP");
    let error = http
        .send(
            http.client
                .get(server.base.join("?sig=secret-signature").expect("URL")),
            "test operation",
        )
        .await
        .expect_err("HTTP failure");
    let diagnostic = format!("{error:?} {error}");
    assert!(diagnostic.contains("403"));
    assert!(!diagnostic.contains("secret"));
}

#[tokio::test]
async fn redirects_do_not_forward_bearer_credentials() {
    let count = Arc::new(AtomicUsize::new(0));
    async fn destination(State(count): State<Arc<AtomicUsize>>) -> impl IntoResponse {
        count.fetch_add(1, Ordering::SeqCst);
        Json(json!({}))
    }
    let destination = Server::start(
        Router::new()
            .fallback(destination)
            .with_state(count.clone()),
    )
    .await;
    let location = destination.base.to_string();
    let origin = Server::start(Router::new().fallback(move || {
        let location = location.clone();
        async move { (StatusCode::TEMPORARY_REDIRECT, [("location", location)]) }
    }))
    .await;
    let http = Http::new().expect("HTTP");
    assert!(
        http.send(
            http.client.get(origin.base.clone()).bearer_auth("secret"),
            "redirect check"
        )
        .await
        .is_err()
    );
    assert_eq!(count.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn token_cache_expires_using_the_runtime_clock() {
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let server = Server::start(Router::new().fallback(move || {
        let count = count.clone();
        async move {
            count.fetch_add(1, Ordering::SeqCst);
            Json(json!({"access_token":"runner-secret","expires_in":62}))
        }
    }))
    .await;
    let http = Http::new().expect("HTTP");
    let authorization = RunnerAuthorization {
        client_id: "client".into(),
        authorization_url: server.base.to_string(),
        server_url: server.base.to_string(),
    };
    let mut auth = RunnerAuth::new(Identity::test_identity(), &authorization).expect("auth");
    auth.token(&http).await.expect("first token");
    tokio::time::pause();
    tokio::time::advance(std::time::Duration::from_secs(3)).await;
    tokio::time::resume();
    auth.token(&http).await.expect("refreshed token");
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn malformed_json_errors_do_not_reflect_input() {
    let server =
        Server::start(Router::new().fallback(|| async { "secret-token invalid json" })).await;
    let http = Http::new().expect("HTTP");
    let error = http
        .json::<Value>(http.client.get(server.base.clone()), "decode response")
        .await
        .expect_err("decode error");
    assert!(!format!("{error:?}").contains("secret-token"));
}
