//! Bounded Part 21 records that preserve legal empty aggregates in ruststep's public AST.
//!
//! The published parser requires a first element inside every aggregate. This adapter
//! constructs the same parameter, record and entity objects without changing STEP source
//! or conflating `()`, `$`, `*` and an empty complex-entity subtype. Optional auxiliary
//! sections retain the published parsers; binary parameters remain unsupported by the AST.

use crate::Result;
use std::mem::size_of;
use truck_stepio::r#in::ruststep::{
    ast::{DataSection, EntityInstance, Exchange, Name, Parameter, Record, SubSuperRecord},
    parser::{combinator::ParseResult, exchange},
};

/// Maximum input size, matching the shared file loader.
const MAX_INPUT: usize = 512 * 1024 * 1024;
/// Maximum estimated owned AST storage, including vector capacity slack.
const MAX_STORAGE: usize = 512 * 1024 * 1024;
/// Maximum aggregate, scalar and typed-parameter nodes.
const MAX_PARAMETERS: usize = 8_000_000;
/// Maximum data entities across all sections.
const MAX_ENTITIES: usize = 250_000;
/// Maximum simple or complex subtype records, including header records.
const MAX_RECORDS: usize = 1_000_000;
/// Maximum header, data and auxiliary section count.
const MAX_SECTIONS: usize = 1024;
/// Maximum actual parenthesis depth, including complex subtype wrappers.
const MAX_DEPTH: usize = 64;
/// Maximum auxiliary section bytes delegated at once.
const MAX_AUXILIARY: usize = 4 * 1024 * 1024;
/// Preserve the importer's established syntax error at the public boundary.
const INVALID: &str = "invalid STEP Part 21 exchange file";

/// Parse a complete exchange without replacing empty lists or modifying source text.
///
/// # Errors
///
/// Invalid syntax, unsupported binary values, overflowing/non-finite numbers, trailing
/// content, excessive nesting or the input/AST/record/section limits are rejected.
pub(super) fn parse(text: &str) -> Result<Exchange> {
    check_structure(text)?;
    let mut parser = Parser::new(text);
    parser.expect("ISO-10303-21;")?;
    parser.section()?;
    parser.expect("HEADER;")?;
    let mut header = Vec::new();
    while !parser.at("ENDSEC;")? {
        header.push(parser.record(1)?);
        parser.expect(";")?;
    }
    if header.is_empty() {
        return Err(INVALID.into());
    }
    parser.expect("ENDSEC;")?;
    let anchor = if parser.at("ANCHOR;")? {
        parser.auxiliary(exchange::anchor_section)?
    } else {
        Vec::new()
    };
    let reference = if parser.at("REFERENCE;")? {
        parser.auxiliary(exchange::reference_section)?
    } else {
        Vec::new()
    };
    let mut data = Vec::new();
    while parser.at("DATA")? {
        parser.section()?;
        parser.storage(2 * size_of::<DataSection>())?;
        data.push(parser.data()?);
    }
    parser.expect("END-ISO-10303-21;")?;
    let mut signature = Vec::new();
    while parser.at("SIGNATURE")? {
        signature.push(parser.auxiliary(exchange::signature_section)?);
    }
    parser.skip()?;
    if !parser.remaining.is_empty() {
        return Err(INVALID.into());
    }
    Ok(Exchange {
        header,
        anchor,
        reference,
        data,
        signature,
    })
}

/// Locate a STEP string's closing quote while honoring doubled apostrophes.
///
/// # Errors
///
/// A quoted token is not terminated.
fn string_end(input: &str) -> Result<usize> {
    let bytes = input.as_bytes();
    let mut position = 1;
    while position < bytes.len() {
        if bytes[position] == b'\'' {
            if bytes.get(position + 1) == Some(&b'\'') {
                position += 2;
            } else {
                return Ok(position + 1);
            }
        } else {
            position += 1;
        }
    }
    Err(INVALID.into())
}

