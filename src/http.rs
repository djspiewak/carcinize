use crate::error::{Failure, Result};
use reqwest::{Client, RequestBuilder, Response, header};
use serde::de::DeserializeOwned;
use std::time::Duration;
use url::Url;

pub(crate) const COMPATIBLE_RUNNER_VERSION: &str = "2.338.1";
pub(crate) const USER_AGENT: &str = "carcinize/0.1.0 (GitHub runner protocol 2.338.1)";
pub(crate) const MAX_BODY: usize = 8 * 1024 * 1024;
pub(crate) const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
pub(crate) const POLL_TIMEOUT: Duration = Duration::from_secs(90);

#[derive(Clone)]
pub(crate) struct Http {
    pub client: Client,
}

impl Http {
    pub fn new() -> Result<Self> {
        Self::with_user_agent(USER_AGENT)
    }

    pub fn with_user_agent(user_agent: &'static str) -> Result<Self> {
        let client = Client::builder()
            .user_agent(user_agent)
            .connect_timeout(Duration::from_secs(10))
            .read_timeout(POLL_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| transport("construct HTTP client", e))?;
        Ok(Self { client })
    }

    pub async fn send(&self, request: RequestBuilder, operation: &'static str) -> Result<Response> {
        let response = self.exchange(request, operation).await?;
        if !response.status().is_success() {
            return Err(Failure::Http {
                operation,
                status: response.status().as_u16(),
            });
        }
        Ok(response)
    }

    // Some runner protocols use non-2xx responses for range reconciliation and cancellation.
    pub async fn exchange(
        &self,
        request: RequestBuilder,
        operation: &'static str,
    ) -> Result<Response> {
        request.send().await.map_err(|e| transport(operation, e))
    }

    pub async fn json<T: DeserializeOwned>(
        &self,
        request: RequestBuilder,
        operation: &'static str,
    ) -> Result<T> {
        let response = self.send(request, operation).await?;
        self.decode(response, operation).await
    }

    pub async fn decode<T: DeserializeOwned>(
        &self,
        mut response: Response,
        operation: &'static str,
    ) -> Result<T> {
        if response
            .content_length()
            .is_some_and(|n| n > MAX_BODY as u64)
        {
            return Err(Failure::Protocol("response exceeds 8 MiB"));
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|e| transport(operation, e))?
        {
            if chunk.len() > MAX_BODY.saturating_sub(bytes.len()) {
                return Err(Failure::Protocol("response exceeds 8 MiB"));
            }
            bytes.extend_from_slice(&chunk);
        }
        serde_json::from_slice(&bytes).map_err(|_| Failure::Protocol(operation))
    }
}

pub(crate) fn transport(operation: &'static str, error: reqwest::Error) -> Failure {
    Failure::Transport {
        operation,
        source: error.without_url(),
    }
}

pub(crate) fn endpoint(base: &Url, path: &str) -> Result<Url> {
    let mut base = base.clone();
    if !base.path().ends_with('/') {
        base.set_path(&format!("{}/", base.path()));
    }
    base.join(path)
        .map_err(|_| Failure::Protocol("invalid service endpoint"))
}

pub(crate) fn service_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|_| Failure::Protocol("invalid service URL"))?;
    let local_test = cfg!(test) && url.scheme() == "http" && url.host_str() == Some("127.0.0.1");
    if (!local_test && url.scheme() != "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.fragment().is_some()
    {
        return Err(Failure::Protocol(
            "service URL must use HTTPS without user information or fragments",
        ));
    }
    Ok(url)
}

pub(crate) fn authorization(scheme: &str, token: &str) -> Result<header::HeaderValue> {
    let mut header = header::HeaderValue::from_str(&format!("{scheme} {token}"))
        .map_err(|_| Failure::Protocol("invalid authorization credential"))?;
    header.set_sensitive(true);
    Ok(header)
}
