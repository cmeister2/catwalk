//! Just enough JSON to read a glTF document.
//!
//! glTF's structure is JSON and its payload is binary, so reading one needs a
//! parser. This is that parser and nothing more: the grammar as the standard
//! for JSON states it (objects, arrays, strings with their escapes, numbers,
//! the three literals) with no schema, no derive, and no dependency. It is
//! here rather than in a general place because the only thing that needs it is
//! the format that carries its own structure this way.
//!
//! Two deliberate limits, both stated rather than discovered. Numbers are read
//! as `f64`, which is what every quantity in a glTF document is used as, and a
//! caller wanting an index asks for one and is told if the value is not a
//! whole number. And duplicate object keys keep the last, which is what a
//! reader has to do with a document that says one thing twice.

use ogeom_core::FastMap;
use ogeom_core::{OgeomResult, ogeom_bail};

/// A parsed JSON value.
#[derive(Debug, Clone, PartialEq)]
pub enum Json {
    /// `null`.
    Null,
    /// `true` or `false`.
    Bool(bool),
    /// Any number, as the double every JSON number fits.
    Number(f64),
    /// A string, escapes resolved.
    Text(String),
    /// An array.
    Array(Vec<Json>),
    /// An object.
    Object(FastMap<String, Json>),
}

impl Json {
    /// The member of an object, or `None` for anything else.
    #[must_use]
    pub fn get(&self, key: &str) -> Option<&Self> {
        match self {
            Self::Object(map) => map.get(key),
            _ => None,
        }
    }

    /// The elements of an array, or an empty slice.
    #[must_use]
    pub fn items(&self) -> &[Self] {
        match self {
            Self::Array(items) => items,
            _ => &[],
        }
    }

    /// The number, or `None`.
    #[must_use]
    pub const fn number(&self) -> Option<f64> {
        match self {
            Self::Number(v) => Some(*v),
            _ => None,
        }
    }

    /// The string, or `None`.
    #[must_use]
    pub fn text(&self) -> Option<&str> {
        match self {
            Self::Text(s) => Some(s),
            _ => None,
        }
    }

    /// A non-negative whole number as an index.
    ///
    /// `None` where the value is absent, not a number, negative, or not
    /// whole; an index of `2.5` is a broken document, not a rounding.
    #[must_use]
    pub fn index(&self) -> Option<usize> {
        let v = self.number()?;
        if v < 0.0 || v.fract() != 0.0 || v > 9_007_199_254_740_992.0 {
            return None;
        }
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "checked whole and non-negative just above"
        )]
        Some(v as usize)
    }

    /// A member read as an index.
    #[must_use]
    pub fn index_at(&self, key: &str) -> Option<usize> {
        self.get(key)?.index()
    }
}

/// Parse a JSON document.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) with the byte
/// offset where the document stopped making sense.
pub fn parse(text: &str) -> OgeomResult<Json> {
    let bytes = text.as_bytes();
    let mut at = 0;
    let value = parse_value(bytes, &mut at, 0)?;
    skip_space(bytes, &mut at);
    if at != bytes.len() {
        ogeom_bail!(Construction, "trailing content at byte {at}");
    }
    Ok(value)
}

fn skip_space(bytes: &[u8], at: &mut usize) {
    while *at < bytes.len() && matches!(bytes[*at], b' ' | b'\t' | b'\n' | b'\r') {
        *at += 1;
    }
}

/// How deep objects and arrays may nest. A glTF document nests a handful
/// of levels; a document nesting thousands is hostile or broken, and
/// reading it by recursion would run the stack out.
const DEPTH: usize = 256;

fn parse_value(bytes: &[u8], at: &mut usize, depth: usize) -> OgeomResult<Json> {
    skip_space(bytes, at);
    let Some(&byte) = bytes.get(*at) else {
        ogeom_bail!(Construction, "the document ends where a value was expected");
    };
    if matches!(byte, b'{' | b'[') && depth >= DEPTH {
        ogeom_bail!(
            Construction,
            "the document nests deeper than {DEPTH} levels, at byte {at}",
            at = *at
        );
    }
    match byte {
        b'{' => parse_object(bytes, at, depth + 1),
        b'[' => parse_array(bytes, at, depth + 1),
        b'"' => Ok(Json::Text(parse_string(bytes, at)?)),
        b't' => literal(bytes, at, "true", Json::Bool(true)),
        b'f' => literal(bytes, at, "false", Json::Bool(false)),
        b'n' => literal(bytes, at, "null", Json::Null),
        _ => parse_number(bytes, at),
    }
}

