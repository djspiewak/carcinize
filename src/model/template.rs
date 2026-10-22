use super::DecodeError;
use serde::{Deserialize, Serialize};
use serde_json::Value;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
pub(crate) struct SourceLocation {
    pub file: Option<u32>,
    pub line: Option<u32>,
    #[serde(rename = "col")]
    pub column: Option<u32>,
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TemplateValue {
    #[serde(flatten)]
    pub kind: TemplateKind,
    pub source: SourceLocation,
}

#[derive(Clone, Debug, Serialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub(crate) enum TemplateKind {
    Null,
    Boolean(bool),
    Number(serde_json::Number),
    String(String),
    Expression(String),
    Insert(String),
    Sequence(Vec<TemplateValue>),
    Mapping(Vec<TemplateEntry>),
}

#[derive(Clone, Debug, Serialize)]
pub(crate) struct TemplateEntry {
    pub key: TemplateValue,
    pub value: TemplateValue,
}

pub(crate) fn decode_template(value: Value) -> Result<TemplateValue, DecodeError> {
    decode_at(value, 0)
}

fn decode_at(value: Value, depth: usize) -> Result<TemplateValue, DecodeError> {
    if depth > 64 {
        return Err(DecodeError::Malformed("template nesting exceeds 64"));
    }
    let mut source = SourceLocation::default();
    let kind = match value {
        Value::Null => TemplateKind::Null,
        Value::Bool(v) => TemplateKind::Boolean(v),
        Value::Number(v) => TemplateKind::Number(v),
        Value::String(v) => TemplateKind::String(v),
        Value::Object(mut obj) => {
            source = serde_json::from_value(Value::Object(obj.clone()))
                .map_err(|_| DecodeError::Malformed("template source location"))?;
            let tag = match obj.remove("type") {
                None => 0,
                Some(Value::Number(n)) => {
                    n.as_u64().ok_or(DecodeError::Malformed("template type"))?
                }
                _ => return Err(DecodeError::Malformed("template type")),
            };
            let mut take = |key| {
                obj.remove(key)
                    .ok_or(DecodeError::Malformed("template value"))
            };
            match tag {
                0 => TemplateKind::String(string(take("lit")?)?),
                1 => {
                    let values = match obj.remove("seq") {
                        None => vec![],
                        Some(Value::Array(v)) => v,
                        _ => return Err(DecodeError::Malformed("template sequence")),
                    };
                    TemplateKind::Sequence(
                        values
                            .into_iter()
                            .map(|v| decode_at(v, depth + 1))
                            .collect::<Result<_, _>>()?,
                    )
                }
                2 => {
                    let values = match obj.remove("map") {
                        None => vec![],
                        Some(Value::Array(v)) => v,
                        _ => return Err(DecodeError::Malformed("template mapping")),
                    };
                    let entries = values
                        .into_iter()
                        .map(|v| {
                            #[derive(Deserialize)]
                            struct Entry {
                                #[serde(alias = "Key")]
                                key: Value,
                                #[serde(alias = "Value")]
                                value: Value,
                            }
                            let e: Entry = serde_json::from_value(v)
                                .map_err(|_| DecodeError::Malformed("template mapping entry"))?;
                            Ok(TemplateEntry {
                                key: decode_at(e.key, depth + 1)?,
                                value: decode_at(e.value, depth + 1)?,
                            })
                        })
                        .collect::<Result<_, DecodeError>>()?;
                    TemplateKind::Mapping(entries)
                }
                3 => TemplateKind::Expression(string(take("expr")?)?),
                4 => TemplateKind::Insert(string(take("directive")?)?),
                5 => match take("bool")? {
                    Value::Bool(v) => TemplateKind::Boolean(v),
                    _ => return Err(DecodeError::Malformed("template boolean")),
                },
                6 => match take("num")? {
                    Value::Number(v) => TemplateKind::Number(v),
                    _ => return Err(DecodeError::Malformed("template number")),
                },
                7 => TemplateKind::Null,
                _ => return Err(DecodeError::Unsupported("template type")),
            }
        }
        Value::Array(_) => return Err(DecodeError::Malformed("template token")),
    };
    Ok(TemplateValue { kind, source })
}

fn string(value: Value) -> Result<String, DecodeError> {
    if let Value::String(v) = value {
        Ok(v)
    } else {
        Err(DecodeError::Malformed("template string"))
    }
}

impl TemplateValue {
    pub fn as_literal(&self) -> Option<&str> {
        if let TemplateKind::String(s) = &self.kind {
            Some(s)
        } else {
            None
        }
    }

    pub fn lookup(&self, key: &str) -> Result<Option<&Self>, DecodeError> {
        let TemplateKind::Mapping(entries) = &self.kind else {
            return Err(DecodeError::Malformed("script input mapping"));
        };
        let mut matching = entries.iter().filter(|e| e.key.as_literal() == Some(key));
        let first = matching.next().map(|e| &e.value);
        if matching.next().is_some() {
            return Err(DecodeError::Malformed("duplicate script input"));
        }
        Ok(first)
    }

    pub fn validate_script_keys(&self) -> Result<(), DecodeError> {
        let TemplateKind::Mapping(entries) = &self.kind else {
            return Err(DecodeError::Malformed("script input mapping"));
        };
        if entries.iter().any(|e| {
            !matches!(
                e.key.as_literal(),
                Some("script" | "shell" | "workingDirectory")
            )
        }) {
            return Err(DecodeError::Unsupported("script input"));
        }
        Ok(())
    }
}
