//! Audit numeric value preservation after serde has validated the native document.
use crate::error::{Failure, Result};
use serde_json::Value;

// This is a lexical audit, not a JSON parser: serde consumes each complete string
// (including keys and escapes) or number. All other bytes belong to already
// validated structure or the literals true/false/null, which contain no digits.
pub(super) fn validate(validated: &str) -> Result<()> {
    if validated.len() > crate::inspection::MAX_OUTPUT {
        return Err(Failure::Protocol(
            "GitLab declaration exceeds 8 MiB preparation budget",
        ));
    }
    let mut remaining = validated.as_bytes();
    while let Some((&first, rest)) = remaining.split_first() {
        if first == b'"' || first == b'-' || first.is_ascii_digit() {
            let mut tokens = serde_json::Deserializer::from_slice(remaining).into_iter::<Value>();
            let token = tokens
                .next()
                .transpose()
                .map_err(|_| precision())?
                .ok_or_else(precision)?;
            let consumed = tokens.byte_offset();
            if let Value::Number(number) = token {
                let original = remaining.get(..consumed).ok_or_else(precision)?;
                let rendered = number.to_string();
                if decimal(original).ok_or_else(precision)?
                    != decimal(rendered.as_bytes()).ok_or_else(precision)?
                {
                    return Err(precision());
                }
            }
            remaining = remaining.get(consumed..).ok_or_else(precision)?;
        } else {
            remaining = rest;
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
struct Decimal {
    negative: bool,
    coefficient: Vec<u8>,
    exponent: i64,
}

// Normalize only serde-validated decimal spellings. Coefficient work/storage is
// linear in token length; an exponent never causes zero-padding or expansion.
// Exponents outside i64 are explicitly unsupported for nonzero coefficients.
fn decimal(spelling: &[u8]) -> Option<Decimal> {
    let negative = spelling.starts_with(b"-");
    let magnitude = spelling.strip_prefix(b"-").unwrap_or(spelling);
    let mut parts = magnitude.split(|b| *b == b'e' || *b == b'E');
    let mantissa = parts.next()?;
    let exponent = parts.next();
    let fractional = mantissa
        .iter()
        .position(|b| *b == b'.')
        .map_or(0, |dot| mantissa.len() - dot - 1);
    let digits: Vec<_> = mantissa.iter().copied().filter(|b| *b != b'.').collect();
    let Some(start) = digits.iter().position(|b| *b != b'0') else {
        // All signed zero spellings have the same numeric value, regardless of exponent.
        return Some(Decimal {
            negative: false,
            coefficient: Vec::new(),
            exponent: 0,
        });
    };
    let end = digits.iter().rposition(|b| *b != b'0')?.checked_add(1)?;
    let exponent = match exponent {
        Some(bytes) => std::str::from_utf8(bytes).ok()?.parse::<i64>().ok()?,
        None => 0,
    };
    Some(Decimal {
        negative,
        coefficient: digits.get(start..end)?.to_vec(),
        exponent: exponent
            .checked_sub(i64::try_from(fractional).ok()?)?
            .checked_add(i64::try_from(digits.len() - end).ok()?)?,
    })
}

fn precision() -> Failure {
    Failure::Protocol(
        "unsupported GitLab native numeric precision; rendering would change its value",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn audit(raw: &str) -> Result<()> {
        let _: Value = serde_json::from_str(raw).expect("valid JSON");
        validate(raw)
    }

    #[test]
    fn preserves_equivalent_zero_decimal_and_exponent_spellings() {
        for raw in [
            "0",
            "-0",
            "0.000",
            "-0e99999999999999999999999",
            "1.2500",
            "125e-2",
            "0.0125E+2",
            "1e+100",
            "1e-100",
            "1e00000000000000000000000000000000000000000000000000000000000000000001",
        ] {
            audit(raw).expect(raw);
        }
    }

    #[test]
    fn rejects_nonzero_underflow_and_precision_loss() {
        for raw in [
            "1e-999999999999999999999999",
            "1e-324",
            "18446744073709551617",
            "9281867110000000000e-10",
            "-9223372036854775809",
            "0.123456789012345678901",
        ] {
            assert!(audit(raw).is_err(), "{raw}");
        }
    }

    #[test]
    fn long_mantissas_and_exponents_do_not_expand() {
        audit(&format!("1.{}", "0".repeat(16_384))).expect("equivalent one");
        audit(&format!("0e{}", "9".repeat(16_384))).expect("zero");
        assert!(audit(&format!("1e-{}", "9".repeat(16_384))).is_err());
        assert!(audit(&format!("0.{}1", "0".repeat(16_384))).is_err());
    }

    proptest::proptest! {
        #[test]
        fn integer_spellings_and_equivalent_exponents_preserve_value(value in proptest::num::i32::ANY, zeros in 1_usize..16) {
            let spelling = value.to_string();
            proptest::prop_assert!(audit(&spelling).is_ok());
            let alternate = format!("{spelling}{}e-{zeros}", "0".repeat(zeros));
            proptest::prop_assert_eq!(decimal(spelling.as_bytes()), decimal(alternate.as_bytes()));
            // serde may round an alternate spelling during parsing. Such input
            // may be rejected; every accepted rendering must still equal the integer.
            if audit(&alternate).is_ok() {
                let rendered: Value = serde_json::from_str(&alternate).expect("number");
                proptest::prop_assert_eq!(rendered.as_f64(), Some(f64::from(value)));
            }
        }
    }
}
