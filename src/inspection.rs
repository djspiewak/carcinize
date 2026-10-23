//! Provider-independent safe declaration rendering.
use crate::{
    error::{Failure, Result},
    redact::Masker,
};
use serde::Serialize;
use serde_json::Value;
use std::io::Write;

pub(crate) const MAX_OUTPUT: usize = 8 * 1024 * 1024;
pub(crate) const BANNER: &str =
    "CARCINIZE INSPECTION ONLY: declarations are described; nothing is executed or resolved.\n";

pub(crate) struct Bounded {
    pub bytes: Vec<u8>,
    limit: usize,
}
impl Bounded {
    pub fn new(limit: usize) -> Self {
        Self {
            bytes: Vec::new(),
            limit,
        }
    }
}

impl Write for Bounded {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            return Err(std::io::Error::other("inspection output exceeds budget"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

pub(crate) fn render<T: Serialize>(
    output: &mut Bounded,
    value: &T,
    masker: &mut Masker<'_>,
) -> Result<()> {
    masker.checkpoint()?;
    let mut value =
        serde_json::to_value(value).map_err(|_| Failure::Protocol("render declaration"))?;
    crate::preparation::validate(&value)?;
    mask(&mut value, masker)?;
    masker.checkpoint()?;
    serde_json::to_writer_pretty(output, &value)
        .map_err(|_| Failure::Protocol("inspection output exceeds budget"))
}

fn mask(value: &mut Value, masker: &mut Masker<'_>) -> Result<()> {
    masker.checkpoint()?;
    match value {
        Value::String(s) => *s = masker.redact(s)?,
        Value::Array(items) => {
            for value in items {
                mask(value, masker)?;
            }
        }
        Value::Object(values) => {
            let old = std::mem::take(values);
            for (key, mut value) in old {
                mask(&mut value, masker)?;
                if values.insert(masker.redact(&key)?, value).is_some() {
                    return Err(Failure::Protocol(
                        "masking declaration keys would lose fields",
                    ));
                }
            }
        }
        scalar => {
            let original = scalar.to_string();
            let masked = masker.redact(&original)?;
            if masked != original {
                *scalar = Value::String(masked);
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::redact::Redactor;
    use serde_json::json;

    #[test]
    fn cancellation_interrupts_regex_rendering_before_output_is_published() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        let mut redactor = Redactor::default();
        redactor.add_pattern("a.*b|a").expect("pattern");
        let canceled = AtomicBool::new(false);
        let checks = AtomicUsize::new(0);
        // Inject cancellation after rendering and several regex searches have
        // begun. No sleeps or scheduler races are needed to exercise the actual
        // rendering path used by the owned preparation worker.
        let checkpoint = || {
            if checks.fetch_add(1, Ordering::Relaxed) == 10 {
                canceled.store(true, Ordering::Relaxed);
            }
            if canceled.load(Ordering::Relaxed) {
                Err(Failure::Cancelled)
            } else {
                Ok(())
            }
        };
        let mut output = Bounded::new(MAX_OUTPUT);
        let result = render(
            &mut output,
            &json!("a".repeat(512)),
            &mut redactor.masker(&checkpoint),
        );
        assert!(matches!(result, Err(Failure::Cancelled)));
        assert_eq!(checks.load(Ordering::Relaxed), 11);
        assert!(output.bytes.is_empty());
    }

    #[test]
    fn work_budget_is_shared_across_rendered_sections() {
        let redactor = Redactor::new(["secret"]).expect("masker");
        let mut masker = redactor.masker(&|| Ok(()));
        let value = json!("x".repeat(1024 * 1024));
        // Each section is individually admissible, but a preparation cannot
        // reset the budget by splitting its declarations into more sections.
        let mut failed = false;
        for _ in 0..32 {
            let mut output = Bounded::new(MAX_OUTPUT);
            if render(&mut output, &value, &mut masker).is_err() {
                assert!(output.bytes.is_empty());
                failed = true;
                break;
            }
        }
        assert!(failed);
    }

    #[test]
    fn masks_decoded_regex_matches_before_escaping() {
        let mut redactor = Redactor::default();
        redactor
            .add_pattern(r#"secret\s+value|quote"secret""#)
            .expect("regex");
        let mut output = Bounded::new(1024);
        render(
            &mut output,
            &json!({"text":"secret\nvalue quote\"secret\"", "preserved":42}),
            &mut redactor.masker(&|| Ok(())),
        )
        .expect("render");
        let rendered: Value = serde_json::from_slice(&output.bytes).expect("JSON");
        assert_eq!(rendered["text"], "*** ***");
        assert_eq!(rendered["preserved"], 42);
    }

    #[test]
    fn key_collisions_and_output_expansion_fail_closed() {
        let redactor = Redactor::new(["secret"]).expect("redactor");
        let mut output = Bounded::new(1024);
        assert!(
            render(
                &mut output,
                &json!({"secret":1,"***":2}),
                &mut redactor.masker(&|| Ok(()))
            )
            .is_err()
        );
        let mut output = Bounded::new(32);
        assert!(
            render(
                &mut output,
                &json!("\n".repeat(32)),
                &mut Redactor::default().masker(&|| Ok(()))
            )
            .is_err()
        );
        assert!(output.bytes.len() <= 32);
    }
}
