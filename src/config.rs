use crate::error::{Error, Failure};
use url::Url;
use uuid::Uuid;

pub(crate) enum Provider {
    Github,
    Gitlab(Url),
}

/// Validated, in-memory runner configuration. Credentials are deliberately not debug-printable.
pub struct Config {
    pub(crate) provider: Provider,
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
        Self::validated(Provider::Github, repository, labels, name, token)
    }

    /// Validate a GitLab host, namespaced project path, tags, name, and administrative token.
    ///
    /// Hosts may be bare authorities or HTTPS base URLs (including deployment prefixes).
    /// No Git remote is consulted. Empty tags default to `carcinize`; names are unique by
    /// default. Returns an error for invalid inputs without including credentials.
    pub fn new_gitlab(
        host: &str,
        project: &str,
        labels: Vec<String>,
        name: Option<String>,
        token: String,
    ) -> Result<Self, Error> {
        let parts: Vec<_> = project.split('/').collect();
        if parts.len() < 2
            || parts.iter().any(|part| {
                part.is_empty()
                    || matches!(*part, "." | "..")
                    || !part
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            })
        {
            return Err(Failure::Config("GitLab project must be a namespaced project path").into());
        }
        if host.is_empty()
            || host.trim() != host
            || host.chars().any(char::is_control)
            || host.contains('\\')
        {
            return Err(Failure::Config("invalid GitLab host").into());
        }
        let value = if host.contains("://") {
            host.to_owned()
        } else {
            format!("https://{host}")
        };
        let mut url = Url::parse(&value).map_err(|_| Failure::Config("invalid GitLab host"))?;
        if url.scheme() != "https"
            || url.host_str().is_none()
            || !url.username().is_empty()
            || url.password().is_some()
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(Failure::Config(
                "GitLab host must use HTTPS without credentials, query, or fragment",
            )
            .into());
        }
        if !url.path().ends_with('/') {
            url.set_path(&format!("{}/", url.path()));
        }
        Self::validated(Provider::Gitlab(url), project, labels, name, token)
    }

    fn validated(
        provider: Provider,
        repository: &str,
        labels: Vec<String>,
        name: Option<String>,
        token: String,
    ) -> Result<Self, Error> {
        if token.trim().is_empty() {
            return Err(Failure::Config(match provider {
                Provider::Github => "GITHUB_TOKEN is required",
                Provider::Gitlab(_) => "GITLAB_TOKEN is required",
            })
            .into());
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
            provider,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gitlab_host_is_explicit_and_preserves_deployment_prefix() {
        let config = Config::new_gitlab(
            "https://example.test:8443/gitlab",
            "group/sub/project",
            vec![],
            None,
            "token".into(),
        )
        .expect("config");
        let Provider::Gitlab(url) = config.provider else {
            panic!("provider")
        };
        assert_eq!(url.as_str(), "https://example.test:8443/gitlab/");
        assert_eq!(config.labels, ["carcinize"]);
        assert!(
            Config::new_gitlab(
                "example.test:8443",
                "group/repo",
                vec![],
                None,
                "token".into()
            )
            .is_ok()
        );
    }

    #[test]
    fn rejects_unsafe_hosts_and_project_paths_without_echoing_them() {
        for host in [
            "",
            "http://example.test",
            "https://user:secret@example.test",
            "https://example.test?q=secret",
            "https://example.test/#secret",
            "example.test\\evil",
            " example.test",
        ] {
            let err = Config::new_gitlab(host, "a/b", vec![], None, "secret".into())
                .err()
                .expect("invalid");
            assert!(!err.to_string().contains("secret"));
        }
        for project in ["repo", "/a/b", "a//b", "a/../b", "a/b?x", "a/%2fb"] {
            assert!(
                Config::new_gitlab("example.test", project, vec![], None, "token".into()).is_err()
            );
        }
    }
}
