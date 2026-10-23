//! Credential-free protocol declaration shapes. Semantic enums reject unknown variants.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Step {
    pub name: StepName,
    pub script: Vec<String>,
    pub timeout: u64,
    pub when: When,
    pub allow_failure: bool,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum StepName {
    Script,
    AfterScript,
    Release,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum When {
    OnSuccess,
    OnFailure,
    Always,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Hook {
    pub name: HookName,
    pub script: Vec<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub(super) enum HookName {
    PreGetSourcesScript,
    PostGetSourcesScript,
}

#[derive(Deserialize, Serialize)]
pub(super) struct Variable {
    pub key: String,
    pub value: Option<String>,
    #[serde(default)]
    pub public: bool,
    #[serde(default)]
    pub masked: bool,
    #[serde(default)]
    pub file: bool,
    #[serde(default)]
    pub raw: bool,
}

#[derive(Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub(super) struct Image {
    pub name: String,
    alias: Option<String>,
    command: Option<Vec<String>>,
    entrypoint: Option<Vec<String>>,
    ports: Option<Vec<Port>>,
    pub variables: Option<Vec<Variable>>,
    pull_policy: Option<Vec<PullPolicy>>,
    executor_opts: Option<ExecutorOptions>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct Port {
    number: u16,
    protocol: String,
    name: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum PullPolicy {
    Always,
    Never,
    IfNotPresent,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ExecutorOptions {
    docker: Option<DockerOptions>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct DockerOptions {
    platform: Option<String>,
    user: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Artifact {
    name: Option<String>,
    #[serde(default)]
    untracked: bool,
    #[serde(default)]
    paths: Option<Vec<String>>,
    #[serde(default)]
    exclude: Vec<String>,
    when: Option<When>,
    artifact_type: Option<String>,
    artifact_format: Option<ArtifactFormat>,
    expire_in: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum ArtifactFormat {
    Zip,
    Gzip,
    Raw,
    Zipzstd,
    Tarzstd,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Cache {
    key: Option<String>,
    untracked: Option<bool>,
    policy: Option<CachePolicy>,
    #[serde(default)]
    paths: Option<Vec<String>>,
    when: Option<When>,
    #[serde(default)]
    fallback_keys: Option<Vec<String>>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "kebab-case")]
enum CachePolicy {
    PullPush,
    Pull,
    Push,
}

#[derive(Deserialize, Serialize)]
pub(super) struct JobInfo {
    pub name: String,
    pub stage: String,
    pub project_id: u64,
    pub project_name: String,
}

#[derive(Deserialize, Serialize)]
pub(super) struct GitInfo {
    pub repo_url: String,
    #[serde(default)]
    refspecs: Vec<String>,
    #[serde(rename = "ref")]
    reference: String,
    sha: String,
    before_sha: String,
    ref_type: RefType,
    depth: u64,
    repo_object_format: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
enum RefType {
    Branch,
    Tag,
}

#[derive(Deserialize, Serialize)]
pub(super) struct RunnerInfo {
    pub timeout: u64,
}

#[derive(Deserialize, Serialize)]
pub(super) struct Dependency {
    pub id: u64,
    pub name: String,
    pub artifacts_file: Option<ArtifactFile>,
}

#[derive(Deserialize, Serialize)]
pub(super) struct ArtifactFile {
    filename: String,
    size: u64,
}

// Secrets are declarations, not resolved values. Their provider data can contain credentials;
// rendering these fields is deliberately excluded until a credential-safe typed adapter exists.
pub(super) type SecretNames = BTreeMap<String, serde_json::Value>;

// Go's optional slices accept both JSON null and absent fields as an empty collection.
pub(super) fn null_default<'de, D, T>(deserializer: D) -> std::result::Result<T, D::Error>
where
    D: serde::Deserializer<'de>,
    T: Deserialize<'de> + Default,
{
    Ok(Option::<T>::deserialize(deserializer)?.unwrap_or_default())
}
