//! GitHub step status, live output, and final log storage.
mod feed;

use crate::{
    error::{Failure, Result},
    github::job_context::{Conclusion, JobContext},
    github::preparation::Step,
    http::*,
    redact::Redactor,
};
use chrono::Utc;
use serde::Deserialize;
use serde_json::{Value, json};
use url::Url;
use uuid::Uuid;

const RECEIVER: &str = "twirp/results.services.receiver.Receiver/";
const STEPS: &str =
    "twirp/github.actions.results.api.v1.WorkflowStepUpdateService/WorkflowStepsUpdate";

enum Phase {
    Pending,
    Running {
        started: String,
    },
    Finished {
        started: Option<String>,
        completed: String,
        outcome: StepOutcome,
    },
}
#[derive(Clone, Copy)]
enum StepOutcome {
    Inspected,
    Failed,
    Cancelled,
    Skipped,
}

impl StepOutcome {
    fn task(self) -> &'static str {
        match self {
            Self::Inspected => "succeeded",
            Self::Failed => "failed",
            Self::Cancelled => "canceled",
            Self::Skipped => "skipped",
        }
    }
    fn results(self) -> u8 {
        match self {
            Self::Inspected => 2,
            Self::Failed => 3,
            Self::Cancelled => 4,
            Self::Skipped => 7,
        }
    }
}

struct Record {
    id: Uuid,
    number: usize,
    name: String,
    phase: Phase,
}

impl Record {
    fn wire(&self, results_api: bool) -> Value {
        let (status, conclusion, started, completed) = match &self.phase {
            Phase::Pending => (
                if results_api {
                    json!(5)
                } else {
                    json!("pending")
                },
                None,
                None,
                None,
            ),
            Phase::Running { started } => (
                if results_api {
                    json!(3)
                } else {
                    json!("inProgress")
                },
                None,
                Some(started),
                None,
            ),
            Phase::Finished {
                started,
                completed,
                outcome,
            } => (
                if results_api {
                    json!(6)
                } else {
                    json!("completed")
                },
                Some(if results_api {
                    json!(outcome.results())
                } else {
                    json!(outcome.task())
                }),
                started.as_ref(),
                Some(completed),
            ),
        };
        json!({"external_id":self.id,"number":self.number,"name":self.name,"status":status,
            "conclusion":conclusion.unwrap_or(if results_api { json!(0) } else { Value::Null }),"started_at":started,"completed_at":completed})
    }

    fn finish(&mut self, outcome: StepOutcome) {
        let started = match &self.phase {
            Phase::Running { started } => Some(started.clone()),
            _ => None,
        };
        self.phase = Phase::Finished {
            started,
            completed: timestamp(),
            outcome,
        };
    }
}

pub(super) struct Reporter<'a> {
    http: Http,
    context: &'a JobContext,
    results_url: Url,
    redactor: Redactor,
    records: Vec<Record>,
    change_order: u64,
    feed: Option<feed::Feed>,
    job_log: String,
    job_lines: usize,
}

impl<'a> Reporter<'a> {
    pub fn new(
        http: Http,
        context: &'a JobContext,
        redactor: Redactor,
        steps: &[Step],
    ) -> Result<Self> {
        let records = steps
            .iter()
            .enumerate()
            .map(|(i, s)| Record {
                id: s.id,
                number: i + 1,
                name: s.label.clone(),
                phase: Phase::Pending,
            })
            .collect();
        Ok(Self {
            http,
            context,
            results_url: service_url(context.results_url.as_deref().ok_or(Failure::Protocol(
                "job requires unsupported legacy results protocol",
            ))?)?,
            redactor,
            records,
            change_order: 0,
            feed: None,
            job_log: String::new(),
            job_lines: 0,
        })
    }

    pub async fn inspect(&mut self, steps: &[Step]) -> Result<()> {
        let url = self
            .context
            .feed_url
            .as_ref()
            .ok_or(Failure::Protocol("job has no live feed URL"))?;
        self.feed = Some(feed::Feed::connect(url, &self.context.token).await?);
        self.update_steps().await?;
        for (index, step) in steps.iter().enumerate() {
            let record = self
                .records
                .get_mut(index)
                .ok_or(Failure::Protocol("missing step record"))?;
            record.phase = Phase::Running {
                started: timestamp(),
            };
            self.update_steps().await?;
            self.job_log.push_str(&step.log);
            self.job_lines += step.lines.len();
            self.feed
                .as_mut()
                .ok_or(Failure::Protocol("missing live feed"))?
                .send(step.id, &step.lines)
                .await?;
            self.upload_log(Some(step.id), step.log.clone(), step.lines.len())
                .await?;
            self.records
                .get_mut(index)
                .ok_or(Failure::Protocol("missing step record"))?
                .finish(StepOutcome::Inspected);
            self.update_steps().await?;
        }
        Ok(())
    }

