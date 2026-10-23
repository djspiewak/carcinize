use crate::{
    error::{Failure, Result},
    github::protocol_enum::ProtocolEnum,
    http::*,
    redact::Redactor,
};
use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use std::collections::BTreeMap;
use url::Url;
use uuid::Uuid;

pub(crate) struct JobContext {
    pub id: Uuid,
    pub plan_id: Uuid,
    pub run_url: Url,
    pub results_url: Option<String>,
    pub feed_url: Option<String>,
    pub token: String,
    pub billing_owner_id: Option<String>,
    secrets: Vec<String>,
    masks: Vec<Mask>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Envelope {
    job_id: Uuid,
    plan: Plan,
    resources: Resources,
    variables: BTreeMap<String, Variable>,
    billing_owner_id: Option<String>,
    #[serde(default)]
    mask: Vec<Mask>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Plan {
    plan_id: Uuid,
}
#[derive(Deserialize)]
struct Resources {
    endpoints: Vec<Service>,
}
#[derive(Deserialize)]
struct Service {
    name: String,
    url: String,
    authorization: ServiceAuth,
    #[serde(default)]
    data: BTreeMap<String, String>,
}
#[derive(Deserialize)]
struct ServiceAuth {
    parameters: BTreeMap<String, String>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Variable {
    value: String,
    #[serde(default)]
    is_secret: bool,
}
#[derive(Deserialize)]
struct Mask {
    #[serde(rename = "type")]
    kind: ProtocolEnum,
    value: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) enum Conclusion {
    Success,
    Failed,
    Cancelled,
}

impl Conclusion {
    pub fn task_result(self) -> &'static str {
        match self {
            Self::Success => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "canceled",
        }
    }
}

impl JobContext {
    pub fn decode(value: &Value) -> Result<Self> {
        let envelope: Envelope = serde_json::from_value(value.clone())
            .map_err(|_| Failure::Protocol("malformed job service envelope"))?;
        let connection = envelope
            .resources
            .endpoints
            .iter()
            .find(|e| e.name.eq_ignore_ascii_case("SystemVssConnection"))
            .ok_or(Failure::Protocol("job has no system connection"))?;
        let token = connection
            .authorization
            .parameters
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("AccessToken"))
            .map(|(_, v)| v.clone())
            .filter(|v| !v.is_empty())
            .ok_or(Failure::Protocol("job has no reporting token"))?;
        let secrets = envelope
            .variables
            .values()
            .filter(|v| v.is_secret)
            .map(|v| v.value.clone())
            .chain(
                envelope
                    .resources
                    .endpoints
                    .iter()
                    .flat_map(|e| e.authorization.parameters.values().cloned()),
            )
            .collect();
        let results_url = envelope
            .variables
            .get("system.github.results_endpoint")
            .map(|v| v.value.clone());
        let feed_url = connection
            .data
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("FeedStreamUrl"))
            .map(|(_, v)| v.clone());
        Ok(Self {
            id: envelope.job_id,
            plan_id: envelope.plan.plan_id,
            run_url: service_url(&connection.url)?,
            results_url,
            feed_url,
            token,
            billing_owner_id: envelope.billing_owner_id,
            secrets,
            masks: envelope.mask,
        })
    }

    pub fn redactor(&self, administrative_token: &str) -> Result<Redactor> {
        let mut redactor = Redactor::new(
            self.secrets
                .iter()
                .map(String::as_str)
                .chain([self.token.as_str(), administrative_token]),
        );
        for mask in &self.masks {
            let code = mask
                .kind
                .code(&[(1, "variable"), (2, "regex")])
                .ok_or(Failure::Protocol("unsupported secret masking hint"))?;
            match code {
                1 => redactor.add(&mask.value),
                2 => redactor
                    .add_pattern(&mask.value)
                    .map_err(|_| Failure::Protocol("unsupported secret masking expression"))?,
                _ => return Err(Failure::Protocol("unsupported secret masking hint")),
            }
        }
        Ok(redactor)
    }

    pub async fn renew(&self, http: &Http) -> Result<DateTime<Utc>> {
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        struct Renewed {
            locked_until: DateTime<Utc>,
        }
        let renewed: Renewed = http
            .json(
                http.client
                    .post(endpoint(&self.run_url, "renewjob")?)
                    .header("authorization", authorization("Bearer", &self.token)?)
                    .json(&json!({"planId":self.plan_id,"jobId":self.id})),
                "renew job",
            )
            .await?;
        if renewed.locked_until <= Utc::now() {
            return Err(Failure::Protocol("job lease has expired"));
        }
        Ok(renewed.locked_until)
    }

    pub async fn complete(
        &self,
        http: &Http,
        conclusion: Conclusion,
        steps: &[Value],
        diagnostic: Option<&str>,
    ) -> Result<()> {
        if let Some(message) = diagnostic {
            eprintln!("carcinize: job {}: {message}", self.id);
        }
        let annotations = diagnostic
            .map(|message| vec![json!({"level":"failure","message":message})])
            .unwrap_or_default();
        http.send(http.client.post(endpoint(&self.run_url,"completejob")?)
            .header("authorization", authorization("Bearer",&self.token)?)
            .json(&json!({"planId":self.plan_id,"jobId":self.id,"conclusion":conclusion.task_result(),
                "outputs":{},"stepResults":steps,"annotations":annotations,"billingOwnerId":self.billing_owner_id})),
            "complete job").await?;
        Ok(())
    }

    pub async fn maintain_lease(
        &self,
        http: &Http,
        mut locked_until: DateTime<Utc>,
        period: std::time::Duration,
    ) -> Result<()> {
        loop {
            let remaining = (locked_until - Utc::now())
                .to_std()
                .map_err(|_| Failure::Protocol("job lease expired"))?;
            tokio::time::sleep(period.min(remaining / 2)).await;
            let remaining = (locked_until - Utc::now())
                .to_std()
                .map_err(|_| Failure::Protocol("job lease expired"))?;
            locked_until = tokio::time::timeout(remaining, self.renew(http))
                .await
                .map_err(|_| Failure::Protocol("job renewal exceeded lease"))??;
        }
    }
}
