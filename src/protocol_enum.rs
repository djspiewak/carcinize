//! GitHub's JSON serializers accept either enum numbers or case-insensitive names.
use serde::Deserialize;

#[derive(Deserialize)]
#[serde(untagged)]
pub(crate) enum ProtocolEnum {
    Code(u32),
    Name(String),
}

impl ProtocolEnum {
    pub fn code(&self, variants: &[(u32, &str)]) -> Option<u32> {
        variants.iter().find_map(|(code, name)| {
            let matches = match self {
                Self::Code(value) => value == code,
                Self::Name(value) => value.eq_ignore_ascii_case(name),
            };
            matches.then_some(*code)
        })
    }
}