    async fn update_steps(&mut self) -> Result<()> {
        self.change_order += 1;
        let steps: Vec<_> = self.records.iter().map(|r| r.wire(true)).collect();
        self.http.send(self.http.client.post(endpoint(&self.results_url, STEPS)?)
            .header("authorization", authorization("Bearer",&self.context.token)?)
            .json(&json!({"steps":steps,"change_order":self.change_order,
                "workflow_run_backend_id":self.context.plan_id,"workflow_job_run_backend_id":self.context.id})),
            "update workflow steps").await?;
        Ok(())
    }

    async fn upload_log(&self, step: Option<Uuid>, log: String, line_count: usize) -> Result<()> {
        let mut body = json!({"workflow_run_backend_id":self.context.plan_id,"workflow_job_run_backend_id":self.context.id});
        if let Some(id) = step {
            body["step_backend_id"] = json!(id);
        }
        let (get, metadata) = if step.is_some() {
            ("GetStepLogsSignedBlobURL", "CreateStepLogsMetadata")
        } else {
            ("GetJobLogsSignedBlobURL", "CreateJobLogsMetadata")
        };
        #[derive(Deserialize)]
        struct SignedUrl {
            logs_url: String,
            blob_storage_type: String,
        }
        let signed: SignedUrl = self
            .http
            .json(
                self.http
                    .client
                    .post(endpoint(&self.results_url, &format!("{RECEIVER}{get}"))?)
                    .header(
                        "authorization",
                        authorization("Bearer", &self.context.token)?,
                    )
                    .json(&body),
                "obtain log upload URL",
            )
            .await?;
        if signed.blob_storage_type != "BLOB_STORAGE_TYPE_AZURE" {
            return Err(Failure::Protocol("unsupported log blob storage type"));
        }
        let url = service_url(&signed.logs_url)?;
        // Signed storage URLs carry their own authority. Never attach a GitHub bearer token.
        self.http
            .send(
                self.http
                    .client
                    .put(url)
                    .header("x-ms-blob-type", "BlockBlob")
                    .header("content-type", "text/plain")
                    .body(log),
                "upload log blob",
            )
            .await?;
        body["uploaded_at"] = json!(timestamp());
        body["line_count"] = json!(line_count);
        #[derive(Deserialize)]
        struct Metadata {
            ok: bool,
        }
        let metadata: Metadata = self
            .http
            .json(
                self.http
                    .client
                    .post(endpoint(
                        &self.results_url,
                        &format!("{RECEIVER}{metadata}"),
                    )?)
                    .header(
                        "authorization",
                        authorization("Bearer", &self.context.token)?,
                    )
                    .json(&body),
                "publish log metadata",
            )
            .await?;
        if !metadata.ok {
            return Err(Failure::Protocol("log metadata was rejected"));
        }
        Ok(())
    }

    pub fn settle(&mut self, inspection: &Result<()>) {
        let outcome = if matches!(inspection, Err(Failure::Cancelled)) {
            StepOutcome::Cancelled
        } else {
            StepOutcome::Failed
        };
        for record in &mut self.records {
            match record.phase {
                Phase::Running { .. } => record.finish(outcome),
                Phase::Pending => record.finish(StepOutcome::Skipped),
                Phase::Finished { .. } => {}
            }
        }
    }

    pub async fn publish(&mut self) -> Result<()> {
        let result = self.update_steps().await;
        let job_log = std::mem::take(&mut self.job_log);
        let upload = self.upload_log(None, job_log, self.job_lines).await;
        let close = match &mut self.feed {
            Some(feed) => feed.close().await,
            None => Ok(()),
        };
        crate::lifecycle::combine(crate::lifecycle::combine(result, upload), close)
    }

    pub async fn complete(self, inspection: Result<()>) -> Result<Conclusion> {
        let cancelled = matches!(inspection, Err(Failure::Cancelled));
        let failure = inspection.err();
        let conclusion = if cancelled {
            Conclusion::Cancelled
        } else if failure.is_some() {
            Conclusion::Failed
        } else {
            Conclusion::Success
        };
        let steps: Vec<_> = self.records.iter().map(|r| r.wire(false)).collect();
        let diagnostic = failure
            .as_ref()
            .map(|e| self.redactor.redact(&e.to_string()));
        self.context
            .complete(&self.http, conclusion, &steps, diagnostic.as_deref())
            .await?;
        Ok(conclusion)
    }
}

fn timestamp() -> String {
    Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true)
}
