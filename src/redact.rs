//! Output masking happens before line splitting, JSON escaping, and transport.
use regex::Regex;

#[derive(Default)]
pub(crate) struct Redactor {
    literals: Vec<String>,
    patterns: Vec<Regex>,
}

impl Redactor {
    pub fn new<'a>(secrets: impl IntoIterator<Item = &'a str>) -> Self {
        let mut result = Self::default();
        for secret in secrets {
            result.add(secret);
        }
        result
    }

    pub fn add(&mut self, secret: &str) {
        if secret.is_empty() {
            return;
        }
        self.literals.push(secret.to_owned());
        for part in secret.lines().filter(|s| !s.is_empty()) {
            self.literals.push(part.to_owned());
        }
        if let Ok(escaped) = serde_json::to_string(secret)
            && let Some(inner) = escaped.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
        {
            self.literals.push(inner.to_owned());
        }
        self.literals.sort_by_key(|s| std::cmp::Reverse(s.len()));
        self.literals.dedup();
    }

    pub fn add_pattern(&mut self, pattern: &str) -> Result<(), regex::Error> {
        self.patterns.push(Regex::new(pattern)?);
        Ok(())
    }

    pub fn redact(&self, value: &str) -> String {
        let mut value = value.to_owned();
        for pattern in &self.patterns {
            value = pattern.replace_all(&value, "***").into_owned();
        }
        for secret in &self.literals {
            value = value.replace(secret, "***");
        }
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_overlapping_secrets_and_json_escaped_forms() {
        let masker = Redactor::new(["abcd", "abcdef", "quote\"newline\nsecret"]);
        assert_eq!(masker.redact("abcdef abcd"), "*** ***");
        assert!(
            !masker
                .redact(&serde_json::to_string("quote\"newline\nsecret").expect("JSON"))
                .contains("secret")
        );
    }

    #[test]
    fn empty_secrets_do_not_corrupt_output() {
        assert_eq!(Redactor::new([""]).redact("hello"), "hello");
    }
}