/// Validate bounded nesting before entering owned or published recursive parsers.
///
/// # Errors
///
/// Input size, parentheses, strings, comments or resource delimiters are invalid.
fn check_structure(text: &str) -> Result<()> {
    if text.len() > MAX_INPUT {
        return Err("STEP input exceeds parser byte limit".into());
    }
    let mut remaining = text;
    let mut depth = 0usize;
    while !remaining.is_empty() {
        let bytes = remaining.as_bytes();
        let consumed = match bytes[0] {
            b'\'' => string_end(remaining)?,
            b'/' if bytes.get(1) == Some(&b'*') => remaining.find("*/").ok_or(INVALID)? + 2,
            b'<' => remaining.find('>').ok_or(INVALID)? + 1,
            b'(' => {
                depth += 1;
                if depth > MAX_DEPTH {
                    return Err("STEP parameter nesting exceeds importer limit".into());
                }
                1
            }
            b')' => {
                depth = depth.checked_sub(1).ok_or(INVALID)?;
                1
            }
            _ => remaining.chars().next().map_or(0, char::len_utf8),
        };
        remaining = &remaining[consumed..];
    }
    if depth != 0 {
        return Err(INVALID.into());
    }
    Ok(())
}

/// A forward-only cursor and counters charged before constructing owned AST objects.
struct Parser<'a> {
    /// Unconsumed original text; tokens are never rewritten.
    remaining: &'a str,
    /// Estimated allocation bytes still available.
    storage_left: usize,
    /// Parameter-node capacity still available.
    parameters_left: usize,
    /// Data-entity capacity still available.
    entities_left: usize,
    /// Simple-record capacity still available.
    records_left: usize,
    /// Section capacity still available.
    sections_left: usize,
}

impl<'a> Parser<'a> {
    /// Start a cursor after the whole-file nesting preflight.
    fn new(remaining: &'a str) -> Self {
        Self {
            remaining,
            storage_left: MAX_STORAGE,
            parameters_left: MAX_PARAMETERS,
            entities_left: MAX_ENTITIES,
            records_left: MAX_RECORDS,
            sections_left: MAX_SECTIONS,
        }
    }

    /// Charge estimated allocations before allocating or delegating.
    ///
    /// # Errors
    ///
    /// The AST storage budget would be exceeded.
    fn storage(&mut self, amount: usize) -> Result<()> {
        self.storage_left = self
            .storage_left
            .checked_sub(amount)
            .ok_or("STEP AST exceeds parser allocation limit")?;
        Ok(())
    }

    /// Charge a parameter and vector/box capacity overhead.
    ///
    /// # Errors
    ///
    /// The parameter count or AST storage budget would be exceeded.
    fn node(&mut self) -> Result<()> {
        self.parameters_left = self
            .parameters_left
            .checked_sub(1)
            .ok_or("STEP parameters exceed parser work limit")?;
        self.storage(2 * size_of::<Parameter>())
    }

    /// Charge another data or delegated auxiliary section.
    ///
    /// # Errors
    ///
    /// The section count would exceed the parser limit.
    fn section(&mut self) -> Result<()> {
        self.sections_left = self
            .sections_left
            .checked_sub(1)
            .ok_or("STEP sections exceed parser limit")?;
        Ok(())
    }

    /// Consume separators without allocating comment contents.
    ///
    /// # Errors
    ///
    /// A comment is not terminated.
    fn skip(&mut self) -> Result<()> {
        loop {
            self.remaining = self.remaining.trim_start_matches(char::is_whitespace);
            if self.remaining.starts_with("/*") {
                let end = self.remaining.find("*/").ok_or(INVALID)? + 2;
                self.remaining = &self.remaining[end..];
            } else if self.remaining.starts_with("\\N\\") || self.remaining.starts_with("\\F\\") {
                self.remaining = &self.remaining[3..];
            } else {
                return Ok(());
            }
        }
    }

    /// Test a literal after separators without consuming its bytes.
    ///
    /// # Errors
    ///
    /// A preceding separator is malformed.
    fn at(&mut self, literal: &str) -> Result<bool> {
        self.skip()?;
        Ok(self.remaining.starts_with(literal))
    }

    /// Require a literal after separators.
    ///
    /// # Errors
    ///
    /// The literal is missing or a separator is malformed.
    fn expect(&mut self, literal: &str) -> Result<()> {
        self.skip()?;
        self.remaining = self.remaining.strip_prefix(literal).ok_or(INVALID)?;
        Ok(())
    }

    /// Parse a standard or user-defined keyword with the upstream AST spelling.
    ///
    /// # Errors
    ///
    /// The keyword is absent or its allocation would exceed the budget.
    fn keyword(&mut self) -> Result<String> {
        self.skip()?;
        self.word(true)
    }

