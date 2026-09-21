//! A JSON value parser for CLI end-to-end tests.
//!
//! RFC 147 §2e: lifted here verbatim from `rfc146_listing_json.rs`, which had itself copied it from
//! `rfc140_status_json_and_enumeration.rs` -- two copies already, and this round would have made a
//! third. prikk's test crates have no third-party dependencies (RFC 118 §10 prerequisite 4), so
//! there is no `serde_json` to lean on; what there can be is *one* parser. The two existing copies
//! are left where they are, on `support/mod.rs`'s own precedent: this is the point past which no one
//! should copy-paste it again, not a retrofit.

#![allow(clippy::expect_used, clippy::indexing_slicing, clippy::unwrap_used)]

use std::collections::BTreeMap;
use std::iter::Peekable;
use std::str::Chars;

#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Null,
    Bool(bool),
    Number(String),
    String(String),
    Array(Vec<Value>),
    Object(BTreeMap<String, Value>),
}

impl Value {
    pub(crate) fn get(&self, key: &str) -> &Value {
        match self {
            Value::Object(map) => map.get(key).unwrap_or_else(|| {
                panic!("missing key {key:?} in object with keys {:?}", map.keys())
            }),
            other => panic!("expected an object to look up {key:?}, got {other:?}"),
        }
    }

    pub(crate) fn as_array(&self) -> &[Value] {
        match self {
            Value::Array(items) => items,
            other => panic!("expected an array, got {other:?}"),
        }
    }

    pub(crate) fn as_str(&self) -> &str {
        match self {
            Value::String(text) => text,
            other => panic!("expected a string, got {other:?}"),
        }
    }

    pub(crate) fn as_bool(&self) -> bool {
        match self {
            Value::Bool(value) => *value,
            other => panic!("expected a bool, got {other:?}"),
        }
    }

    pub(crate) fn is_null(&self) -> bool {
        matches!(self, Value::Null)
    }
}

pub(crate) fn parse(input: &str) -> Value {
    let mut chars = input.trim().chars().peekable();
    let value = parse_value(&mut chars);
    skip_ws(&mut chars);
    assert!(
        chars.next().is_none(),
        "trailing content after the top-level JSON value: {input}"
    );
    value
}

fn skip_ws(chars: &mut Peekable<Chars<'_>>) {
    while matches!(chars.peek(), Some(c) if c.is_whitespace()) {
        chars.next();
    }
}

fn parse_value(chars: &mut Peekable<Chars<'_>>) -> Value {
    skip_ws(chars);
    match chars.peek().copied() {
        Some('{') => parse_object(chars),
        Some('[') => parse_array(chars),
        Some('"') => Value::String(parse_string(chars)),
        Some('t') => {
            parse_literal(chars, "true");
            Value::Bool(true)
        }
        Some('f') => {
            parse_literal(chars, "false");
            Value::Bool(false)
        }
        Some('n') => {
            parse_literal(chars, "null");
            Value::Null
        }
        Some(c) if c == '-' || c.is_ascii_digit() => Value::Number(parse_number(chars)),
        other => panic!("unexpected JSON token starting with {other:?}"),
    }
}

fn parse_literal(chars: &mut Peekable<Chars<'_>>, literal: &str) {
    for expected in literal.chars() {
        assert_eq!(chars.next(), Some(expected), "expected literal {literal}");
    }
}

/// Decode a JSON string, **escapes included**. This used to drop the backslash and keep the next character,
/// so `\n` came back as the letter `n` and any text with a newline in it was silently corrupted -- which
/// nothing noticed until `prikk diff` put whole unified hunks in a JSON string and the round-trip control
/// had to apply them. `\uXXXX` is decoded too, because the binary writes every other control character below
/// U+0020 that way (`escape_json_string`); surrogate pairs are decoded as the format allows, though the binary
/// never emits one.
fn parse_string(chars: &mut Peekable<Chars<'_>>) -> String {
    assert_eq!(chars.next(), Some('"'), "expected opening quote");
    let mut value = String::new();
    loop {
        match chars.next() {
            Some('"') => break,
            Some('\\') => match chars.next().expect("dangling escape at end of string") {
                'n' => value.push('\n'),
                'r' => value.push('\r'),
                't' => value.push('\t'),
                'b' => value.push('\u{8}'),
                'f' => value.push('\u{c}'),
                'u' => {
                    let high = parse_hex4(chars);
                    let code = if (0xD800..0xDC00).contains(&high) {
                        assert_eq!(chars.next(), Some('\\'), "a high surrogate needs a low one");
                        assert_eq!(chars.next(), Some('u'), "a high surrogate needs a low one");
                        let low = parse_hex4(chars);
                        0x10000 + ((high - 0xD800) << 10) + (low - 0xDC00)
                    } else {
                        high
                    };
                    value.push(char::from_u32(code).expect("a valid Unicode scalar value"));
                }
                // `\"`, `\\`, `\/`: the character itself.
                other => value.push(other),
            },
            Some(other) => value.push(other),
            None => panic!("unterminated JSON string"),
        }
    }
    value
}

fn parse_hex4(chars: &mut Peekable<Chars<'_>>) -> u32 {
    let digits: String = (0..4)
        .map(|_| chars.next().expect("a \\u escape needs four hex digits"))
        .collect();
    u32::from_str_radix(&digits, 16).expect("four hex digits")
}

fn parse_number(chars: &mut Peekable<Chars<'_>>) -> String {
    let mut text = String::new();
    if chars.peek() == Some(&'-') {
        text.push(chars.next().unwrap());
    }
    while matches!(chars.peek(), Some(c) if c.is_ascii_digit() || *c == '.') {
        text.push(chars.next().unwrap());
    }
    text
}

fn parse_object(chars: &mut Peekable<Chars<'_>>) -> Value {
    assert_eq!(chars.next(), Some('{'));
    let mut map = BTreeMap::new();
    skip_ws(chars);
    if chars.peek() == Some(&'}') {
        chars.next();
        return Value::Object(map);
    }
    loop {
        skip_ws(chars);
        let key = parse_string(chars);
        skip_ws(chars);
        assert_eq!(chars.next(), Some(':'), "expected ':' after object key");
        let value = parse_value(chars);
        map.insert(key, value);
        skip_ws(chars);
        match chars.next() {
            Some(',') => continue,
            Some('}') => break,
            other => panic!("expected ',' or '}}' in object, got {other:?}"),
        }
    }
    Value::Object(map)
}

fn parse_array(chars: &mut Peekable<Chars<'_>>) -> Value {
    assert_eq!(chars.next(), Some('['));
    let mut items = Vec::new();
    skip_ws(chars);
    if chars.peek() == Some(&']') {
        chars.next();
        return Value::Array(items);
    }
    loop {
        items.push(parse_value(chars));
        skip_ws(chars);
        match chars.next() {
            Some(',') => continue,
            Some(']') => break,
            other => panic!("expected ',' or ']' in array, got {other:?}"),
        }
    }
    Value::Array(items)
}
