//! The ISO 10303-21 exchange structure, parsed but not yet interpreted.
//!
//! Part 21 is a syntax, not a schema: `#3 = CIRCLE('', #2, 5.0);` says an
//! instance exists with a keyword and arguments, and what a `CIRCLE` *means*
//! is the reader's business, not the parser's. This module turns the text
//! into a map from instance number to typed argument trees and nothing more,
//! which is what lets the reader say precisely which entities it understood
//! and which it deliberately walked past.

use ogeom_core::{OgeomResult, ogeom_bail};

/// One argument of an entity instance.
#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    /// `$`: no value.
    Null,
    /// `*`: value derivable from the schema, not stated.
    Derived,
    /// `#n`: a reference to another instance.
    Ref(u64),
    /// An integer literal.
    Int(i64),
    /// A real literal.
    Real(f64),
    /// A string literal, with Part 21's quote doubling undone.
    Str(String),
    /// `.NAME.`: an enumeration value, without its dots.
    Enum(String),
    /// A parenthesised list.
    List(Vec<Arg>),
    /// `KEYWORD(...)` in argument position: a typed (select) value: the
    /// keyword and its arguments, boxed, so the rare typed value does not
    /// size every argument of the file.
    Typed(Box<(String, Vec<Arg>)>),
}

impl Arg {
    /// The reference this argument carries, if it is one.
    pub fn reference(&self) -> Option<u64> {
        match self {
            Self::Ref(n) => Some(*n),
            _ => None,
        }
    }

    /// The number this argument carries, integer or real.
    pub fn number(&self) -> Option<f64> {
        match self {
            Self::Int(n) =>
            {
                #[allow(clippy::cast_precision_loss)]
                Some(*n as f64)
            }
            Self::Real(x) => Some(*x),
            _ => None,
        }
    }

    /// The list this argument carries, if it is one.
    pub fn list(&self) -> Option<&[Arg]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }

    /// Whether this is the enumeration value `name`.
    pub fn is_enum(&self, name: &str) -> bool {
        matches!(self, Self::Enum(e) if e == name)
    }
}

/// One instance: usually one keyword with arguments, several for a complex
/// (multi-leaf) instance like `#1 = (A(...) B(...));`.
#[derive(Debug, Clone)]
pub struct Instance {
    /// The parts, in file order.
    pub parts: Vec<(String, Vec<Arg>)>,
}

impl Instance {
    /// The arguments of the part with this keyword, if present.
    pub fn part(&self, keyword: &str) -> Option<&[Arg]> {
        self.parts
            .iter()
            .find(|(k, _)| k == keyword)
            .map(|(_, a)| a.as_slice())
    }

    /// The single keyword of a simple instance.
    pub fn keyword(&self) -> &str {
        self.parts.first().map_or("", |(k, _)| k.as_str())
    }

    /// Every part of the instance: one for a simple instance, several for a
    /// complex one, in file order.
    pub fn parts(&self) -> impl Iterator<Item = (&str, &[Arg])> {
        self.parts.iter().map(|(k, a)| (k.as_str(), a.as_slice()))
    }
}

/// A parsed exchange file: the data section as a graph, the header kept as
/// raw instances for whoever wants the schema name or the file's own record
/// of itself.
#[derive(Debug)]
pub struct Exchange {
    /// Header entries, in order.
    pub header: Vec<(String, Vec<Arg>)>,
    /// The data section, by instance number.
    #[allow(
        clippy::disallowed_types,
        reason = "public field; a std map keeps the API"
    )]
    pub data: std::collections::HashMap<u64, Instance>,
}

/// Parse a Part 21 exchange file.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) on malformed
/// syntax, with the byte offset where reading stopped making sense.
pub fn parse(text: &str) -> OgeomResult<Exchange> {
    let (header, instances) = parse_instances(text)?;
    Ok(Exchange {
        header,
        data: instances.into_iter().collect(),
    })
}

/// The data section by instance number, as the reader looks it up.
///
/// Instance numbers are nearly always dense, numbered from one in file
/// order, so a slot per number answers a lookup with an index. A file whose
/// numbers run far past its count (more than four per instance, plus a
/// margin) gets a map instead.
#[derive(Debug)]
pub(crate) enum Instances {
    /// A slot per instance number up to the largest.
    Dense(Vec<Option<Instance>>),
    /// The instances of a file whose numbers are sparse.
    Sparse(ogeom_core::FastMap<u64, Instance>),
}

