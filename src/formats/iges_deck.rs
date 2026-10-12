//! Bounded IGES record access for the preview adapter; no geometry is inferred here.
use crate::Result;
use std::{
    collections::{BTreeMap, BTreeSet},
    ops::Range,
};

/// One parsed parameter, retaining the distinction between omitted fields and text.
#[derive(Clone, Debug)]
pub(super) enum Value {
    /// A finite numeric literal, including supported Fortran exponent notation.
    Number(f64),
    /// A length-delimited Hollerith string.
    Text(String),
    /// An empty parameter whose format-defined default is supplied by its owner.
    Default,
}

/// Directory metadata and parameters for one validated source entity.
#[derive(Clone, Debug)]
pub(super) struct Entity {
    /// Numeric entity type shared by both directory records and its parameters.
    pub kind: i64,
    /// Entity-specific form number.
    pub form: i64,
    /// Directory pointer to the entity's optional transformation matrix.
    pub transform: i64,
    /// Packed blanking, subordination, usage and hierarchy status digits.
    pub status: i64,
    /// Geometry parameters after the leading entity-type field.
    pub params: Vec<Value>,
    /// First paired directory record in the retained fixed-column lines.
    directory_line: usize,
    /// First parameter record in the retained contiguous parameter section.
    parameter_line: usize,
    /// Leading numeric type field within the joined 64-column parameter payloads.
    parameter_type: Range<usize>,
}

impl Entity {
    /// Require purely numeric parameters before using defaulting geometry accessors.
    ///
    /// # Errors
    /// Returns an error when any geometry parameter contains a Hollerith string.
    pub fn numeric_geometry(&self) -> Result<()> {
        if self.params.iter().any(|v| matches!(v, Value::Text(_))) {
            return Err("IGES numeric geometry field contains text".into());
        }
        Ok(())
    }

    /// Read a numeric field after `numeric_geometry`, defaulting omitted fields to zero.
    pub fn number(&self, index: usize) -> f64 {
        match self.params.get(index) {
            Some(Value::Number(value)) => *value,
            _ => 0.,
        }
    }
    /// Read a finite numeric field or its zero default.
    ///
    /// # Errors
    /// Returns an error when the selected parameter contains text.
    pub fn real(&self, index: usize) -> Result<f64> {
        match self.params.get(index) {
            Some(Value::Number(value)) => Ok(*value),
            Some(Value::Default) | None => Ok(0.),
            Some(Value::Text(_)) => Err("IGES numeric geometry field contains text".into()),
        }
    }

    /// Read an exactly representable integer field.
    ///
    /// # Errors
    /// Returns an error for text, fractional values or integers beyond exact f64 precision.
    pub fn integer(&self, index: usize) -> Result<i64> {
        exact_integer(self.real(index)?)
    }

    /// Read a nonnegative count bounded by its parameter record and the topology ceiling.
    ///
    /// # Errors
    /// Returns an error for invalid integers, negative counts or excessive record expansion.
    pub fn count(&self, index: usize) -> Result<usize> {
        let count = usize::try_from(self.integer(index)?)?;
        if count > self.params.len() || count > 100_000 {
            return Err("IGES geometry count exceeds its bounded parameter record".into());
        }
        Ok(count)
    }
}

/// A validated exchange deck with stable directory identities and unit conversion.
pub(super) struct Deck {
    /// Original entity definitions indexed by directory sequence pointer.
    pub entities: BTreeMap<i64, Entity>,
    /// Multiplier converting authored length units to millimetres.
    pub scale: f64,
    /// Original fixed-column records, excluding empty lines.
    lines: Vec<String>,
}

/// Convert a finite parameter into an exact integer without lossy identity comparisons.
///
/// # Errors
/// Returns an error for nonfinite, fractional or excessively large values.
fn exact_integer(value: f64) -> Result<i64> {
    if !value.is_finite() || value.fract() != 0. || value.abs() > 2_f64.powi(53) {
        return Err("IGES geometry requires an exact integer field".into());
    }
    #[allow(
        clippy::cast_possible_truncation,
        reason = "finite integral value bounded to exact f64 precision"
    )]
    Ok(value as i64)
}

