//! Provider-neutral assertions; each fake retains its own wire protocol.
use crate::{
    RunSummary,
    error::{Failure, Result},
};

pub(crate) fn output(text: &str, secrets: &[&str], declarations: &[&str]) {
    for secret in secrets {
        assert!(!text.contains(secret), "output exposed a synthetic secret");
        let escaped = serde_json::to_string(secret).expect("JSON");
        let escaped = escaped
            .strip_prefix('"')
            .expect("quote")
            .strip_suffix('"')
            .expect("quote");
        assert!(
            !text.contains(escaped),
            "output exposed an escaped synthetic secret"
        );
    }
    for declaration in declarations {
        assert!(
            text.contains(declaration),
            "inspection omitted a preserved declaration"
        );
    }
    assert!(text.contains("***"), "expected masking marker");
}

pub(crate) fn shutdown_error(result: Result<RunSummary>) {
    assert!(
        matches!(result, Err(Failure::Signal(_))),
        "shutdown errors must survive finalization and cleanup"
    );
}