impl Instances {
    /// The table for these instances, a later number winning over an
    /// earlier one, as a repeated definition in the file does.
    fn new(list: Vec<(u64, Instance)>) -> Self {
        let top = list.iter().map(|(id, _)| *id).max().unwrap_or(0);
        match usize::try_from(top) {
            Ok(top) if top <= list.len().saturating_mul(4).saturating_add(1024) => {
                let mut slots: Vec<Option<Instance>> = Vec::new();
                slots.resize_with(top + 1, || None);
                for (id, instance) in list {
                    // In range: no number exceeds `top`.
                    if let Some(slot) = usize::try_from(id).ok().and_then(|i| slots.get_mut(i)) {
                        *slot = Some(instance);
                    }
                }
                Self::Dense(slots)
            }
            _ => Self::Sparse(list.into_iter().collect()),
        }
    }

    /// The instance numbered `id`.
    pub(crate) fn get(&self, id: u64) -> Option<&Instance> {
        match self {
            Self::Dense(slots) => usize::try_from(id)
                .ok()
                .and_then(|i| slots.get(i))
                .and_then(Option::as_ref),
            Self::Sparse(map) => map.get(&id),
        }
    }

    /// Whether an instance is numbered `id`.
    pub(crate) fn contains(&self, id: u64) -> bool {
        self.get(id).is_some()
    }

    /// Every instance with its number: in number order when dense.
    pub(crate) fn iter(&self) -> impl Iterator<Item = (u64, &Instance)> {
        let (dense, sparse) = match self {
            Self::Dense(slots) => (Some(slots), None),
            Self::Sparse(map) => (None, Some(map)),
        };
        let dense = dense.into_iter().flat_map(|slots| {
            (0_u64..)
                .zip(slots)
                .filter_map(|(id, slot)| Some((id, slot.as_ref()?)))
        });
        let sparse = sparse
            .into_iter()
            .flat_map(|map| map.iter().map(|(id, instance)| (*id, instance)));
        dense.chain(sparse)
    }

    /// Every instance.
    pub(crate) fn values(&self) -> impl Iterator<Item = &Instance> {
        self.iter().map(|(_, instance)| instance)
    }

    fn into_iter(self) -> Box<dyn Iterator<Item = (u64, Instance)>> {
        match self {
            Self::Dense(slots) => Box::new(
                (0_u64..)
                    .zip(slots)
                    .filter_map(|(id, slot)| Some((id, slot?))),
            ),
            Self::Sparse(map) => Box::new(map.into_iter()),
        }
    }
}

/// Header entries, each a keyword and its arguments, in order.
pub(crate) type Header = Vec<(String, Vec<Arg>)>;

/// The header entries and the data section of a Part 21 exchange file.
///
/// # Errors
///
/// As [`parse`].
pub(crate) fn parse_instances(text: &str) -> OgeomResult<(Header, Instances)> {
    let mut p = Parser {
        bytes: text.as_bytes(),
        at: 0,
        depth: 0,
    };
    p.skip_noise();
    p.expect_keyword("ISO-10303-21")?;
    p.expect(b';')?;

    p.expect_keyword("HEADER")?;
    p.expect(b';')?;
    let mut header = Vec::new();
    loop {
        p.skip_noise();
        if p.peek_keyword("ENDSEC") {
            p.expect_keyword("ENDSEC")?;
            p.expect(b';')?;
            break;
        }
        let keyword = p.keyword()?;
        let args = p.arguments()?;
        p.expect(b';')?;
        header.push((keyword, args));
    }

    p.expect_keyword("DATA")?;
    p.expect(b';')?;
    // Sized up front: an instance averages well under a hundred bytes, so
    // this over-reserves a little rather than growing a half-million-entry
    // list several times on the way up.
    let mut data = Vec::with_capacity(text.len() / 96);
    loop {
        p.skip_noise();
        if p.peek_keyword("ENDSEC") {
            p.expect_keyword("ENDSEC")?;
            p.expect(b';')?;
            break;
        }
        p.expect(b'#')?;
        let id = p.id()?;
        p.expect(b'=')?;
        p.skip_noise();
        let parts = if p.peek(b'(') {
            // A complex instance: parenthesised sequence of parts.
            p.expect(b'(')?;
            let mut parts = Vec::new();
            loop {
                p.skip_noise();
                if p.peek(b')') {
                    p.expect(b')')?;
                    break;
                }
                let keyword = p.keyword()?;
                let args = p.arguments()?;
                parts.push((keyword, args));
            }
            parts
        } else {
            let keyword = p.keyword()?;
            let args = p.arguments()?;
            vec![(keyword, args)]
        };
        p.expect(b';')?;
        data.push((id, Instance { parts }));
    }

    p.expect_keyword("END-ISO-10303-21")?;
    Ok((header, Instances::new(data)))
}