/// Parse a blank-defaulting integer field from a fixed-column record.
///
/// # Errors
/// Returns an error for invalid UTF-8 or a noninteger field.
fn field(bytes: &[u8]) -> Result<i64> {
    let value = std::str::from_utf8(bytes)?.trim();
    Ok(if value.is_empty() { 0 } else { value.parse()? })
}

/// Parse bounded numeric, omitted and Hollerith parameters using the deck delimiters.
///
/// # Errors
/// Returns an error for truncation, invalid numbers/strings, nonfinite values or excess fields.
fn values(text: &str, delimiter: u8, terminal: u8) -> Result<Vec<Value>> {
    let bytes = text.as_bytes();
    let mut at = 0;
    let mut result = Vec::new();
    loop {
        while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
            at += 1;
        }
        let start = at;
        while bytes.get(at).is_some_and(u8::is_ascii_digit) {
            at += 1;
        }
        let value = if at > start && bytes.get(at) == Some(&b'H') {
            let length = std::str::from_utf8(&bytes[start..at])?.parse::<usize>()?;
            at += 1;
            let end = at
                .checked_add(length)
                .ok_or("IGES string length overflow")?;
            let value = std::str::from_utf8(bytes.get(at..end).ok_or("truncated IGES string")?)?;
            at = end;
            while bytes.get(at).is_some_and(u8::is_ascii_whitespace) {
                at += 1;
            }
            Value::Text(value.to_owned())
        } else {
            at = start;
            while bytes
                .get(at)
                .is_some_and(|b| *b != delimiter && *b != terminal)
            {
                at += 1;
            }
            let value = std::str::from_utf8(&bytes[start..at])?.trim();
            if value.is_empty() {
                Value::Default
            } else {
                let value = value.replace(['D', 'd'], "E").parse::<f64>()?;
                if !value.is_finite() {
                    return Err("IGES geometry contains a non-finite value".into());
                }
                Value::Number(value)
            }
        };
        result.push(value);
        match bytes.get(at) {
            Some(byte) if *byte == delimiter => at += 1,
            Some(byte) if *byte == terminal => return Ok(result),
            _ => return Err("IGES parameter record lacks its terminator".into()),
        }
        if result.len() > 2_000_000 {
            return Err("IGES parameter record exceeds preview limit".into());
        }
    }
}

/// Decode the global delimiter declarations and millimetre unit conversion.
///
/// # Errors
/// Returns an error for truncated delimiters, malformed global parameters or unsupported units.
fn configuration(global: &str) -> Result<(u8, u8, f64)> {
    let global = global.trim_start();
    let delimiter = if global.starts_with("1H") {
        *global.as_bytes().get(2).ok_or("truncated IGES delimiter")?
    } else {
        b','
    };
    let second = if global.starts_with("1H") { 4 } else { 1 };
    let terminal = if global.as_bytes().get(second..second + 2) == Some(b"1H") {
        *global
            .as_bytes()
            .get(second + 2)
            .ok_or("truncated IGES delimiters")?
    } else {
        b';'
    };
    let global = values(global, delimiter, terminal)?;
    let global = Entity {
        kind: 0,
        form: 0,
        transform: 0,
        status: 0,
        params: global,
        directory_line: 0,
        parameter_line: 0,
        parameter_type: 0..0,
    };
    let scale = match global.integer(13)? {
        1 => 25.4,
        2 => 1.,
        4 => 304.8,
        5 => 1_609_344.,
        6 => 1000.,
        7 => 1_000_000.,
        8 => 0.0254,
        9 => 0.001,
        10 => 10.,
        11 => 0.000_025_4,
        3 => match global.params.get(14) {
            Some(Value::Text(name)) => match name.to_ascii_uppercase().as_str() {
                "IN" | "INCH" => 25.4,
                "MM" => 1.,
                "FT" => 304.8,
                "MI" => 1_609_344.,
                "M" => 1000.,
                "KM" => 1_000_000.,
                "MIL" => 0.0254,
                "UM" => 0.001,
                "CM" => 10.,
                "UIN" => 0.000_025_4,
                _ => return Err("unsupported IGES units".into()),
            },
            _ => return Err("IGES custom units lack their name".into()),
        },
        _ => return Err("unsupported IGES units".into()),
    };
    Ok((delimiter, terminal, scale))
}

