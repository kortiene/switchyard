use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::error::{Error, Result};

/// SHA-256 of RFC 8785 canonical JSON, prefixed `sha256:`.
///
/// S1 accepts null, booleans, strings, arrays, objects, and integers in the
/// `i64`/`u64` range. Non-integer numbers are rejected so this crate never
/// emits a non-conformant ES6 number serialization. S2 still has to freeze
/// shared Rust/TypeScript vectors for the full number grammar.
pub fn payload_hash(value: &Value) -> Result<String> {
    let canonical = canonicalize(value)?;
    Ok(format!("sha256:{}", sha256_hex(canonical.as_bytes())))
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    hex_encode(Sha256::digest(bytes).as_slice())
}

pub fn canonicalize(value: &Value) -> Result<String> {
    let mut out = String::new();
    write_value(&mut out, value)?;
    Ok(out)
}

fn write_value(out: &mut String, value: &Value) -> Result<()> {
    match value {
        Value::Null => out.push_str("null"),
        Value::Bool(true) => out.push_str("true"),
        Value::Bool(false) => out.push_str("false"),
        Value::Number(number) => write_number(out, number)?,
        Value::String(text) => write_string(out, text),
        Value::Array(items) => {
            out.push('[');
            for (index, item) in items.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_value(out, item)?;
            }
            out.push(']');
        }
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort_by(|left, right| left.encode_utf16().cmp(right.encode_utf16()));
            out.push('{');
            for (index, key) in keys.iter().enumerate() {
                if index > 0 {
                    out.push(',');
                }
                write_string(out, key);
                out.push(':');
                write_value(out, map.get(*key).expect("key came from this map"))?;
            }
            out.push('}');
        }
    }
    Ok(())
}

fn write_number(out: &mut String, number: &serde_json::Number) -> Result<()> {
    if let Some(value) = number.as_i64() {
        out.push_str(&value.to_string());
        return Ok(());
    }
    if let Some(value) = number.as_u64() {
        out.push_str(&value.to_string());
        return Ok(());
    }
    Err(Error::Canonical {
        detail: "non-integer numbers are rejected in S1 canonical JSON".into(),
    })
}

fn write_string(out: &mut String, text: &str) {
    out.push('"');
    for ch in text.chars() {
        match ch {
            '\u{0008}' => out.push_str("\\b"),
            '\u{0009}' => out.push_str("\\t"),
            '\u{000A}' => out.push_str("\\n"),
            '\u{000C}' => out.push_str("\\f"),
            '\u{000D}' => out.push_str("\\r"),
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            other if (other as u32) < 0x20 => {
                out.push_str(&format!("\\u{code:04x}", code = other as u32));
            }
            other => out.push(other),
        }
    }
    out.push('"');
}

fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut encoded = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        encoded.push(HEX[(byte >> 4) as usize] as char);
        encoded.push(HEX[(byte & 0x0f) as usize] as char);
    }
    encoded
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn object_keys_sort_and_integers_have_no_whitespace() {
        let value = json!({"b": 1, "a": 2, "nested": {"z": false, "m": null}});
        assert_eq!(
            canonicalize(&value).unwrap(),
            r#"{"a":2,"b":1,"nested":{"m":null,"z":false}}"#
        );
    }

    #[test]
    fn utf16_code_unit_order_differs_from_code_point_order() {
        let value = json!({ "\u{E000}": 1, "\u{10000}": 2 });
        let canonical = canonicalize(&value).unwrap();
        let earlier = canonical.find('\u{10000}').unwrap();
        let later = canonical.find('\u{E000}').unwrap();
        assert!(earlier < later, "{canonical}");
    }

    #[test]
    fn strings_escape_controls_and_keep_unicode_raw() {
        let value = json!("a\"b\\c\u{0001}\n€");
        assert_eq!(canonicalize(&value).unwrap(), "\"a\\\"b\\\\c\\u0001\\n€\"");
    }

    #[test]
    fn non_integer_numbers_are_rejected() {
        let value = json!(1.5);
        let error = canonicalize(&value).unwrap_err();
        assert_eq!(error.category(), "canonical_rejected");
    }

    #[test]
    fn prefixed_hash_matches_canonical_bytes() {
        let value = json!({"a": 1});
        assert_eq!(canonicalize(&value).unwrap(), r#"{"a":1}"#);
        assert_eq!(
            payload_hash(&value).unwrap(),
            "sha256:015abd7f5cc57a2dd94b7590f04ad8084273905ee33ec5cebeae62276a97f862"
        );
    }
}
