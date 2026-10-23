//! Native, inspection-only GitHub Actions and GitLab Pipeline runners.

mod config;
mod error;
mod github;
mod gitlab;
mod http;
mod redact;
mod runner;

pub use config::Config;
pub use error::Error;
pub use runner::{RunSummary, run};

#[cfg(test)]
mod tests;
