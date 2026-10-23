//! Typed declarations from GitLab 17.7's run_steps JSON schema. Never resolve references.
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

type Environment = BTreeMap<String, String>;
type Values = BTreeMap<String, Value>;

#[derive(Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub(super) enum NativeStep {
    Script {
        name: String,
        script: String,
        #[serde(default)]
        env: Environment,
    },
    Step {
        name: String,
        step: Reference,
        #[serde(default)]
        inputs: Values,
        #[serde(default)]
        env: Environment,
    },
    Action {
        name: String,
        action: String,
        #[serde(default)]
        inputs: Values,
        #[serde(default)]
        env: Environment,
    },
    Run {
        run: Vec<NativeStep>,
        #[serde(default)]
        env: Environment,
        #[serde(default)]
        outputs: Values,
        delegate: Option<String>,
    },
    Steps {
        steps: Vec<NativeStep>,
        #[serde(default)]
        env: Environment,
        #[serde(default)]
        outputs: Values,
        delegate: Option<String>,
    },
    Exec {
        exec: Exec,
        #[serde(default)]
        env: Environment,
    },
}

#[derive(Deserialize, Serialize)]
#[serde(untagged, deny_unknown_fields)]
pub(super) enum Reference {
    Name(String),
    Git { git: GitReference },
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct GitReference {
    url: String,
    rev: String,
    dir: Option<String>,
}

#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Exec {
    command: Vec<String>,
    work_dir: Option<String>,
}

impl NativeStep {
    pub fn valid(&self, depth: usize) -> bool {
        if depth > 32 {
            return false;
        }
        let env = match self {
            Self::Script { name, script, env } => {
                if !identifier(name) || script.is_empty() {
                    return false;
                }
                env
            }
            Self::Step {
                name, inputs, env, ..
            } => {
                if !identifier(name) || !inputs.keys().all(|s| identifier(s)) {
                    return false;
                }
                env
            }
            Self::Action {
                name,
                action,
                inputs,
                env,
            } => {
                if !identifier(name) || action.is_empty() || !inputs.keys().all(|s| identifier(s)) {
                    return false;
                }
                env
            }
            Self::Run {
                run: steps,
                env,
                outputs,
                ..
            }
            | Self::Steps {
                steps,
                env,
                outputs,
                ..
            } => {
                if !steps.iter().all(|s| s.valid(depth + 1))
                    || !outputs.keys().all(|s| identifier(s))
                {
                    return false;
                }
                env
            }
            Self::Exec { exec, env } => {
                if exec.command.is_empty() {
                    return false;
                }
                env
            }
        };
        env.keys().all(|s| identifier(s))
    }
}

fn identifier(s: &str) -> bool {
    s.bytes()
        .next()
        .is_some_and(|b| b.is_ascii_alphabetic() || b == b'_')
        && s.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_')
}