    /// Read one contiguous keyword token, optionally permitting a user-defined prefix.
    ///
    /// # Errors
    ///
    /// The keyword is missing, has a forbidden prefix or exceeds the allocation budget.
    fn word(&mut self, user_defined: bool) -> Result<String> {
        if !user_defined && self.remaining.starts_with('!') {
            return Err(INVALID.into());
        }
        // ruststep omits the syntactic '!' from user-defined keyword AST values.
        let source = self.remaining.strip_prefix('!').unwrap_or(self.remaining);
        let bytes = source.as_bytes();
        if !bytes
            .first()
            .is_some_and(|b| b.is_ascii_uppercase() || *b == b'_')
        {
            return Err(INVALID.into());
        }
        let end = bytes
            .iter()
            .take_while(|b| b.is_ascii_uppercase() || b.is_ascii_digit() || **b == b'_')
            .count();
        self.storage(end)?;
        let keyword = source[..end].to_owned();
        self.remaining = &source[end..];
        Ok(keyword)
    }

    /// Parse a nonempty unsigned identifier without unchecked integer conversion.
    ///
    /// # Errors
    ///
    /// Digits are missing or the identifier exceeds `u64`.
    fn identifier(&mut self) -> Result<u64> {
        let end = self
            .remaining
            .bytes()
            .take_while(u8::is_ascii_digit)
            .count();
        let value = self.remaining[..end].parse().map_err(|_| INVALID)?;
        self.remaining = &self.remaining[end..];
        Ok(value)
    }

