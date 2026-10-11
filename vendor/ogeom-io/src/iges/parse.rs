//! The IGES file model: sections, directory entries, parameters.
//!
//! IGES is a 1980s fixed-column format, and the parser honours that rather
//! than fighting it: a file is a deck of 80-column records, column 73 names
//! the section (Start, Global, Directory, Parameter, Terminate) and columns
//! 74 to 80 number the record within its section. The Directory section holds
//! two fixed-format lines of eight-character fields per entity; the Parameter
//! section holds free-format values whose delimiters the Global section
//! itself declares, strings as Hollerith constants (`4Htext`), and reals in
//! Fortran spellings (`1.0D0` as well as `1.0E0`).
//!
//! Everything here is *structure*; meaning belongs to [`super::read`]. The
//! one interpretation this layer performs is the pointer convention: a
//! parameter that references another entity holds that entity's directory
//! sequence number (always odd, since each entity owns two directory lines),
//! and a negative value carries orientation or dependency context the
//! consumer may use, so the sign is preserved.

use ogeom_core::{OgeomResult, ogeom_bail};
use std::collections::BTreeMap;

/// One parameter value from the parameter data section.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    /// An integer, which is also how pointers arrive.
    Int(i64),
    /// A real, accepting both `E` and Fortran `D` exponents.
    Real(f64),
    /// A Hollerith string, decoded.
    Text(String),
    /// An empty field between delimiters: the entity's documented default.
    Default,
}

impl Value {
    /// The value as a real, with integers promoted and defaults zero.
    pub fn real(&self) -> f64 {
        match self {
            Self::Int(i) => {
                // A pointer-sized integer is exactly representable far past
                // any coordinate an IGES file can carry.
                #[allow(clippy::cast_precision_loss, reason = "file coordinates")]
                {
                    *i as f64
                }
            }
            Self::Real(r) => *r,
            _ => 0.0,
        }
    }

    /// The value as an integer, with defaults zero.
    pub fn int(&self) -> i64 {
        match self {
            Self::Int(i) => *i,
            Self::Real(r) => {
                #[allow(clippy::cast_possible_truncation, reason = "file integer")]
                {
                    *r as i64
                }
            }
            _ => 0,
        }
    }
}

/// One entity: its directory entry fields and its parameters.
#[derive(Debug, Clone)]
pub struct Entity {
    /// The entity type number: 100 is a circular arc, 126 a B-spline curve.
    pub kind: i64,
    /// The form number, which selects among an entity type's variants.
    pub form: i64,
    /// Directory pointer to a transformation matrix entity, or 0.
    pub transform: i64,
    /// The colour: a negated directory pointer to a 314, a small positive
    /// palette number, or 0.
    pub colour: i64,
    /// The level (layer) number, or a negated pointer to a level property
    /// listing several; the reader maps each level to a document layer.
    pub level: i64,
    /// The four two-digit fields of the status word, as one number.
    pub status: i64,
    /// The entity label, trimmed.
    pub label: String,
    /// The parameters, without the leading entity-type number.
    pub params: Vec<Value>,
}

impl Entity {
    /// Parameter `i` (0-based), or `Default` past the end; trailing defaults
    /// are legitimately omitted by writers.
    pub fn at(&self, i: usize) -> &Value {
        static DEFAULT: Value = Value::Default;
        self.params.get(i).unwrap_or(&DEFAULT)
    }

    /// Parameter `i` read as a count or a degree: zero where it is negative
    /// or absent, and never more than the entity's own parameters. Every
    /// counted item takes at least one parameter, so no valid count is
    /// larger, and a count from the file cannot size an allocation past
    /// what the file holds.
    #[must_use]
    pub fn count(&self, i: usize) -> usize {
        usize::try_from(self.at(i).int())
            .unwrap_or(0)
            .min(self.params.len())
    }
}

/// A parsed file: global parameters and entities by directory pointer.
#[derive(Debug)]
pub struct File {
    /// The global section's parameters, in order.
    pub global: Vec<Value>,
    /// Entities keyed by their directory-entry pointer (1, 3, 5, …).
    pub entities: BTreeMap<i64, Entity>,
}

impl File {
    /// The entity a parameter points at, sign ignored.
    pub fn entity(&self, pointer: i64) -> Option<&Entity> {
        self.entities.get(&pointer.abs())
    }

    /// Millimetres per model unit, from the global units flag, with the
    /// units-name fallback the flag value 3 delegates to.
    pub fn scale_mm(&self) -> Option<f64> {
        let flag = self.global.get(13).map_or(0, Value::int);
        Some(match flag {
            1 => 25.4,
            2 => 1.0,
            3 => match self.global.get(14) {
                Some(Value::Text(name)) => unit_by_name(name)?,
                _ => return None,
            },
            4 => 304.8,
            5 => 1_609_344.0,
            6 => 1000.0,
            7 => 1.0e6,
            8 => 0.0254,
            9 => 1.0e-3,
            10 => 10.0,
            11 => 2.54e-5,
            _ => return None,
        })
    }
}

