use crate::{
    config::{runner_arch, runner_os},
    error::{Failure, Result},
    github::auth::RunnerAuth,
    http::*,
};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

pub(crate) struct Broker {
    http: Http,
    auth: RunnerAuth,
    base: Url,
    session_id: Uuid,
}

pub(crate) enum Message {
    Job(Assignment),
    Cancel(Uuid),
}

#[derive(Deserialize)]
pub(crate) struct Assignment {
    pub runner_request_id: String,
    pub run_service_url: String,
    #[serde(default)]
    pub should_acknowledge: bool,
    pub billing_owner_id: Option<String>,
}

impl Broker {
    pub async fn connect(
        http: Http,
        mut auth: RunnerAuth,
        base: Url,
        id: u64,
        name: &str,
    ) -> Result<Self> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Session {
            session_id: Uuid,
        }
        let token = authorization("Bearer", auth.token(&http).await?)?;
        let session: Session = http.json(http.client.post(endpoint(&base, "session")?).header("authorization", token)
            .json(&json!({"ownerName":name,"agent":{"id":id,"name":name,"version":COMPATIBLE_RUNNER_VERSION,
                "osDescription":format!("{} {}",runner_os(),runner_arch())},"brokerSession":true})), "create broker session").await?;
        if session.session_id.is_nil() {
            return Err(Failure::Protocol("broker returned empty session identity"));
        }
        Ok(Self {
            http,
            auth,
            base,
            session_id: session.session_id,
        })
    }

    pub async fn poll(&mut self, busy: bool) -> Result<Option<Message>> {
        for attempt in 0..2 {
            let request = self
                .http
                .client
                .get(endpoint(&self.base, "message")?)
                .header(
                    "authorization",
                    authorization("Bearer", self.auth.token(&self.http).await?)?,
                )
                .query(&[
                    ("sessionId", self.session_id.to_string()),
                    ("status", if busy { "Busy" } else { "Online" }.into()),
                    ("runnerVersion", COMPATIBLE_RUNNER_VERSION.into()),
                    ("os", runner_os().into()),
                    ("architecture", runner_arch().into()),
                    ("disableUpdate", "true".into()),
                ])
                .timeout(POLL_TIMEOUT);
            let response = match self.http.send(request, "poll runner message").await {
                Err(e) if e.is_status(401) && attempt == 0 => {
                    self.auth.invalidate();
                    continue;
                }
                result => result?,
            };
            if matches!(response.status().as_u16(), 202 | 204) {
                return Ok(None);
            }
            #[derive(Deserialize)]
            #[serde(rename_all = "camelCase")]
            struct WireMessage {
                message_type: String,
                body: String,
            }
            let Some(wire) = self
                .http
                .decode::<Option<WireMessage>>(response, "decode broker message")
                .await?
            else {
                return Ok(None);
            };
            return match wire.message_type.as_str() {
                "RunnerJobRequest" => Ok(Some(Message::Job(
                    serde_json::from_str(&wire.body)
                        .map_err(|_| Failure::Protocol("malformed job assignment"))?,
                ))),
                "JobCancellation" => {
                    #[derive(Deserialize)]
                    #[serde(rename_all = "camelCase")]
                    struct Cancel {
                        job_id: Uuid,
                    }
                    let cancel: Cancel = serde_json::from_str(&wire.body)
                        .map_err(|_| Failure::Protocol("malformed cancellation"))?;
                    Ok(Some(Message::Cancel(cancel.job_id)))
                }
                _ => Err(Failure::Protocol(
                    "unsupported broker message; check runner protocol compatibility",
                )),
            };
        }
        Err(Failure::Protocol("runner authentication retry exhausted"))
    }

    pub async fn acquire(&mut self, assignment: &Assignment) -> Result<Value> {
        if assignment.should_acknowledge {
            let token = authorization("Bearer", self.auth.token(&self.http).await?)?;
            let response = self
                .http
                .send(
                    self.http
                        .client
                        .post(endpoint(&self.base, "acknowledge")?)
                        .header("authorization", token)
                        .query(&[
                            ("sessionId", self.session_id.to_string()),
                            ("runnerVersion", COMPATIBLE_RUNNER_VERSION.into()),
                            ("status", "Online".into()),
                            ("os", runner_os().into()),
                            ("architecture", runner_arch().into()),
                        ])
                        .json(&json!({"runnerRequestId":assignment.runner_request_id})),
                    "acknowledge assignment",
                )
                .await;
            // The official broker acknowledgment is best effort; acquisition owns the assignment.
            if let Err(error) = response {
                eprintln!("carcinize: {error}");
            }
        }
        let url = endpoint(&service_url(&assignment.run_service_url)?, "acquirejob")?;
        self.http
            .json(
                self.http
                    .client
                    .post(url)
                    .header(
                        "authorization",
                        authorization("Bearer", self.auth.token(&self.http).await?)?,
                    )
                    .json(
                        &json!({"jobMessageId":assignment.runner_request_id,"runnerOS":runner_os(),
                "billingOwnerId":assignment.billing_owner_id}),
                    ),
                "acquire job",
            )
            .await
    }

    pub async fn close(&mut self) -> Result<()> {
        let token = authorization("Bearer", self.auth.token(&self.http).await?)?;
        match self
            .http
            .send(
                self.http
                    .client
                    .request(Method::DELETE, endpoint(&self.base, "session")?)
                    .header("authorization", token)
                    .query(&[("sessionId", self.session_id.to_string())]),
                "delete broker session",
            )
            .await
        {
            Ok(_) => Ok(()),
            Err(e) if e.is_status(404) => Ok(()),
            Err(e) => Err(e),
        }
    }
}
