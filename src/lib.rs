//! A standalone, inspection-only GitHub Actions runner.

mod auth;
mod broker;
mod config;
mod error;
mod http;
mod job_context;
mod model;
mod redact;
mod reporting;
mod runner;

pub use config::Config;
pub use error::Error;
pub use runner::{RunSummary, run};

#[cfg(test)]
mod tests;