fn unit_by_name(name: &str) -> Option<f64> {
    Some(match name.to_ascii_uppercase().as_str() {
        "IN" | "INCH" => 25.4,
        "MM" => 1.0,
        "FT" => 304.8,
        "MI" => 1_609_344.0,
        "M" => 1000.0,
        "KM" => 1.0e6,
        "MIL" => 0.0254,
        "UM" => 1.0e-3,
        "CM" => 10.0,
        "UIN" => 2.54e-5,
        _ => return None,
    })
}

/// Parse an IGES file.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) if the
/// deck is not IGES-shaped: no directory section, a directory entry without
/// its second line, a field that is not the number it must be, or an
/// unterminated Hollerith string.
pub fn parse(text: &str) -> OgeomResult<File> {
    let mut start = String::new();
    let mut global_text = String::new();
    let mut directory: Vec<String> = Vec::new();
    // Parameter records grouped by the directory back-pointer in cols 66 to 72.
    let mut params: BTreeMap<i64, String> = BTreeMap::new();

    for line in text.lines() {
        if line.trim().is_empty() {
            continue;
        }
        // The section letter lives in column 73. Short lines are padded: some
        // writers trim trailing blanks, and the format's fixed columns must
        // survive that. Columns are bytes: a label holding a character past
        // ASCII takes as many columns as its encoding has bytes.
        let mut record = line.as_bytes().to_vec();
        if record.len() < 80 {
            record.resize(80, b' ');
        }
        let columns = |from: usize, to: usize| String::from_utf8_lossy(&record[from..to]);
        let section = record[72] as char;
        match section {
            'S' => start.push_str(columns(0, 72).trim_end()),
            'G' => {
                global_text.push_str(&columns(0, 72));
            }
            'D' => directory.push(columns(0, 72).into_owned()),
            'P' => {
                let back: i64 = columns(64, 72).trim().parse().map_err(|_| {
                    ogeom_core::ogeom_err!(
                        Construction,
                        "IGES parameter record carries no directory back-pointer"
                    )
                })?;
                params.entry(back).or_default().push_str(&columns(0, 64));
            }
            'T' => break,
            _ => ogeom_bail!(
                Construction,
                "IGES record with unknown section letter {section:?} in column 73"
            ),
        }
    }
    let _ = start;
    if directory.is_empty() {
        ogeom_bail!(Construction, "IGES file has no directory section");
    }
    if !directory.len().is_multiple_of(2) {
        ogeom_bail!(
            Construction,
            "IGES directory holds {} lines; entries are pairs",
            directory.len()
        );
    }

    // The global section declares its own delimiters in its first one or two
    // parameters; until they are read, the defaults hold.
    let (param_delim, record_delim, global) = parse_global(&global_text)?;

    let mut entities = BTreeMap::new();
    for (index, pair) in directory.chunks(2).enumerate() {
        let de_pointer = 2 * index as i64 + 1;
        let f = |line: &str, field: usize| -> String {
            line.get(field * 8..(field + 1) * 8)
                .unwrap_or("")
                .trim()
                .to_string()
        };
        let int = |line: &str, field: usize| -> OgeomResult<i64> {
            let s = f(line, field);
            if s.is_empty() {
                return Ok(0);
            }
            s.parse().map_err(|_| {
                ogeom_core::ogeom_err!(
                    Construction,
                    "IGES directory field {field} of entry {de_pointer} is {s:?}, not an integer"
                )
            })
        };
        let kind = int(&pair[0], 0)?;
        // The status word keeps blank-padded zeros meaningful, so it is read
        // as digits rather than a plain integer.
        let status: i64 = pair[0][64..72]
            .replace(' ', "0")
            .parse()
            .unwrap_or_default();
        // Line one carries fields 1 to 10 of the entry, line two fields 11 to 20;
        // within each line a field's index is its number minus the line's
        // first. The label is field 18, the form 15, the colour 13.
        let entity = Entity {
            kind,
            form: int(&pair[1], 4)?,
            transform: int(&pair[0], 6)?,
            colour: int(&pair[1], 2)?,
            level: int(&pair[0], 4)?,
            status,
            label: f(&pair[1], 7),
            params: Vec::new(),
        };
        entities.insert(de_pointer, entity);
    }

    for (back, text) in params {
        let values = parse_params(&text, param_delim, record_delim)?;
        let Some(entity) = entities.get_mut(&back) else {
            ogeom_bail!(
                Construction,
                "IGES parameter data points back at directory entry {back}, which does not exist"
            );
        };
        let Some((Value::Int(kind), rest)) = values.split_first() else {
            ogeom_bail!(
                Construction,
                "IGES parameter record for entry {back} does not begin with its entity type"
            );
        };
        if *kind != entity.kind {
            ogeom_bail!(
                Construction,
                "IGES entry {back} is type {} in the directory and {kind} in its parameters",
                entity.kind
            );
        }
        entity.params = rest.to_vec();
    }

    Ok(File { global, entities })
}

