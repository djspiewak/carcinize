use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};

#[derive(Clone, Copy, ValueEnum)]
enum Provider {
    Github,
    Gitlab,
}

#[derive(Parser)]
#[command(
    version,
    about = "Inspect GitHub Actions or GitLab Pipeline jobs without executing their steps"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Register a transient runner and inspect jobs until Ctrl-C.
    Run {
        /// CI provider; GitHub remains the default.
        #[arg(long, value_enum, default_value = "github")]
        provider: Provider,
        /// GitLab hostname[:port] or HTTPS base URL. Required for GitLab.
        #[arg(long, required_if_eq("provider", "gitlab"))]
        host: Option<String>,
        /// GitHub owner/repo or GitLab namespace/project (including subgroups).
        #[arg(long)]
        repo: String,
        /// Custom scheduling label; repeat to add labels. Defaults to carcinize.
        #[arg(long = "label")]
        labels: Vec<String>,
        /// Runner display name; defaults to a unique name for this invocation.
        #[arg(long)]
        name: Option<String>,
    },
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let Command::Run {
        provider,
        host,
        repo,
        labels,
        name,
    } = Cli::parse().command;
    let config = match provider {
        Provider::Github => {
            anyhow::ensure!(host.is_none(), "--host applies only to --provider gitlab");
            let token = std::env::var("GITHUB_TOKEN")
                .context("GITHUB_TOKEN must contain a repository administration token")?;
            carcinize::Config::new(&repo, labels, name, token)?
        }
        Provider::Gitlab => {
            let host = host.context("--host is required for GitLab")?;
            let token = std::env::var("GITLAB_TOKEN")
                .context("GITLAB_TOKEN must contain a project runner administration token")?;
            carcinize::Config::new_gitlab(&host, &repo, labels, name, token)?
        }
    };
    let summary = carcinize::run(config, tokio::signal::ctrl_c()).await?;
    println!("{}", serde_json::to_string(&summary)?);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn retains_github_default_and_requires_explicit_gitlab_host() {
        assert!(matches!(
            Cli::try_parse_from(["carcinize", "run", "--repo", "a/b"])
                .expect("legacy CLI")
                .command,
            Command::Run {
                provider: Provider::Github,
                ..
            }
        ));
        assert!(
            Cli::try_parse_from(["carcinize", "run", "--provider", "gitlab", "--repo", "a/b"])
                .is_err()
        );
        assert!(
            Cli::try_parse_from([
                "carcinize",
                "run",
                "--provider",
                "gitlab",
                "--host",
                "example.test",
                "--repo",
                "a/b"
            ])
            .is_ok()
        );
    }
}
