//! Small authored IGES records built in memory for library unit tests.

use std::fmt::Write;

/// Construct millimetre global records without a repository fixture dependency.
pub(super) fn header() -> Vec<String> {
    let global = concat!(
        "1H,,1H;,7HCatwalk,9Hunit.iges,7HCatwalk,7HCatwalk,32,308,15,308,15,",
        "7HCatwalk,1.,2,2HMM,1,0.01,15H20261012.000000,1.E-7,0.,7HCatwalk,",
        "7HCatwalk,11,0,15H20261012.000000;"
    );
    let mut records = vec![format!(
        "{:<72}S      1",
        "Catwalk authored IGES unit input"
    )];
    for (index, chunk) in global.as_bytes().chunks(72).enumerate() {
        records.push(format!(
            "{:<72}G{:7}",
            String::from_utf8_lossy(chunk),
            index + 1
        ));
    }
    records
}

/// Construct one fixed-column point for delegation and numeric-spelling checks.
///
/// # Errors
/// Returns formatting errors while encoding fixed-column directory records.
pub(super) fn point() -> crate::Result<String> {
    let mut records = header();
    let global_count = records.len() - 1;
    for (fields, id) in [
        ([116, 1, 0, 0, 0, 0, 0, 0, 10_000], 1),
        ([116, 0, 0, 1, 0, 0, 0, 0, 0], 2),
    ] {
        let mut payload = String::new();
        for value in fields {
            write!(payload, "{value:8}")?;
        }
        records.push(format!("{payload}D{id:7}"));
    }
    records.push(format!("{:<64}{:8}P      1", "116,3.25,-1.5,2.,0;", 1));
    records.push(format!(
        "{:<72}T      1",
        format!("S{:7}G{global_count:7}D{:7}P{:7}", 1, 2, 1)
    ));
    Ok(records.join("\n"))
}
