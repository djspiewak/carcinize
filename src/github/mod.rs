//! GitHub Actions runner protocols and declarations.
pub(crate) mod auth;
pub(crate) mod broker;
pub(crate) mod job_context;
pub(crate) mod model;
pub(crate) mod protocol_enum;
pub(crate) mod reporting;
pub(crate) mod runner;

pub(crate) const COMPATIBLE_RUNNER_VERSION: &str = "2.338.1";
pub(crate) const USER_AGENT: &str = "carcinize/0.1.0 (GitHub runner protocol 2.338.1)";

mod preparation;