    /// Parse an entity/value reference or a standard named constant.
    ///
    /// # Errors
    ///
    /// The name or numeric identifier is malformed.
    fn reference(&mut self, entity: bool) -> Result<Name> {
        self.remaining = &self.remaining[1..];
        if self
            .remaining
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_digit)
        {
            let id = self.identifier()?;
            Ok(if entity {
                Name::Entity(id)
            } else {
                Name::Value(id)
            })
        } else {
            let keyword = self.word(false)?;
            Ok(if entity {
                Name::ConstantEntity(keyword)
            } else {
                Name::ConstantValue(keyword)
            })
        }
    }

    /// Read one finite STEP integer or decimal real, including signed exponents.
    ///
    /// # Errors
    ///
    /// Numeric grammar is invalid, an integer overflows, or a real is non-finite.
    fn number(&mut self) -> Result<Parameter> {
        let bytes = self.remaining.as_bytes();
        let mut end = usize::from(matches!(bytes.first(), Some(b'+' | b'-')));
        let first_digit = end;
        while bytes.get(end).is_some_and(u8::is_ascii_digit) {
            end += 1;
        }
        if end == first_digit {
            return Err(INVALID.into());
        }
        let real = bytes.get(end) == Some(&b'.');
        if real {
            end += 1;
            while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                end += 1;
            }
            if bytes.get(end) == Some(&b'E') {
                end += 1;
                end += usize::from(matches!(bytes.get(end), Some(b'+' | b'-')));
                let exponent_start = end;
                while bytes.get(end).is_some_and(u8::is_ascii_digit) {
                    end += 1;
                }
                if end == exponent_start {
                    return Err(INVALID.into());
                }
            }
        }
        let token = &self.remaining[..end];
        let result = if real {
            let value: f64 = token.parse().map_err(|_| INVALID)?;
            if !value.is_finite() {
                return Err(INVALID.into());
            }
            Parameter::Real(value)
        } else {
            Parameter::Integer(token.parse().map_err(|_| INVALID)?)
        };
        self.remaining = &self.remaining[end..];
        Ok(result)
    }

    /// Parse one parameter, preserving empty aggregates and exact omission variants.
    ///
    /// # Errors
    ///
    /// Syntax, numeric values, nesting or AST resource budgets are invalid.
    fn parameter(&mut self, depth: usize) -> Result<Parameter> {
        if depth > MAX_DEPTH {
            return Err("STEP parameter nesting exceeds importer limit".into());
        }
        self.node()?;
        self.skip()?;
        match self.remaining.as_bytes().first().copied().ok_or(INVALID)? {
            b'(' => Ok(Parameter::List(self.parameters(depth + 1)?)),
            b'$' => {
                self.remaining = &self.remaining[1..];
                Ok(Parameter::NotProvided)
            }
            b'*' => {
                self.remaining = &self.remaining[1..];
                Ok(Parameter::Omitted)
            }
            b'\'' => {
                let end = string_end(self.remaining)?;
                self.storage(end - 2)?;
                let value = self.remaining[1..end - 1].replace("''", "'");
                self.remaining = &self.remaining[end..];
                Ok(Parameter::String(value))
            }
            b'#' => Ok(Parameter::Ref(self.reference(true)?)),
            b'@' => Ok(Parameter::Ref(self.reference(false)?)),
            b'.' => {
                self.remaining = &self.remaining[1..];
                let value = self.word(false)?;
                self.remaining = self.remaining.strip_prefix('.').ok_or(INVALID)?;
                Ok(Parameter::Enumeration(value))
            }
            b'+' | b'-' | b'0'..=b'9' => self.number(),
            b'!' | b'A'..=b'Z' | b'_' => {
                let keyword = self.keyword()?;
                self.expect("(")?;
                let parameter = Box::new(self.parameter(depth + 1)?);
                self.expect(")")?;
                Ok(Parameter::Typed { keyword, parameter })
            }
            _ => Err(INVALID.into()),
        }
    }

    /// Read a parenthesized, possibly empty comma-separated parameter sequence.
    ///
    /// # Errors
    ///
    /// Parameters, separators, closing parentheses or budgets are invalid.
    fn parameters(&mut self, depth: usize) -> Result<Vec<Parameter>> {
        self.expect("(")?;
        let mut values = Vec::new();
        if !self.at(")")? {
            loop {
                values.push(self.parameter(depth)?);
                if !self.at(",")? {
                    break;
                }
                self.expect(",")?;
            }
        }
        self.expect(")")?;
        Ok(values)
    }

    /// Build one simple record; an empty subtype has an empty parameter sequence.
    ///
    /// # Errors
    ///
    /// Record syntax, parameters or resource budgets are invalid.
    fn record(&mut self, depth: usize) -> Result<Record> {
        self.records_left = self
            .records_left
            .checked_sub(1)
            .ok_or("STEP records exceed parser work limit")?;
        self.storage(2 * size_of::<Record>())?;
        self.node()?;
        Ok(Record {
            name: self.keyword()?,
            parameter: Parameter::List(self.parameters(depth)?),
        })
    }

    /// Build one numeric-ID entity with simple or externally mapped subtype records.
    ///
    /// # Errors
    ///
    /// Entity syntax, subtype records or resource budgets are invalid.
    fn entity(&mut self) -> Result<EntityInstance> {
        self.entities_left = self
            .entities_left
            .checked_sub(1)
            .ok_or("STEP file exceeds entity limit")?;
        self.storage(2 * size_of::<EntityInstance>())?;
        self.expect("#")?;
        let id = self.identifier()?;
        self.expect("=")?;
        let entity = if self.at("(")? {
            self.expect("(")?;
            let mut records = Vec::new();
            while !self.at(")")? {
                records.push(self.record(2)?);
            }
            if records.is_empty() {
                return Err(INVALID.into());
            }
            self.expect(")")?;
            EntityInstance::Complex {
                id,
                subsuper: SubSuperRecord(records),
            }
        } else {
            EntityInstance::Simple {
                id,
                record: self.record(1)?,
            }
        };
        self.expect(";")?;
        Ok(entity)
    }

    /// Read one DATA section with optional metadata and zero or more instances.
    ///
    /// # Errors
    ///
    /// Metadata, entities, section syntax or resource limits are invalid.
    fn data(&mut self) -> Result<DataSection> {
        self.expect("DATA")?;
        let meta = if self.at("(")? {
            self.parameters(1)?
        } else {
            Vec::new()
        };
        self.expect(";")?;
        let mut entities = Vec::new();
        while !self.at("ENDSEC;")? {
            entities.push(self.entity()?);
        }
        self.expect("ENDSEC;")?;
        Ok(DataSection { meta, entities })
    }

    /// Delegate a bounded unchanged auxiliary section to its public ruststep parser.
    ///
    /// # Errors
    ///
    /// Auxiliary syntax, allocation/section budgets or published token conversion fail.
    fn auxiliary<T>(&mut self, parse: fn(&str) -> ParseResult<'_, T>) -> Result<T> {
        self.section()?;
        let mut end = 0;
        while !self.remaining[end..].starts_with("ENDSEC;") {
            let tail = &self.remaining[end..];
            if tail.is_empty() || end > MAX_AUXILIARY {
                return Err(
                    "STEP auxiliary section exceeds parser limit or is unterminated".into(),
                );
            }
            end += match tail.as_bytes()[0] {
                b'\'' => string_end(tail)?,
                b'<' => tail.find('>').ok_or(INVALID)? + 1,
                b'/' if tail.starts_with("/*") => tail.find("*/").ok_or(INVALID)? + 2,
                _ => tail.chars().next().map_or(0, char::len_utf8),
            };
        }
        end += "ENDSEC;".len();
        if end > MAX_AUXILIARY {
            return Err("STEP auxiliary section exceeds parser byte limit".into());
        }
        // Published token parsers collect Unicode chars and AST vectors; charge capacity
        // slack conservatively before entering them and catch their numeric-overflow panic.
        self.parameters_left = self
            .parameters_left
            .checked_sub(end)
            .ok_or("STEP auxiliary parameters exceed parser work limit")?;
        self.storage(end * 64)?;
        let source = &self.remaining[..end];
        let parsed = std::panic::catch_unwind(|| parse(source)).map_err(|_| INVALID)?;
        let (remaining, result) = parsed.map_err(|_| INVALID)?;
        if !remaining.is_empty() {
            return Err(INVALID.into());
        }
        self.remaining = &self.remaining[end..];
        Ok(result)
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    reason = "Tests require accepted fixtures to parse successfully."
)]
mod tests {
    use super::*;

