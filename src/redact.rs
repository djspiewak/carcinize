//! Bounded mask construction and replacement over original, unescaped text.
use crate::error::{Failure, Result};
use regex_automata::nfa::thompson::{self, pikevm::PikeVM};

const MAX_ENTRIES: usize = 384;
const MAX_METADATA: usize = 32 * 1024;
const MAX_WORK: usize = 64 * 1024 * 1024;
const MAX_SEARCH_WORK: usize = 4 * 1024 * 1024;

#[derive(Default)]
pub(crate) struct Redactor {
    literals: Vec<String>,
    patterns: Vec<PikeVM>,
    entries: usize,
    bytes: usize,
}

impl Redactor {
    pub fn new<'a>(secrets: impl IntoIterator<Item = &'a str>) -> Result<Self> {
        let mut result = Self::default();
        for secret in secrets {
            result.add(secret)?;
        }
        Ok(result)
    }

    fn reserve(&mut self, entries: usize, bytes: usize) -> Result<()> {
        self.entries = self.entries.saturating_add(entries);
        self.bytes = self.bytes.saturating_add(bytes);
        if self.entries > MAX_ENTRIES || self.bytes > MAX_METADATA {
            return Err(Failure::Protocol(
                "masking metadata exceeds preparation budget",
            ));
        }
        Ok(())
    }

    pub fn check_work(&self, text_bytes: usize) -> Result<()> {
        let scans = self.entries;
        if text_bytes.saturating_mul(scans).saturating_mul(3) > MAX_WORK {
            return Err(Failure::Protocol("masking work exceeds 64 MiB scan budget"));
        }
        Ok(())
    }

    pub fn add(&mut self, secret: &str) -> Result<()> {
        if secret.is_empty() {
            return Ok(());
        }
        self.reserve(secret.lines().count().saturating_add(2), secret.len())?;
        self.literals.push(secret.to_owned());
        for part in secret.lines().filter(|s| !s.is_empty()) {
            self.literals.push(part.to_owned());
        }
        if let Ok(escaped) = serde_json::to_string(secret)
            && let Some(inner) = escaped.strip_prefix('"').and_then(|s| s.strip_suffix('"'))
        {
            self.literals.push(inner.to_owned());
        }
        self.literals.sort_unstable();
        self.literals.dedup();
        Ok(())
    }

    pub fn add_pattern(&mut self, pattern: &str) -> Result<()> {
        self.reserve(1, pattern.len())?;
        let compiled = PikeVM::builder()
            .thompson(
                thompson::Config::new()
                    .nfa_size_limit(Some(256 * 1024))
                    .which_captures(thompson::WhichCaptures::Implicit),
            )
            .build(pattern)
            .map_err(|_| Failure::Protocol("unsupported secret masking expression"))?;
        self.patterns.push(compiled);
        Ok(())
    }

    /// Diagnostics cannot return an inspection failure. Suppress the entire
    /// diagnostic if bounded masking fails; never return partially masked text.
    pub fn redact(&self, value: &str) -> String {
        self.masker(&|| Ok(()))
            .redact(value)
            .unwrap_or_else(|_| "***".to_owned())
    }

    pub fn masker<'a>(&'a self, checkpoint: &'a dyn Fn() -> Result<()>) -> Masker<'a> {
        Masker {
            redactor: self,
            remaining: MAX_WORK,
            checkpoint,
        }
    }
}

/// One budget spans every value, key, label and section in a preparation.
/// The checkpoint also makes cancellation testable without timing assumptions.
pub(crate) struct Masker<'a> {
    redactor: &'a Redactor,
    remaining: usize,
    checkpoint: &'a dyn Fn() -> Result<()>,
}