impl Deck {
    /// Validate the complete record envelope, then retain entity definitions and global units.
    ///
    /// # Errors
    /// Returns an error for malformed records, delimiters, units, entity types or parameter ownership.
    pub fn read(text: &str) -> Result<Self> {
        // All fixed-column indexing below relies on this complete record,
        // count, ordering, ownership and sequence validation.
        super::super::validate_iges(text)?;
        let lines: Vec<_> = text
            .lines()
            .filter(|s| !s.trim().is_empty())
            .map(str::to_owned)
            .collect();
        let mut global = String::new();
        let mut directories = Vec::new();
        let mut parameters = Vec::new();
        for (index, line) in lines.iter().enumerate() {
            match line.as_bytes()[72] {
                b'G' => global.push_str(&line[..72]),
                b'D' => directories.push(index),
                b'P' => parameters.push(index),
                _ => {}
            }
        }
        let (delimiter, terminal, scale) = configuration(&global)?;
        let mut entities = BTreeMap::new();
        if !directories.len().is_multiple_of(2) {
            return Err("IGES directory lacks a paired record".into());
        }
        for pair in directories.as_chunks::<2>().0 {
            let first = lines[pair[0]].as_bytes();
            let second = lines[pair[1]].as_bytes();
            let pointer = field(&first[73..80])?;
            let parameter = usize::try_from(field(&first[8..16])?)?
                .checked_sub(1)
                .ok_or("invalid IGES parameter pointer")?;
            let count = usize::try_from(field(&second[24..32])?)?;
            let mut data = String::new();
            let end = parameter
                .checked_add(count)
                .ok_or("IGES parameter count overflow")?;
            for &index in parameters
                .get(parameter..end)
                .ok_or("truncated IGES parameters")?
            {
                data.push_str(&lines[index][..64]);
            }
            let mut params = values(&data, delimiter, terminal)?;
            let type_start = data
                .bytes()
                .position(|byte| !byte.is_ascii_whitespace())
                .ok_or("IGES parameters lack their entity type")?;
            let type_end = data
                .bytes()
                .position(|byte| byte == delimiter || byte == terminal)
                .ok_or("IGES entity type lacks its delimiter")?;
            let kind = i64::from(u16::try_from(field(&first[..8])?)?);
            let numeric_kind = match params.first() {
                Some(Value::Number(value)) => exact_integer(*value)?,
                _ => return Err("IGES parameters lack a numeric entity type".into()),
            };
            if numeric_kind != kind {
                return Err("IGES parameter/directory types disagree".into());
            }
            params.remove(0);
            if entities
                .insert(
                    pointer,
                    Entity {
                        kind,
                        form: field(&second[32..40])?,
                        transform: field(&first[48..56])?,
                        status: field(&first[64..72])?,
                        params,
                        directory_line: pair[0],
                        parameter_line: parameters[parameter],
                        parameter_type: type_start..type_end,
                    },
                )
                .is_some()
            {
                return Err("duplicate IGES directory pointer".into());
            }
        }
        if entities.len() > 100_000 {
            return Err("IGES preview exceeds 100000 entities".into());
        }
        Ok(Self {
            entities,
            scale,
            lines,
        })
    }