    /// Wrap authored records in a minimal exchange accepted by the published parser.
    fn exchange(data: &str) -> String {
        format!(
            "ISO-10303-21;HEADER;FILE_SCHEMA(('TEST'));ENDSEC;DATA;{data}ENDSEC;END-ISO-10303-21;"
        )
    }

    /// Obtain a simple record's parameters for explicit AST assertions.
    fn values(exchange: &Exchange) -> &[Parameter] {
        let EntityInstance::Simple { record, .. } = &exchange.data[0].entities[0] else {
            panic!("expected authored simple record");
        };
        let Parameter::List(parameters) = &record.parameter else {
            panic!("expected record parameter sequence");
        };
        parameters
    }

    #[test]
    fn empty_aggregates_remain_distinct_from_omission_and_empty_subtypes() {
        let parsed = parse(&exchange(
            "#1=TEST((),$,*,TYPE(()),((())));#2=(BASE() SUBTYPE(()));",
        ))
        .unwrap();
        let parameters = values(&parsed);
        assert_eq!(parameters[0], Parameter::List(Vec::new()));
        assert_eq!(parameters[1], Parameter::NotProvided);
        assert_eq!(parameters[2], Parameter::Omitted);
        assert_eq!(
            parameters[3],
            Parameter::Typed {
                keyword: "TYPE".into(),
                parameter: Box::new(Parameter::List(Vec::new()))
            }
        );
        assert_eq!(
            parameters[4],
            Parameter::List(vec![Parameter::List(vec![Parameter::List(Vec::new())])])
        );
        let EntityInstance::Complex { subsuper, .. } = &parsed.data[0].entities[1] else {
            panic!("expected authored complex entity");
        };
        assert_eq!(subsuper.0[0].parameter, Parameter::List(Vec::new()));
        assert_eq!(
            subsuper.0[1].parameter,
            Parameter::List(vec![Parameter::List(Vec::new())])
        );
    }

    #[test]
    fn ordinary_entities_match_the_published_ast() {
        let source = exchange(
            "#001=TEST('name',-12,+3,1.,-0.25E+2,.T.,#42,@7,#CONST,@VALUE,TYPE(2.));#42=(BASE() SUBTYPE((1,2))); ",
        );
        assert_eq!(
            parse(&source).unwrap(),
            truck_stepio::r#in::ruststep::parser::parse(&source).unwrap()
        );
    }

