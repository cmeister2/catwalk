//! Bounded FCStd ZIP members and schema-four document identity.

use crate::Result;
use roxmltree::{Document, ParsingOptions};
use std::{collections::BTreeMap, fs::File, io::Read, path::Path};

/// Maximum encoded `FreeCAD` document accepted before ZIP directory allocation.
const MAX_SOURCE: u64 = 64 * 1024 * 1024;
/// Maximum archive directory records, including directory entries.
const MAX_ENTRIES: usize = 16_384;
/// Maximum decoded document or GUI XML payload.
pub(crate) const MAX_XML: usize = 16 * 1024 * 1024;
/// Maximum cumulative decoded member bytes admitted by one container.
const MAX_DECODED: usize = 128 * 1024 * 1024;
/// Maximum XML nodes admitted before DOM allocation.
const MAX_NODES: u32 = 1_000_000;
/// Maximum nested XML elements.
const MAX_DEPTH: usize = 64;
/// Maximum individual XML name or attribute payload.
const MAX_STRING: usize = 1024 * 1024;

/// File-backed archive with unambiguous addressed members and validated identity.
pub(crate) struct DocumentArchive {
    /// Opened ZIP reader retaining the source file handle.
    archive: zip::ZipArchive<File>,
    /// Exact canonical member names mapped to physical entry indices.
    entries: BTreeMap<String, usize>,
    /// Bounded UTF-8 schema-four document metadata.
    document: String,
    /// Cumulative admitted decoded bytes, including document metadata.
    decoded: usize,
}

impl DocumentArchive {
    /// Open an `FCStd` archive and validate its bounded root document identity.
    ///
    /// # Errors
    ///
    /// Source size, ZIP framing, member ownership, XML identity or resource limits are invalid.
    pub(crate) fn open(path: &Path) -> Result<Self> {
        let mut file = File::open(path)?;
        let length = file.metadata()?.len();
        if length > MAX_SOURCE {
            return Err("FCStd source exceeds the 64 MiB limit".into());
        }
        let physical = super::zip_directory::validate(&mut file, length, MAX_ENTRIES)?;
        let mut archive = zip::ZipArchive::new(file)?;
        if archive.len() != physical {
            return Err("FCStd duplicate decoded ZIP member name".into());
        }
        let mut entries = BTreeMap::new();
        for index in 0..archive.len() {
            let entry = archive.by_index(index)?;
            let name = entry.name()?;
            let canonical = member_name(&name, entry.is_dir())?;
            if entries.insert(canonical, index).is_some() {
                return Err("FCStd duplicate canonical ZIP member name".into());
            }
        }
        let mut result = Self {
            archive,
            entries,
            document: String::new(),
            decoded: 0,
        };
        let bytes = result.read("Document.xml", MAX_XML)?;
        result.document = String::from_utf8(bytes)?;
        let document = xml(&result.document)?;
        let root = document.root_element();
        if root.tag_name().name() != "Document"
            || root.tag_name().namespace().is_some()
            || root.attribute("SchemaVersion") != Some("4")
            || root.attribute("FileVersion") != Some("1")
        {
            return Err("FCStd requires Document SchemaVersion=4 and FileVersion=1".into());
        }
        Ok(result)
    }

    /// Borrow the original validated document XML without re-reading its member.
    pub(crate) fn document(&self) -> &str {
        &self.document
    }

    /// Decode one exact addressed member while bounding its declared and actual size.
    ///
    /// # Errors
    ///
    /// The member is absent, encrypted, unsupported, corrupt or exceeds decoded-byte limits.
    pub(crate) fn read(&mut self, name: &str, limit: usize) -> Result<Vec<u8>> {
        self.read_optional(name, limit)?
            .ok_or_else(|| format!("FCStd addressed member is missing: {name}").into())
    }

    /// Decode an addressed optional member, returning absence without substituting another file.
    ///
    /// # Errors
    ///
    /// Invalid addressing, ZIP decoding, CRC or declared/actual decoded-byte bounds fail.
    pub(crate) fn read_optional(&mut self, name: &str, limit: usize) -> Result<Option<Vec<u8>>> {
        member_name(name, false)?;
        let Some(&index) = self.entries.get(name) else {
            return Ok(None);
        };
        let mut entry = self.archive.by_index(index)?;
        if entry.is_dir() {
            return Err("FCStd addressed member is a directory".into());
        }
        if !matches!(
            entry.compression(),
            zip::CompressionMethod::Stored | zip::CompressionMethod::Deflated
        ) {
            return Err("FCStd selected ZIP member compression is unsupported".into());
        }
        let limit = limit.min(MAX_DECODED.saturating_sub(self.decoded));
        if entry.size() > u64::try_from(limit)? {
            return Err("FCStd selected member exceeds decoded-byte limit".into());
        }
        let capacity = usize::try_from(entry.size())?;
        let mut bytes = Vec::with_capacity(capacity);
        entry
            .by_ref()
            .take(u64::try_from(limit)? + 1)
            .read_to_end(&mut bytes)?;
        if bytes.len() > limit || bytes.len() != capacity {
            return Err("FCStd selected member has invalid decoded size".into());
        }
        self.decoded = self
            .decoded
            .checked_add(bytes.len())
            .ok_or("FCStd decoded-byte count overflow")?;
        Ok(Some(bytes))
    }
}

/// Validate exact ZIP member paths without URI decoding or filesystem extraction.
///
/// # Errors
///
/// Empty, absolute, traversal, alternate-separator or ambiguous names are rejected.
fn member_name(name: &str, directory: bool) -> Result<String> {
    let canonical = if directory {
        name.strip_suffix('/').unwrap_or(name)
    } else {
        name
    };
    if canonical.is_empty()
        || canonical.len() > 4096
        || canonical.contains(['\\', '\0', ':'])
        || canonical
            .split('/')
            .any(|part| part.is_empty() || matches!(part, "." | ".."))
    {
        return Err("FCStd ZIP member has an invalid or ambiguous path".into());
    }
    Ok(canonical.to_owned())
}

/// Parse bounded document XML with DTD resolution disabled and limited DOM allocation.
///
/// # Errors
///
/// XML bytes, nodes, nesting, namespaces or attribute/name lengths exceed the qualified grammar.
pub(crate) fn xml(text: &str) -> Result<Document<'_>> {
    if text.len() > MAX_XML {
        return Err("FCStd XML exceeds the 16 MiB limit".into());
    }
    let document = Document::parse_with_options(
        text,
        ParsingOptions {
            allow_dtd: false,
            nodes_limit: MAX_NODES,
            ..ParsingOptions::default()
        },
    )
    .map_err(|error| format!("FCStd native XML document is invalid: {error}"))?;
    for node in document.descendants().filter(roxmltree::Node::is_element) {
        if node.tag_name().namespace().is_some()
            || node
                .attributes()
                .any(|attribute| attribute.namespace().is_some())
        {
            return Err("FCStd namespaced native XML carriers are unsupported".into());
        }
        if node.ancestors().take(MAX_DEPTH + 2).count() > MAX_DEPTH + 1
            || node.tag_name().name().len() > MAX_STRING
            || node.attributes().len() > 64
            || node.attributes().any(|attribute| {
                attribute.name().len() > MAX_STRING || attribute.value().len() > MAX_STRING
            })
        {
            return Err("FCStd XML exceeds nesting or attribute limits".into());
        }
    }
    Ok(document)
}