    /// Resolve a signed directory reference by its absolute source identity.
    ///
    /// # Errors
    /// Returns an error for an overflowing signed pointer or a missing entity.
    pub fn entity(&self, pointer: i64) -> Result<&Entity> {
        self.entities
            .get(&pointer.checked_abs().ok_or("IGES pointer overflow")?)
            .ok_or_else(|| format!("IGES pointer D{pointer} names no entity").into())
    }

    /// Keep native directory pointers stable. Owned-only definitions become
    /// harmless unused directions in the delegation deck; shared definitions
    /// retain their exact original records and parameters.
    ///
    /// # Errors
    /// Returns an error for missing entities or an invalid leading parameter-type field.
    pub fn delegation(&self, masked: &BTreeSet<i64>, face_roots: &[i64]) -> Result<String> {
        let mut lines = self.lines.clone();
        for &id in masked {
            let entity = self.entity(id)?;
            for index in [entity.directory_line, entity.directory_line + 1] {
                lines[index].replace_range(..8, "     123");
            }
            if entity.parameter_type.len() < 3 {
                return Err("IGES delegated entity type cannot hold its replacement".into());
            }
            // Replace the entire numeric spelling, including leading zeros or
            // exponent notation, while retaining every record width and pointer.
            for (offset, at) in entity.parameter_type.clone().enumerate() {
                let line = lines
                    .get_mut(entity.parameter_line + at / 64)
                    .ok_or("truncated IGES delegated entity type")?;
                let column = at % 64;
                let original = line
                    .get(column..=column)
                    .ok_or("IGES entity type must use single-byte numeric characters")?;
                if !original.is_ascii() {
                    return Err("IGES entity type must use ASCII numeric characters".into());
                }
                let replacement = match offset {
                    0 => "1",
                    1 => "2",
                    2 => "3",
                    _ => " ",
                };
                line.replace_range(column..=column, replacement);
            }
        }
        for &id in face_roots {
            let entity = self.entity(id)?;
            let status = entity.status + 10_000;
            lines[entity.directory_line].replace_range(64..72, &format!("{status:08}"));
        }
        Ok(lines.join("\n"))
    }
}

/// Regressions for alternate numeric type spellings in delegated fixed-column records.
#[cfg(test)]
mod tests {
    use super::{BTreeSet, Deck};

    /// Change one retained point's leading type spelling without changing record widths.
    fn alternate_type(spelling: &str) -> (String, i64) {
        let source = super::super::test_input::point().unwrap();
        let deck = Deck::read(&source).unwrap();
        let (&id, entity) = deck
            .entities
            .iter()
            .find(|(_, entity)| entity.kind == 116)
            .unwrap();
        let mut lines: Vec<String> = source.lines().map(str::to_owned).collect();
        let line = &mut lines[entity.parameter_line];
        let mut payload = line[..64].trim_end().to_owned();
        payload.replace_range(entity.parameter_type.clone(), spelling);
        assert!(payload.len() <= 64);
        line.replace_range(..64, &format!("{payload:<64}"));
        (lines.join("\n"), id)
    }

    /// Preserve numeric parameters and record identities when masking noncanonical type fields.
    #[test]
    fn delegation_preserves_noncanonical_numeric_type_parameters() {
        for spelling in ["000116", "116.0", "1.16E2"] {
            let (source, id) = alternate_type(spelling);
            let original = Deck::read(&source).unwrap();
            let delegated = original.delegation(&BTreeSet::from([id]), &[]).unwrap();
            let imported = Deck::read(&delegated).unwrap();
            let before = original.entity(id).unwrap();
            let after = imported.entity(id).unwrap();
            assert_eq!(after.kind, 123);
            assert_eq!(after.params.len(), before.params.len());
            for index in 0..before.params.len() {
                assert_eq!(after.real(index).unwrap(), before.real(index).unwrap());
            }
            for (before, after) in source.lines().zip(delegated.lines()) {
                assert_eq!(before.len(), after.len());
                assert_eq!(&before[64..], &after[64..]);
            }
        }
    }
}