/// The global section: delimiters first, then everything else.
fn parse_global(text: &str) -> OgeomResult<(char, char, Vec<Value>)> {
    // Defaults per the specification.
    let mut param_delim = ',';
    let mut record_delim = ';';
    let bytes = text.as_bytes();
    let mut i = 0;
    // First parameter: the parameter delimiter, `1H,` or empty for default.
    if bytes.first() == Some(&b'1') && bytes.get(1) == Some(&b'H') {
        param_delim = bytes.get(2).map_or(param_delim, |b| *b as char);
        i = 4; // past "1H?" and the delimiter that follows it
    } else if bytes.first() == Some(&b',') {
        i = 1;
    }
    // Second: the record delimiter.
    if bytes.get(i) == Some(&b'1') && bytes.get(i + 1) == Some(&b'H') {
        record_delim = bytes.get(i + 2).map_or(record_delim, |b| *b as char);
        i += 3;
        if bytes.get(i) == Some(&(param_delim as u8)) {
            i += 1;
        }
    } else if bytes.get(i) == Some(&(param_delim as u8)) {
        i += 1;
    }
    let mut global = vec![
        Value::Text(param_delim.to_string()),
        Value::Text(record_delim.to_string()),
    ];
    global.extend(parse_params(
        &String::from_utf8_lossy(bytes.get(i..).unwrap_or(&[])),
        param_delim,
        record_delim,
    )?);
    Ok((param_delim, record_delim, global))
}

/// Free-format parameter text into values.
fn parse_params(text: &str, param_delim: char, record_delim: char) -> OgeomResult<Vec<Value>> {
    let mut out = Vec::new();
    let bytes = text.as_bytes();
    let mut i = 0;
    let mut field_start = 0;
    while i <= bytes.len() {
        let at_end = i == bytes.len();
        let c = if at_end {
            param_delim
        } else {
            bytes[i] as char
        };
        if !at_end && c.is_ascii_digit() {
            // Possible Hollerith: digits followed by `H` swallow the counted
            // characters verbatim, delimiters included.
            let digits_start = i;
            let mut j = i;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if bytes.get(j) == Some(&b'H')
                && bytes[field_start..digits_start]
                    .iter()
                    .all(u8::is_ascii_whitespace)
            {
                let n: usize = String::from_utf8_lossy(&bytes[digits_start..j])
                    .parse()
                    .map_err(|_| {
                        ogeom_core::ogeom_err!(Construction, "IGES Hollerith count out of range")
                    })?;
                let Some(end) = n.checked_add(j + 1).filter(|end| *end <= bytes.len()) else {
                    ogeom_bail!(
                        Construction,
                        "IGES Hollerith string runs past the end of its record"
                    );
                };
                out.push(Value::Text(
                    String::from_utf8_lossy(&bytes[j + 1..end]).into_owned(),
                ));
                i = end;
                // Skip the delimiter after the string, if present.
                if i < bytes.len()
                    && (bytes[i] as char == param_delim || bytes[i] as char == record_delim)
                {
                    i += 1;
                }
                field_start = i;
                continue;
            }
            i = j;
            continue;
        }
        if at_end || c == param_delim || c == record_delim {
            let field = String::from_utf8_lossy(&bytes[field_start..i]);
            let field = field.trim();
            if !field.is_empty() || (!at_end && c == param_delim) {
                out.push(scalar(field)?);
            }
            if !at_end && c == record_delim {
                break;
            }
            i += 1;
            field_start = i;
            continue;
        }
        i += 1;
    }
    // Trailing all-default fields drop; interior ones were kept above.
    while out.last() == Some(&Value::Default) {
        out.pop();
    }
    Ok(out)
}

/// One scalar field: integer, real (with Fortran `D` exponents), or default.
fn scalar(field: &str) -> OgeomResult<Value> {
    if field.is_empty() {
        return Ok(Value::Default);
    }
    if let Ok(i) = field.parse::<i64>() {
        return Ok(Value::Int(i));
    }
    let normalised = field.replace(['D', 'd'], "E");
    if let Ok(r) = normalised.parse::<f64>() {
        return Ok(Value::Real(r));
    }
    ogeom_bail!(Construction, "IGES parameter {field:?} is not a number")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, reason = "test code")]
