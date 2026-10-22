use anyhow::Context;
use clap::{Parser, Subcommand};

#[derive(Parser)]
#[command(
    version,
    about = "Inspect GitHub Actions jobs without executing their steps"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Register a transient runner and inspect jobs until Ctrl-C.
    Run {
        /// Repository on github.com, as owner/repo.
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
    let Command::Run { repo, labels, name } = Cli::parse().command;
    let token = std::env::var("GITHUB_TOKEN")
        .context("GITHUB_TOKEN must contain a repository administration token")?;
    let config = carcinize::Config::new(&repo, labels, name, token)?;
    let summary = carcinize::run(config, tokio::signal::ctrl_c()).await?;
    println!("{}", serde_json::to_string(&summary)?);
    Ok(())
}