fn literal(bytes: &[u8], at: &mut usize, word: &str, value: Json) -> OgeomResult<Json> {
    if bytes[*at..].starts_with(word.as_bytes()) {
        *at += word.len();
        return Ok(value);
    }
    ogeom_bail!(Construction, "expected `{word}` at byte {at}", at = *at);
}

fn parse_object(bytes: &[u8], at: &mut usize, depth: usize) -> OgeomResult<Json> {
    *at += 1;
    let mut map = FastMap::default();
    skip_space(bytes, at);
    if bytes.get(*at) == Some(&b'}') {
        *at += 1;
        return Ok(Json::Object(map));
    }
    loop {
        skip_space(bytes, at);
        if bytes.get(*at) != Some(&b'"') {
            ogeom_bail!(
                Construction,
                "an object's key is a string, at byte {at}",
                at = *at
            );
        }
        let key = parse_string(bytes, at)?;
        skip_space(bytes, at);
        if bytes.get(*at) != Some(&b':') {
            ogeom_bail!(Construction, "expected `:` at byte {at}", at = *at);
        }
        *at += 1;
        let value = parse_value(bytes, at, depth)?;
        map.insert(key, value);
        skip_space(bytes, at);
        match bytes.get(*at) {
            Some(&b',') => *at += 1,
            Some(&b'}') => {
                *at += 1;
                return Ok(Json::Object(map));
            }
            _ => ogeom_bail!(Construction, "expected `,` or `}}` at byte {at}", at = *at),
        }
    }
}

fn parse_array(bytes: &[u8], at: &mut usize, depth: usize) -> OgeomResult<Json> {
    *at += 1;
    let mut items = Vec::new();
    skip_space(bytes, at);
    if bytes.get(*at) == Some(&b']') {
        *at += 1;
        return Ok(Json::Array(items));
    }
    loop {
        items.push(parse_value(bytes, at, depth)?);
        skip_space(bytes, at);
        match bytes.get(*at) {
            Some(&b',') => *at += 1,
            Some(&b']') => {
                *at += 1;
                return Ok(Json::Array(items));
            }
            _ => ogeom_bail!(Construction, "expected `,` or `]` at byte {at}", at = *at),
        }
    }
}

fn parse_string(bytes: &[u8], at: &mut usize) -> OgeomResult<String> {
    *at += 1;
    let mut out = String::new();
    loop {
        let Some(&byte) = bytes.get(*at) else {
            ogeom_bail!(Construction, "a string runs off the end of the document");
        };
        *at += 1;
        match byte {
            b'"' => return Ok(out),
            b'\\' => {
                let Some(&escape) = bytes.get(*at) else {
                    ogeom_bail!(Construction, "an escape runs off the end of the document");
                };
                *at += 1;
                match escape {
                    b'"' => out.push('"'),
                    b'\\' => out.push('\\'),
                    b'/' => out.push('/'),
                    b'b' => out.push('\u{8}'),
                    b'f' => out.push('\u{c}'),
                    b'n' => out.push('\n'),
                    b'r' => out.push('\r'),
                    b't' => out.push('\t'),
                    b'u' => out.push(parse_escape(bytes, at)?),
                    other => ogeom_bail!(Construction, "unknown escape `\\{}`", other as char),
                }
            }
            // Anything else is copied through as UTF-8, which the document
            // already is: the slice is walked byte by byte, so a multi-byte
            // character arrives one continuation at a time and rebuilds here.
            _ => {
                let start = *at - 1;
                let width = utf8_width(byte);
                if start + width > bytes.len() {
                    ogeom_bail!(Construction, "a character runs off the end of the document");
                }
                let Ok(text) = core::str::from_utf8(&bytes[start..start + width]) else {
                    ogeom_bail!(Construction, "the document is not UTF-8 at byte {start}");
                };
                out.push_str(text);
                *at = start + width;
            }
        }
    }
}

/// How many bytes a UTF-8 character starting with this byte occupies.
const fn utf8_width(lead: u8) -> usize {
    match lead {
        0x00..=0x7F => 1,
        0xC0..=0xDF => 2,
        0xE0..=0xEF => 3,
        _ => 4,
    }
}

/// A `\uXXXX` escape, surrogate pairs included: a character outside the
/// basic plane is written as two of them, and one alone is not a character.
fn parse_escape(bytes: &[u8], at: &mut usize) -> OgeomResult<char> {
    let first = hex4(bytes, at)?;
    if (0xD800..0xDC00).contains(&first) {
        if bytes.get(*at) != Some(&b'\\') || bytes.get(*at + 1) != Some(&b'u') {
            ogeom_bail!(Construction, "a leading surrogate with no trailing one");
        }
        *at += 2;
        let second = hex4(bytes, at)?;
        if !(0xDC00..0xE000).contains(&second) {
            ogeom_bail!(Construction, "a leading surrogate followed by {second:#x}");
        }
        let combined = 0x1_0000 + ((first - 0xD800) << 10) + (second - 0xDC00);
        let Some(c) = char::from_u32(combined) else {
            ogeom_bail!(Construction, "the surrogate pair names no character");
        };
        return Ok(c);
    }
    let Some(c) = char::from_u32(first) else {
        ogeom_bail!(Construction, "the escape {first:#x} names no character");
    };
    Ok(c)
}