mod tests {
    use super::*;

    /// A record padded to 80 columns with its section letter and number.
    fn rec(body: &str, section: char, seq: usize) -> String {
        format!("{body:<72}{section}{seq:>7}\n")
    }

    /// A directory line from its ten eight-character fields.
    fn dline(fields: [&str; 9]) -> String {
        fields.iter().map(|f| format!("{f:>8}")).collect()
    }

    fn tiny_file() -> String {
        let mut s = String::new();
        s += &rec("test deck", 'S', 1);
        s += &rec(
            "1H,,1H;,4Htest,8Htest.igs,6Hogeom,6Hogeom,32,38,6,308,15,4Htest,",
            'G',
            1,
        );
        s += &rec(
            "1.0,2,2HMM,1,0.01,15H20260807.120000,1D-06,10.0,2Hme,",
            'G',
            2,
        );
        s += &rec("4Hhere,11,0,15H20260807.120000;", 'G', 3);
        // A line entity: two directory lines, one parameter record.
        s += &rec(
            &dline(["110", "1", "0", "0", "0", "0", "0", "0", "00000000"]),
            'D',
            1,
        );
        s += &rec(
            &dline(["110", "0", "0", "1", "0", "", "", "LINE", "0"]),
            'D',
            2,
        );
        s += &format!("{:<64}{:>8}P{:>7}\n", "110,0.,0.,0.,10.,0.,0.;", 1, 1);
        s += &rec("S      1G      3D      2P      1", 'T', 1);
        s
    }

    #[test]
    fn a_tiny_deck_parses_with_its_delimiters_and_units() {
        let file = parse(&tiny_file()).unwrap();
        assert_eq!(file.scale_mm(), Some(1.0));
        assert_eq!(file.entities.len(), 1);
        let line = file.entity(1).unwrap();
        assert_eq!(line.kind, 110);
        assert_eq!(line.label, "LINE");
        assert_eq!(line.params.len(), 6);
        assert_eq!(line.at(3).real(), 10.0);
    }

    #[test]
    fn hollerith_strings_swallow_delimiters_and_d_exponents_read() {
        let vals = parse_params("3H a,,,7H,;,1H,A,1.5D2,2;", ',', ';').unwrap();
        assert_eq!(
            vals,
            vec![
                Value::Text(" a,".into()),
                Value::Default,
                Value::Text(",;,1H,A".into()),
                Value::Real(150.0),
                Value::Int(2),
            ]
        );
    }

    /// Columns are bytes. A character past ASCII straddling the column a
    /// field starts at leaves that field unreadable, which is an error to
    /// report, not a string to slice through the middle of a character.
    #[test]
    fn a_character_straddling_a_column_is_an_error_not_a_panic() {
        let mut s = tiny_file();
        let at = s.find("110,0.,0.,0.,10.,0.,0.;").unwrap();
        let record = format!("{:<63}\u{f8}{:>7}P{:>7}\n", "110,0.,0.,0.,10.,0.,0.;", 1, 1);
        let end = at + s[at..].find('\n').unwrap() + 1;
        s.replace_range(at..end, &record);
        assert!(parse(&s).is_err());
    }

    #[test]
    fn hollerith_counts_past_the_record_are_errors_not_panics() {
        for field in [
            "99999999999999999999H,1;",
            "18446744073709551615H,1;",
            "9Hshort;",
        ] {
            assert!(parse_params(field, ',', ';').is_err(), "{field}");
        }
        // A count that ends inside a character, and a global section cut
        // short after its first `1H`, answer rather than panic.
        let _ = parse_params("1H\u{f8},2;", ',', ';');
        let _ = parse_global("1H");
    }

    #[test]
    fn a_deck_without_a_directory_is_refused_by_name() {
        let err = parse("nothing\n").unwrap_err();
        assert!(err.to_string().contains("unknown section letter"), "{err}");
        let mut s = String::new();
        s += &rec("empty", 'S', 1);
        s += &rec("1H,,1H;;", 'G', 1);
        let err = parse(&s).unwrap_err();
        assert!(err.to_string().contains("no directory section"), "{err}");
    }

    #[test]
    fn inch_files_scale_and_unknown_units_refuse() {
        let mut s = tiny_file();
        s = s.replace("1.0,2,2HMM", "1.0,1,2HIN");
        assert_eq!(parse(&s).unwrap().scale_mm(), Some(25.4));
        let mut s = tiny_file();
        // Same length, so the fixed columns survive the substitution.
        s = s.replace("1.0,2,2HMM", "1.0,3,2HXX");
        assert_eq!(parse(&s).unwrap().scale_mm(), None);
    }
}
