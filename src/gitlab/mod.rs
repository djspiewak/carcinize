//! GitLab runner protocol, pinned to Runner 17.7.1.
mod api;
mod context;
mod declarations;
mod model;
mod native;
mod numbers;
mod preparation;
mod reporting;
pub(crate) mod runner;
mod trace;

#[cfg(test)]
mod tests;
