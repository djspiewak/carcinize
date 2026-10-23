//! GitLab-specific bounds around the shared literal redactor.
use crate::{
    error::{Failure, Result},
    redact::Redactor,
};

pub(super) struct Masking {
    pub redactor: Redactor,
    entries: usize,
    bytes: usize,
}
impl Masking {
    pub fn new() -> Self {
        Self {
            redactor: Redactor::default(),
            entries: 0,
            bytes: 0,
        }
    }
    pub fn check_work(&self, text_bytes: usize) -> Result<()> {
        // Each replacement can expand one source byte to the three-byte marker.
        if text_bytes.saturating_mul(self.entries).saturating_mul(3) > 64 * 1024 * 1024 {
            return Err(Failure::Protocol(
                "GitLab masking work exceeds 64 MiB scan budget",
            ));
        }
        Ok(())
    }
    pub fn add(&mut self, secret: &str) -> Result<()> {
        // The shared masker replaces with ***. Avoid recursive growth from
        // marker fragments repeated through multiline literal expansion.
        if secret
            .lines()
            .any(|line| matches!(line, "*" | "**" | "***"))
        {
            return Err(Failure::Protocol(
                "GitLab secret conflicts with masking marker",
            ));
        }
        self.entries = self
            .entries
            .saturating_add(secret.lines().count().saturating_add(2));
        self.bytes = self.bytes.saturating_add(secret.len());
        if self.entries > 384 || self.bytes > 32 * 1024 {
            return Err(Failure::Protocol(
                "GitLab masking metadata exceeds preparation budget",
            ));
        }
        self.redactor.add(secret);
        Ok(())
    }
}
