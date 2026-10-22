use super::*;
use crate::{auth::*, config::Config, http::Http};
use axum::{
    Json,
    extract::State,
    http::HeaderMap,
    routing::{get, post},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};

type Registrations = Arc<Mutex<Vec<(String, Value)>>>;

#[tokio::test]
async fn tenant_registration_bootstrap_returns_a_modern_broker_identity() {
    use axum::{http::StatusCode, response::IntoResponse};
    let captured: Registrations = Default::default();
    async fn api(
        State(seen): State<Registrations>,
        headers: HeaderMap,
        uri: axum::http::Uri,
        body: axum::body::Bytes,
    ) -> axum::response::Response {
        let base = format!("http://{}/", headers["host"].to_str().expect("host"));
        let response = match uri.path() {
            "/repos/example/project/actions/runners/registration-token" => {
                json!({"token":"registration-secret"})
            }
            "/actions/runner-registration" => {
                json!({"url":format!("{base}tenant/"),"token":"tenant-secret","token_schema":"OAuthAccessToken"})
            }
            "/tenant/_apis/distributedtask/pools" => {
                assert_eq!(headers["authorization"], "Bearer tenant-secret");
                json!({"value":[{"id":17,"isInternal":true,"isHosted":false}]})
            }
            "/tenant/_apis/distributedtask/pools/17/agents" => {
                let body: Value = serde_json::from_slice(&body).expect("body");
                seen.lock().expect("capture").push((
                    headers["authorization"].to_str().expect("header").into(),
                    body,
                ));
                json!({"id":42,"authorization":{"clientId":"client","authorizationUrl":format!("{base}oauth")},
                    "properties":{"ServerUrlV2":{"$type":"System.String","$value":format!("{base}broker/")},
                        "UseV2Flow":{"$type":"System.Boolean","$value":true}}})
            }
            _ => return StatusCode::NOT_FOUND.into_response(),
        };
        Json(response).into_response()
    }
    let server = Server::start(Router::new().fallback(api).with_state(captured.clone())).await;
    let config = Config::new(
        "example/project",
        vec!["chosen".into()],
        None,
        "administrative-secret".into(),
    )
    .expect("config");
    let registered = register_runner(
        &Http::new().expect("HTTP"),
        &server.base,
        &config,
        &Identity::test_identity(),
    )
    .await
    .expect("tenant registration");
    assert_eq!(registered.id, 42);
    assert_eq!(
        registered.authorization.server_url,
        server.base.join("broker/").expect("URL").as_str()
    );
    let captured = captured.lock().expect("capture");
    assert_eq!(captured[0].0, "Bearer tenant-secret");
    let body = &captured[0].1;
    assert_eq!(body["disableUpdate"], true);
    assert_eq!(body["maxParallelism"], 1);
    assert!(body["authorization"]["publicKey"]["modulus"].is_string());
}

#[tokio::test]
async fn registers_requested_labels_and_uses_distinct_credentials() {
    let seen = Arc::new(Mutex::new(Vec::<(String, Value)>::new()));
    async fn token(headers: HeaderMap) -> Json<Value> {
        assert_eq!(headers["authorization"], "Bearer administrative-secret");
        Json(json!({"token":"registration-secret"}))
    }
    async fn tenant(headers: HeaderMap) -> Json<Value> {
        assert_eq!(headers["authorization"], "RemoteAuth registration-secret");
        Json(
            json!({"url":"https://unused.example/","token":"unused-tenant-secret","token_schema":"OAuthAccessToken","use_v2_flow":true}),
        )
    }
    async fn groups(headers: HeaderMap) -> Json<Value> {
        assert_eq!(headers["authorization"], "RemoteAuth registration-secret");
        Json(json!({"runner_groups":[{"id":17,"name":"Default","default":true}]}))
    }
    async fn register(
        State(seen): State<Registrations>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        seen.lock().expect("lock").push((
            headers["authorization"]
                .to_str()
                .expect("header")
                .to_owned(),
            body,
        ));
        Json(
            json!({"id":42,"name":"test-runner","authorization":{"client_id":"00000000-0000-4000-8000-000000000001",
            "authorization_url":"https://token.actions.githubusercontent.com/","server_url":"https://broker.actions.githubusercontent.com/"}}),
        )
    }
    let server = Server::start(
        Router::new()
            .route(
                "/repos/example/project/actions/runners/registration-token",
                post(token),
            )
            .route("/actions/runner-registration", post(tenant))
            .route("/repos/example/project/actions/runner-groups", get(groups))
            .route("/actions/runners/register", post(register))
            .with_state(seen.clone()),
    )
    .await;
    let config = Config::new(
        "example/project",
        vec!["chosen".into()],
        Some("test-runner".into()),
        "administrative-secret".into(),
    )
    .expect("config");
    let identity = Identity::test_identity();
    let runner = register_runner(
        &Http::new().expect("client"),
        &server.base,
        &config,
        &identity,
    )
    .await
    .expect("registration");
    assert_eq!(runner.id, 42);
    let captured = seen.lock().expect("lock");
    let (header, body) = &captured[0];
    assert_eq!(header, "RemoteAuth registration-secret");
    assert_eq!(body["group_id"], 17);
    assert_eq!(body["updates_disabled"], true);
    assert_eq!(body["ephemeral"], false);
    assert!(
        body["labels"]
            .as_array()
            .expect("labels")
            .iter()
            .any(|v| v["name"] == "chosen")
    );
    assert!(
        body["public_key"]
            .as_str()
            .expect("key")
            .starts_with("<RSAKeyValue>")
    );
    assert!(!body["public_key"].as_str().expect("key").contains("<D>"));
}

#[test]
fn validates_repository_and_never_includes_token_in_errors() {
    for repo in ["", "owner", "a/b/c", "../repo", "owner/a?b"] {
        let error = match Config::new(repo, vec![], None, "administrative-secret".into()) {
            Ok(_) => panic!("invalid repo"),
            Err(e) => e,
        };
        assert!(!error.to_string().contains("administrative-secret"));
    }
}

#[test]
fn signs_a_verifiable_ps256_client_assertion_with_injected_time() {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
    use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode};
    use rsa::traits::PublicKeyParts;
    let identity = Identity::test_identity();
    let assertion = identity
        .assertion("client-id", "https://token.example/", 1_700_000_000)
        .expect("assertion");
    let key = DecodingKey::from_rsa_components(
        &URL_SAFE_NO_PAD.encode(identity.key.n().to_bytes_be()),
        &URL_SAFE_NO_PAD.encode(identity.key.e().to_bytes_be()),
    )
    .expect("public key");
    let mut validation = Validation::new(Algorithm::PS256);
    validation.set_audience(&["https://token.example/"]);
    validation.validate_exp = false;
    let claims = decode::<Value>(&assertion, &key, &validation)
        .expect("valid signature")
        .claims;
    assert_eq!(claims["iss"], "client-id");
    assert_eq!(claims["sub"], "client-id");
    assert_eq!(claims["nbf"], 1_699_999_970_u64);
    assert_eq!(claims["exp"], 1_700_000_270_u64);
}