impl Masker<'_> {
    pub fn checkpoint(&self) -> Result<()> {
        (self.checkpoint)()
    }

    fn charge(&mut self, work: usize) -> Result<()> {
        self.checkpoint()?;
        self.remaining = self
            .remaining
            .checked_sub(work)
            .ok_or(Failure::Protocol("masking work exceeds preparation budget"))?;
        Ok(())
    }

    pub fn redact(&mut self, value: &str) -> Result<String> {
        // Charge scans and output even without any masks, before allocating.
        self.charge(
            value
                .len()
                .saturating_add(1)
                .saturating_mul(self.redactor.literals.len().saturating_add(3)),
        )?;
        // Mark original bytes, then emit once. Matches may overlap, but inserted
        // markers are never searched again and output grows by at most 3x.
        let mut masked = vec![false; value.len()];
        for pattern in &self.redactor.patterns {
            let mut cache = pattern.create_cache();
            let mut matches = pattern.find_iter(&mut cache, value);
            let mut start = 0;
            loop {
                // PikeVM has linear O(NFA size * haystack length) searches.
                // NFA memory includes states and transition tables, unlike source
                // length. Implicit captures keep per-state capture work constant.
                // Each iterator advance can search twice to skip an overlapping
                // empty match; both searches stay within the remaining suffix.
                let work = pattern
                    .get_nfa()
                    .memory_usage()
                    .saturating_mul(value.len().saturating_sub(start).saturating_add(1))
                    .saturating_mul(2);
                self.checkpoint()?;
                if work > MAX_SEARCH_WORK {
                    return Err(Failure::Protocol(
                        "masking search exceeds preparation budget",
                    ));
                }
                self.charge(work)?;
                let Some(found) = matches.next() else { break };
                masked[found.range()].fill(true);
                start = found.end();
            }
        }
        for secret in &self.redactor.literals {
            self.checkpoint()?;
            for (start, found) in value.match_indices(secret) {
                self.checkpoint()?;
                masked[start..start + found.len()].fill(true);
            }
        }
        let mut output = String::new();
        let mut start = 0;
        while start < value.len() {
            let hide = masked[start];
            let mut end = start + 1;
            while end < value.len() && masked[end] == hide {
                end += 1;
            }
            if hide {
                output.push_str("***");
            } else {
                // Every match boundary is a UTF-8 boundary supplied by str/Regex.
                output.push_str(&value[start..end]);
            }
            start = end;
        }
        self.checkpoint()?;
        Ok(output)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_overlapping_secrets_and_json_escaped_forms() {
        let masker = Redactor::new(["abcd", "abcdef", "quote\"newline\nsecret"]).expect("masker");
        assert_eq!(masker.redact("abcdef abcd"), "*** ***");
        assert!(
            !masker
                .redact(&serde_json::to_string("quote\"newline\nsecret").expect("JSON"))
                .contains("secret")
        );
    }

    #[test]
    fn empty_secrets_do_not_corrupt_output() {
        assert_eq!(
            Redactor::new([""]).expect("masker").redact("hello"),
            "hello"
        );
    }

    #[test]
    fn markers_and_overlapping_patterns_never_remask_inserted_text() {
        let mut masker = Redactor::new(["*\na", "*\nb", "**", "abc", "bcd"]).expect("masker");
        masker.add_pattern(r"\*").expect("pattern");
        assert_eq!(masker.redact("* abcd é"), "*** *** é");
    }

    proptest::proptest! {
        #[test]
        fn replacement_is_bounded_and_preserves_utf8(text in ".{0,256}", secret in ".{1,16}") {
            let masker = Redactor::new([secret.as_str(), "*", "**"]).expect("masker");
            proptest::prop_assert!(masker.redact(&text).len() <= text.len().saturating_mul(3));
        }
    }
}

#[cfg(test)]
mod budget_tests {
    use super::*;

    #[test]
    fn repeated_suffix_scans_exhaust_the_shared_budget() {
        let mut redactor = Redactor::default();
        redactor.add_pattern("a.*b|a").expect("pattern");
        let text = "a".repeat(512);
        let first_search = redactor.patterns[0].get_nfa().memory_usage() * (text.len() + 1) * 2;
        assert!(
            first_search < MAX_SEARCH_WORK,
            "first search must be admitted"
        );
        let error = redactor
            .masker(&|| Ok(()))
            .redact(&text)
            .expect_err("repeated scans");
        assert!(matches!(
            error,
            Failure::Protocol("masking work exceeds preparation budget")
        ));
        // A diagnostic must fail closed too, instead of exposing a partial result.
        assert_eq!(
            redactor.redact(&format!("private {}", "a".repeat(32_000))),
            "***"
        );
    }

    #[test]
    fn compiled_complexity_and_single_search_size_are_bounded() {
        for (pattern, text) in [("a{1000}", "a".repeat(100)), ("a.*b|a", "a".repeat(32_000))] {
            let mut redactor = Redactor::default();
            redactor.add_pattern(pattern).expect("bounded compilation");
            assert!(matches!(
                redactor.masker(&|| Ok(())).redact(&text),
                Err(Failure::Protocol(
                    "masking search exceeds preparation budget"
                ))
            ));
        }
    }

    #[test]
    fn bounded_search_preserves_greedy_alternation_empty_and_unicode_matches() {
        for (pattern, text, expected) in [
            ("a.*b|a", "aXXb aYb!", "***!"),
            ("a|ab", "ab", "***b"),
            ("a*", "aéaa!", "***é***!"),
            ("", "é!", "é!"),
            (r"(é+)|\bsecret\b", "éé secret!", "*** ***!"),
            ("^a|b$", "ab ab", "***b a***"),
        ] {
            let mut redactor = Redactor::default();
            redactor.add_pattern(pattern).expect("pattern");
            assert_eq!(
                redactor.masker(&|| Ok(())).redact(text).expect("mask"),
                expected,
                "{pattern}"
            );
            let original = regex::Regex::new(pattern).expect("original engine");
            let mut cache = redactor.patterns[0].create_cache();
            assert_eq!(
                redactor.patterns[0]
                    .find_iter(&mut cache, text)
                    .map(|m| m.range())
                    .collect::<Vec<_>>(),
                original
                    .find_iter(text)
                    .map(|m| m.range())
                    .collect::<Vec<_>>()
            );
        }
    }

    #[test]
    fn excessive_literal_and_regex_work_is_rejected() {
        assert!(Redactor::new(["x".repeat(MAX_METADATA + 1).as_str()]).is_err());
        let mut redactor = Redactor::default();
        assert!(redactor.add_pattern("a{100000000}").is_err());
        let redactor = Redactor::new(["secret"]).expect("masker");
        assert!(redactor.check_work(MAX_WORK).is_err());
    }
}