struct Parser<'a> {
    bytes: &'a [u8],
    at: usize,
    /// How many argument lists are open.
    depth: u32,
}

/// How deep argument lists may nest. Real files nest a few levels (a list
/// of lists of control points); past this the file is hostile, and the
/// recursive descent would otherwise run out of stack.
const MOST_NESTING: u32 = 256;

impl Parser<'_> {
    fn skip_noise(&mut self) {
        loop {
            while self.at < self.bytes.len() && self.bytes[self.at].is_ascii_whitespace() {
                self.at += 1;
            }
            if self.at + 1 < self.bytes.len() && &self.bytes[self.at..self.at + 2] == b"/*" {
                self.at += 2;
                while self.at + 1 < self.bytes.len() && &self.bytes[self.at..self.at + 2] != b"*/" {
                    self.at += 1;
                }
                self.at = (self.at + 2).min(self.bytes.len());
                continue;
            }
            break;
        }
    }

    fn peek(&mut self, byte: u8) -> bool {
        self.skip_noise();
        self.bytes.get(self.at) == Some(&byte)
    }

    fn expect(&mut self, byte: u8) -> OgeomResult<()> {
        self.skip_noise();
        if self.bytes.get(self.at) == Some(&byte) {
            self.at += 1;
            return Ok(());
        }
        ogeom_bail!(
            Construction,
            "expected '{}' at byte {} of the exchange file",
            char::from(byte),
            self.at
        );
    }

    fn peek_keyword(&mut self, word: &str) -> bool {
        self.skip_noise();
        let end = self.at + word.len();
        end <= self.bytes.len()
            && &self.bytes[self.at..end] == word.as_bytes()
            && self
                .bytes
                .get(end)
                .is_none_or(|b| !b.is_ascii_alphanumeric() && *b != b'_' && *b != b'-')
    }

    fn expect_keyword(&mut self, word: &str) -> OgeomResult<()> {
        if self.peek_keyword(word) {
            self.at += word.len();
            return Ok(());
        }
        ogeom_bail!(
            Construction,
            "expected '{word}' at byte {} of the exchange file",
            self.at
        );
    }

    fn keyword(&mut self) -> OgeomResult<String> {
        self.skip_noise();
        let start = self.at;
        while self
            .bytes
            .get(self.at)
            .is_some_and(|b| b.is_ascii_alphanumeric() || *b == b'_')
        {
            self.at += 1;
        }
        if self.at == start {
            ogeom_bail!(
                Construction,
                "expected a keyword at byte {} of the exchange file",
                start
            );
        }
        Ok(String::from_utf8_lossy(&self.bytes[start..self.at]).into_owned())
    }

    fn integer(&mut self) -> OgeomResult<i64> {
        self.skip_noise();
        let start = self.at;
        if self.bytes.get(self.at) == Some(&b'-') || self.bytes.get(self.at) == Some(&b'+') {
            self.at += 1;
        }
        while self.bytes.get(self.at).is_some_and(u8::is_ascii_digit) {
            self.at += 1;
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at]).unwrap_or("");
        text.parse().map_err(|_| {
            ogeom_core::ogeom_err!(
                Construction,
                "expected an integer at byte {start} of the exchange file"
            )
        })
    }

    /// An instance id: a non-negative integer.
    fn id(&mut self) -> OgeomResult<u64> {
        let at = self.at;
        let id = self.integer()?;
        u64::try_from(id).map_err(|_| {
            ogeom_core::ogeom_err!(
                Construction,
                "a negative instance id at byte {at} of the exchange file"
            )
        })
    }

    fn arguments(&mut self) -> OgeomResult<Vec<Arg>> {
        if self.depth >= MOST_NESTING {
            ogeom_bail!(
                Construction,
                "argument lists nest more than {MOST_NESTING} deep at byte {} of the \
                 exchange file",
                self.at
            );
        }
        self.depth += 1;
        let listed = self.argument_list();
        self.depth -= 1;
        listed
    }

    fn argument_list(&mut self) -> OgeomResult<Vec<Arg>> {
        self.expect(b'(')?;
        let mut out = Vec::with_capacity(4);
        loop {
            self.skip_noise();
            if self.peek(b')') {
                self.expect(b')')?;
                break;
            }
            out.push(self.argument()?);
            self.skip_noise();
            if self.peek(b',') {
                self.expect(b',')?;
            }
        }
        Ok(out)
    }

    fn argument(&mut self) -> OgeomResult<Arg> {
        self.skip_noise();
        let Some(&byte) = self.bytes.get(self.at) else {
            ogeom_bail!(Construction, "the exchange file ends inside an argument");
        };
        match byte {
            b'$' => {
                self.at += 1;
                Ok(Arg::Null)
            }
            b'*' => {
                self.at += 1;
                Ok(Arg::Derived)
            }
            b'#' => {
                self.at += 1;
                Ok(Arg::Ref(self.id()?))
            }
            b'(' => Ok(Arg::List(self.arguments()?)),
            b'\'' => self.string(),
            b'.' => {
                self.at += 1;
                let word = self.keyword()?;
                self.expect(b'.')?;
                Ok(Arg::Enum(word))
            }
            b'-' | b'+' | b'0'..=b'9' => self.number(),
            _ if byte.is_ascii_alphabetic() || byte == b'_' => {
                let keyword = self.keyword()?;
                let args = self.arguments()?;
                Ok(Arg::Typed(Box::new((keyword, args))))
            }
            _ => ogeom_bail!(
                Construction,
                "unexpected '{}' at byte {} of the exchange file",
                char::from(byte),
                self.at
            ),
        }
    }

    fn string(&mut self) -> OgeomResult<Arg> {
        self.expect(b'\'')?;
        let mut raw: Vec<u8> = Vec::new();
        loop {
            // The common run, everything up to the next quote, in one
            // extend rather than a byte at a time.
            let start = self.at;
            while self.bytes.get(self.at).is_some_and(|b| *b != b'\'') {
                self.at += 1;
            }
            raw.extend_from_slice(&self.bytes[start..self.at]);
            match self.bytes.get(self.at) {
                None => ogeom_bail!(Construction, "the exchange file ends inside a string"),
                Some(_) => {
                    if self.bytes.get(self.at + 1) == Some(&b'\'') {
                        raw.push(b'\'');
                        self.at += 2;
                    } else {
                        self.at += 1;
                        break;
                    }
                }
            }
        }
        // Bytes past ASCII are not the standard's, but exporters write
        // them: UTF-8 where they are valid UTF-8, Latin-1 otherwise.
        let text = match std::str::from_utf8(&raw) {
            Ok(text) => text.to_owned(),
            Err(_) => raw.iter().map(|b| char::from(*b)).collect(),
        };
        Ok(Arg::Str(decode_escapes(&text)))
    }

    fn number(&mut self) -> OgeomResult<Arg> {
        let start = self.at;
        if matches!(self.bytes.get(self.at), Some(b'-' | b'+')) {
            self.at += 1;
        }
        let mut real = false;
        while let Some(&b) = self.bytes.get(self.at) {
            match b {
                b'0'..=b'9' => self.at += 1,
                b'.' => {
                    // A dot starts a real, unless it starts an enumeration
                    // hard against the number, which no real file does.
                    real = true;
                    self.at += 1;
                }
                b'E' | b'e' => {
                    real = true;
                    self.at += 1;
                    if matches!(self.bytes.get(self.at), Some(b'-' | b'+')) {
                        self.at += 1;
                    }
                }
                _ => break,
            }
        }
        let text = std::str::from_utf8(&self.bytes[start..self.at]).unwrap_or("");
        if real {
            text.parse().map(Arg::Real).map_err(|_| {
                ogeom_core::ogeom_err!(
                    Construction,
                    "unreadable real at byte {start} of the exchange file"
                )
            })
        } else {
            text.parse().map(Arg::Int).map_err(|_| {
                ogeom_core::ogeom_err!(
                    Construction,
                    "unreadable integer at byte {start} of the exchange file"
                )
            })
        }
    }
}