fn hex4(bytes: &[u8], at: &mut usize) -> OgeomResult<u32> {
    let Some(slice) = bytes.get(*at..*at + 4) else {
        ogeom_bail!(Construction, "a `\\u` escape wants four hex digits");
    };
    let Ok(text) = core::str::from_utf8(slice) else {
        ogeom_bail!(Construction, "a `\\u` escape wants four hex digits");
    };
    let Ok(value) = u32::from_str_radix(text, 16) else {
        ogeom_bail!(Construction, "`{text}` is not four hex digits");
    };
    *at += 4;
    Ok(value)
}

fn parse_number(bytes: &[u8], at: &mut usize) -> OgeomResult<Json> {
    let start = *at;
    if bytes.get(*at) == Some(&b'-') {
        *at += 1;
    }
    while matches!(bytes.get(*at), Some(b'0'..=b'9')) {
        *at += 1;
    }
    if bytes.get(*at) == Some(&b'.') {
        *at += 1;
        while matches!(bytes.get(*at), Some(b'0'..=b'9')) {
            *at += 1;
        }
    }
    if matches!(bytes.get(*at), Some(b'e' | b'E')) {
        *at += 1;
        if matches!(bytes.get(*at), Some(b'+' | b'-')) {
            *at += 1;
        }
        while matches!(bytes.get(*at), Some(b'0'..=b'9')) {
            *at += 1;
        }
    }
    let Ok(text) = core::str::from_utf8(&bytes[start..*at]) else {
        ogeom_bail!(Construction, "a number is not UTF-8 at byte {start}");
    };
    let Ok(value) = text.parse::<f64>() else {
        ogeom_bail!(Construction, "`{text}` is not a number, at byte {start}");
    };
    Ok(Json::Number(value))
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_grammar_round_trips_the_shapes_a_gltf_document_uses() {
        let document = r#"
        {
          "asset": {"version": "2.0", "generator": "something \"quoted\""},
          "scene": 0,
          "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": 3, "type": "VEC3",
             "min": [-1, -2.5, 0], "max": [1e1, 2.5E-1, 0]},
            {"componentType": 5121, "count": 0, "type": "SCALAR", "normalized": true}
          ],
          "nothing": null,
          "empty": {},
          "none": []
        }"#;
        let json = parse(document).unwrap();
        assert_eq!(json.index_at("scene"), Some(0));
        assert_eq!(
            json.get("asset").unwrap().get("generator").unwrap().text(),
            Some("something \"quoted\"")
        );
        let accessors = json.get("accessors").unwrap().items();
        assert_eq!(accessors.len(), 2);
        assert_eq!(accessors[0].index_at("componentType"), Some(5126));
        assert_eq!(
            accessors[0].get("max").unwrap().items()[0].number(),
            Some(10.0)
        );
        assert_eq!(
            accessors[0].get("min").unwrap().items()[1].number(),
            Some(-2.5)
        );
        assert_eq!(accessors[1].get("normalized"), Some(&Json::Bool(true)));
        assert_eq!(json.get("nothing"), Some(&Json::Null));
        assert!(json.get("none").unwrap().items().is_empty());
    }

    #[test]
    fn escapes_and_characters_outside_the_basic_plane_survive() {
        let json = parse(r#"{"n":"aé\n\t😀b","x":"café"}"#).unwrap();
        assert_eq!(json.get("n").unwrap().text(), Some("aé\n\t😀b"));
        assert_eq!(json.get("x").unwrap().text(), Some("café"));
    }

    #[test]
    fn an_index_that_is_not_a_whole_number_is_not_an_index() {
        let json = parse(r#"{"a": 2.5, "b": -1, "c": 7}"#).unwrap();
        assert_eq!(json.index_at("a"), None);
        assert_eq!(json.index_at("b"), None);
        assert_eq!(json.index_at("c"), Some(7));
    }

    #[test]
    fn broken_documents_are_refused_rather_than_guessed_at() {
        for broken in [
            "{",
            "{\"a\"}",
            "{\"a\":}",
            "[1,]",
            "[1 2]",
            "{\"a\":1} trailing",
            "\"unterminated",
            r#""\q""#,
            r#""\ud83d""#,
        ] {
            assert!(parse(broken).is_err(), "`{broken}` should not parse");
        }
    }
}
