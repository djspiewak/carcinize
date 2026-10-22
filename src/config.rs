use crate::error::{Error, Failure};
use uuid::Uuid;

/// Validated, in-memory runner configuration. Credentials are deliberately not debug-printable.
pub struct Config {
    pub(crate) repository: String,
    pub(crate) labels: Vec<String>,
    pub(crate) name: String,
    pub(crate) token: String,
}

impl Config {
    /// Validate a GitHub.com repository, custom labels, optional name, and administrative token.
    ///
    /// Empty label lists default to `carcinize`. Missing names get a unique invocation ID.
    /// Returns an error for invalid identifiers or a missing token; never includes the token.
    pub fn new(
        repository: &str,
        labels: Vec<String>,
        name: Option<String>,
        token: String,
    ) -> Result<Self, Error> {
        let parts = repository.split('/').collect::<Vec<_>>();
        if parts.len() != 2
            || parts.iter().any(|p| {
                p.is_empty()
                    || *p == "."
                    || *p == ".."
                    || !p
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            })
        {
            return Err(Failure::Config("repository must be owner/repo").into());
        }
        if token.trim().is_empty() {
            return Err(Failure::Config("GITHUB_TOKEN is required").into());
        }
        let labels = if labels.is_empty() {
            vec!["carcinize".into()]
        } else {
            labels
        };
        if labels
            .iter()
            .any(|l| l.trim().is_empty() || l.len() > 255 || l.chars().any(char::is_control))
        {
            return Err(Failure::Config("labels must be nonempty and at most 255 bytes").into());
        }
        let name = name.unwrap_or_else(|| format!("carcinize-{}", Uuid::new_v4()));
        if name.trim().is_empty() || name.len() > 64 || name.chars().any(char::is_control) {
            return Err(
                Failure::Config("runner name must be nonempty and at most 64 bytes").into(),
            );
        }
        Ok(Self {
            repository: repository.into(),
            labels,
            name,
            token,
        })
    }

    pub(crate) fn github_url(&self) -> String {
        format!("https://github.com/{}", self.repository)
    }
}

pub(crate) fn runner_os() -> &'static str {
    match std::env::consts::OS {
        "macos" => "OSX",
        "windows" => "Windows",
        _ => "Linux",
    }
}

pub(crate) fn runner_arch() -> &'static str {
    match std::env::consts::ARCH {
        "aarch64" => "ARM64",
        "x86_64" => "X64",
        "x86" => "X86",
        _ => "ARM",
    }
}