/// A STEP string's escapes read: `\\` a backslash, `\X\hh` the Latin-1
/// character `hh`, `\X2\...\X0\` UTF-16 and `\X4\...\X0\` UTF-32 code
/// units in hex, `\S\c` the character `c` in the upper half of the code
/// page. A code-page directive (`\P?\`) is dropped. Anything that does not
/// parse as an escape is kept as written.
pub(crate) fn decode_escapes(text: &str) -> String {
    if !text.contains('\\') {
        return text.to_owned();
    }
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    let hex = |slice: &[char]| -> Option<u32> {
        let s: String = slice.iter().collect();
        u32::from_str_radix(&s, 16).ok()
    };
    while i < chars.len() {
        let rest = &chars[i..];
        let starts = |p: &str| {
            let p: Vec<char> = p.chars().collect();
            rest.len() >= p.len() && rest[..p.len()] == p[..]
        };
        if starts("\\\\") {
            out.push('\\');
            i += 2;
        } else if starts("\\X2\\") || starts("\\X4\\") {
            let width = if starts("\\X2\\") { 4 } else { 8 };
            let body_start = i + 4;
            let Some(end) = (body_start..chars.len().saturating_sub(3))
                .find(|&k| chars[k..k + 4] == ['\\', 'X', '0', '\\'])
            else {
                out.push(chars[i]);
                i += 1;
                continue;
            };
            let body = &chars[body_start..end];
            let units: Option<Vec<u32>> = body.chunks(width).map(hex).collect();
            let decoded = units.and_then(|units| {
                if width == 4 {
                    let units: Vec<u16> = units
                        .iter()
                        .filter_map(|u| u16::try_from(*u).ok())
                        .collect();
                    String::from_utf16(&units).ok()
                } else {
                    units.iter().map(|u| char::from_u32(*u)).collect()
                }
            });
            match decoded {
                Some(decoded) if body.len().is_multiple_of(width) => {
                    out.push_str(&decoded);
                    i = end + 4;
                }
                _ => {
                    out.push(chars[i]);
                    i += 1;
                }
            }
        } else if starts("\\X\\") && rest.len() >= 5 {
            match hex(&rest[3..5]).and_then(char::from_u32) {
                Some(c) => {
                    out.push(c);
                    i += 5;
                }
                None => {
                    out.push(chars[i]);
                    i += 1;
                }
            }
        } else if starts("\\S\\") && rest.len() >= 4 {
            match char::from_u32(u32::from(rest[3]) + 0x80) {
                Some(c) if rest[3].is_ascii() => {
                    out.push(c);
                    i += 4;
                }
                _ => {
                    out.push(chars[i]);
                    i += 1;
                }
            }
        } else if starts("\\P") && rest.len() >= 4 && rest[3] == '\\' {
            i += 4;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {

    /// An argument costs a pointer's worth of payload and a tag: a file
    /// holds millions of them.
    #[test]
    fn an_argument_is_small() {
        assert!(
            core::mem::size_of::<Arg>() <= 32,
            "{}",
            core::mem::size_of::<Arg>()
        );
    }

    use super::*;

    const SMALL: &str = "ISO-10303-21;
HEADER;
FILE_DESCRIPTION(('a part'),'2;1');
FILE_NAME('p.stp','2020-01-01',(''),(''),'','','');
FILE_SCHEMA(('AP203'));
ENDSEC;
DATA;
#1=CARTESIAN_POINT('',(0.,1.5,-2.E-3));
#2=DIRECTION('',(0.,0.,1.));
#3=AXIS2_PLACEMENT_3D('',#1,#2,$);
#4=(GEOMETRIC_REPRESENTATION_CONTEXT(3) GLOBAL_UNIT_ASSIGNED_CONTEXT((#5)));
#5=SI_UNIT(.MILLI.,.METRE.);
ENDSEC;
END-ISO-10303-21;
";

    #[test]
    fn a_small_file_parses_into_its_instances() {
        let file = parse(SMALL).unwrap();
        assert_eq!(file.header.len(), 3);
        assert_eq!(file.data.len(), 5);

        let point = &file.data[&1];
        assert_eq!(point.keyword(), "CARTESIAN_POINT");
        let coords = point.parts[0].1[1].list().unwrap();
        assert_eq!(coords[0].number(), Some(0.0));
        assert_eq!(coords[1].number(), Some(1.5));
        assert_eq!(coords[2].number(), Some(-2e-3));

        let placement = &file.data[&3];
        assert_eq!(placement.parts[0].1[1].reference(), Some(1));
        assert_eq!(placement.parts[0].1[3], Arg::Null);

        // The complex instance keeps both parts, each with its own arguments.
        let context = &file.data[&4];
        assert_eq!(context.parts.len(), 2);
        assert!(context.part("GLOBAL_UNIT_ASSIGNED_CONTEXT").is_some());

        let unit = &file.data[&5];
        assert!(unit.parts[0].1[0].is_enum("MILLI"));
    }

    #[test]
    fn strings_undouble_their_quotes() {
        let file = parse("ISO-10303-21;HEADER;ENDSEC;DATA;#1=X('it''s');ENDSEC;END-ISO-10303-21;")
            .unwrap();
        assert_eq!(file.data[&1].parts[0].1[0], Arg::Str("it's".into()));
    }

    #[test]
    fn malformed_files_are_refused_with_a_place() {
        assert!(parse("ISO-10303-21;HEADER;DATA;").is_err());
        assert!(parse("not a step file").is_err());
    }
}