    #[test]
    fn strings_comments_and_minimum_signed_integer_are_safe() {
        let parsed = parse(&exchange("/* () #7 */ #1=TEST('can''t /* comment */ ()','\\X2\\03A9\\X0\\',-9223372036854775808,/* empty */());")).unwrap();
        assert_eq!(
            values(&parsed)[0],
            Parameter::String("can't /* comment */ ()".into())
        );
        assert_eq!(
            values(&parsed)[1],
            Parameter::String("\\X2\\03A9\\X0\\".into())
        );
        assert_eq!(values(&parsed)[2], Parameter::Integer(i64::MIN));
        assert_eq!(values(&parsed)[3], Parameter::List(Vec::new()));
    }

    #[test]
    fn empty_header_metadata_and_multiple_data_sections_are_preserved() {
        let source = "ISO-10303-21;HEADER;FILE_NAME('', '', (), (), '', '', '');ENDSEC;DATA('parts',());#1=A(());ENDSEC;DATA;#2=B();ENDSEC;END-ISO-10303-21;";
        let parsed = parse(source).unwrap();
        assert_eq!(parsed.data.len(), 2);
        assert_eq!(
            parsed.data[0].meta,
            vec![
                Parameter::String("parts".into()),
                Parameter::List(Vec::new())
            ]
        );
        assert_eq!(
            parsed.header[0].parameter,
            Parameter::List(vec![
                Parameter::String(String::new()),
                Parameter::String(String::new()),
                Parameter::List(Vec::new()),
                Parameter::List(Vec::new()),
                Parameter::String(String::new()),
                Parameter::String(String::new()),
                Parameter::String(String::new())
            ])
        );
    }

    #[test]
    fn auxiliary_sections_keep_published_objects_and_resource_delimiters() {
        let source = "ISO-10303-21;HEADER;FILE_SCHEMA(('TEST'));ENDSEC;ANCHOR;<first>=(#1,$);ENDSEC;REFERENCE;#2=<https://example.com/ENDSEC;(x)>;ENDSEC;DATA;#1=A();ENDSEC;END-ISO-10303-21;SIGNATURE YWJjZA== ENDSEC;";
        let parsed = parse(source).unwrap();
        let published = truck_stepio::r#in::ruststep::parser::parse(source).unwrap();
        assert_eq!(parsed, published);
        assert_eq!(parsed.anchor.len(), 1);
        assert_eq!(parsed.reference.len(), 1);
        assert_eq!(parsed.signature, vec!["YWJjZA=="]);
    }

    #[test]
    fn malformed_tokens_and_incomplete_records_are_rejected_without_panics() {
        for data in [
            "#1=A((,));",
            "#1=A((1,));",
            "#1=A(TYPE());",
            "#1=();",
            "#1=A(.!T.);",
            "#1=A(. T.);",
            "#1=A(.T .);",
            "#1=A(# CONST);",
            "#1=A(@!CONST);",
            "#1=A(1.2E);",
            "#1=A(1E2);",
            "#1=A(1.E9999);",
            "#1=A(9223372036854775808);",
            "#18446744073709551616=A();",
            "#1=A('unfinished);",
            "#1=A(/* unfinished); ",
            "#1=A(\"0F\");",
            "#1=A(1);garbage",
            "#1=(A() B(,));",
            "#1=A(()); /*",
        ] {
            assert!(
                parse(&exchange(data)).is_err(),
                "accepted malformed record: {data}"
            );
        }
        assert!(parse(&(exchange("#1=A();") + "unexpected")).is_err());
        assert!(parse("ISO-10303-21;HEADER;ENDSEC;END-ISO-10303-21;").is_err());
    }

    #[test]
    fn nesting_and_allocation_work_counters_are_enforced_before_growth() {
        let nested = format!("#1=A({}0{});", "(".repeat(MAX_DEPTH), ")".repeat(MAX_DEPTH));
        assert!(parse(&exchange(&nested)).is_err());
        let mut parser = Parser::new("(1,2)");
        parser.parameters_left = 1;
        assert!(parser.parameters(1).is_err());
        let mut parser = Parser::new("'long'");
        parser.storage_left = 2 * size_of::<Parameter>();
        assert!(parser.parameter(1).is_err());
        let mut parser = Parser::new("A()");
        parser.records_left = 0;
        assert!(parser.record(1).is_err());
        let mut parser = Parser::new("#1=A();");
        parser.entities_left = 0;
        assert!(parser.entity().is_err());
        let mut parser = Parser::new("DATA;ENDSEC;");
        parser.sections_left = 0;
        assert!(parser.section().is_err());
    }
}
