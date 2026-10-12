//! Independent stored ZIP carriers and temporary inputs for `FreeCAD` integration controls.

use catwalk3d::Result;
use std::{
    fs,
    io::{Cursor, Read},
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};

/// Unique suffix for directories owned and removed by these integration controls.
static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
/// Bound both original-member decoding and authored archive payloads in test helpers.
const MAX_CONTROL_BYTES: usize = 4 * 1024 * 1024;

/// One explicitly named archive member, independent of production admission logic.
pub(super) struct Member {
    /// Exact ZIP filename, including deliberately invalid names in rejection controls.
    pub(super) name: String,
    /// Uncompressed bytes whose checksum is written by the independent ZIP encoder.
    pub(super) data: Vec<u8>,
}

impl Member {
    /// Construct an authored member without interpreting its filename or payload.
    pub(super) fn new(name: impl Into<String>, data: impl Into<Vec<u8>>) -> Self {
        Self {
            name: name.into(),
            data: data.into(),
        }
    }
}

/// Resolve an original fixture without depending on the process working directory.
pub(super) fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/formats/freecad")
        .join(name)
}

/// Decode the exact named members of one pinned original, with bounded helper reads.
///
/// # Errors
/// Returns an error for unreadable or malformed originals, member codec errors,
/// or decoded members larger than the test helper's four-MiB bound.
pub(super) fn original_members(name: &str) -> Result<Vec<Member>> {
    let bytes = fs::read(fixture(name))?;
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let mut members = Vec::new();
    for index in 0..archive.len() {
        let mut member = archive.by_index(index)?;
        let name = member.name()?.into_owned();
        let mut data = Vec::new();
        Read::by_ref(&mut member)
            .take(u64::try_from(MAX_CONTROL_BYTES)? + 1)
            .read_to_end(&mut data)?;
        if data.len() > MAX_CONTROL_BYTES {
            return Err("original FreeCAD member exceeds the test helper bound".into());
        }
        members.push(Member { name, data });
    }
    Ok(members)
}

/// Read the original directly addressed member without using `FreeCAD` loading code.
///
/// # Errors
/// Returns an error if original members cannot be decoded or the exact name is absent.
pub(super) fn original_member(original: &str, name: &str) -> Result<Vec<u8>> {
    original_members(original)?
        .into_iter()
        .find(|member| member.name == name)
        .map(|member| member.data)
        .ok_or_else(|| format!("original FreeCAD member {name} is absent").into())
}

/// Calculate the stored ZIP CRC-32 directly, without production ZIP-writing helpers.
fn crc32(bytes: &[u8]) -> u32 {
    let mut state = u32::MAX;
    for byte in bytes {
        state ^= u32::from(*byte);
        for _ in 0..8 {
            state = (state >> 1) ^ if state & 1 == 0 { 0 } else { 0xedb8_8320 };
        }
    }
    !state
}

/// Append one fixed-width little-endian field to the independently authored carrier.
fn short(output: &mut Vec<u8>, value: u16) {
    output.extend_from_slice(&value.to_le_bytes());
}

/// Append one fixed-width little-endian field to the independently authored carrier.
fn integer(output: &mut Vec<u8>, value: u32) {
    output.extend_from_slice(&value.to_le_bytes());
}

/// Write a complete stored ZIP, allowing duplicate names for physical-directory controls.
///
/// # Errors
/// Returns an error for oversized payloads, filenames, directory counts or offsets.
pub(super) fn stored(members: &[Member]) -> Result<Vec<u8>> {
    let count = u16::try_from(members.len())?;
    let total = members.iter().try_fold(22_usize, |sum, member| {
        sum.checked_add(member.data.len())?
            .checked_add(member.name.len().checked_mul(2)?)?
            .checked_add(76)
    });
    if total.is_none_or(|size| size > MAX_CONTROL_BYTES) {
        return Err("authored FreeCAD ZIP exceeds the test helper bound".into());
    }
    let mut output = Vec::new();
    let mut directory = Vec::new();
    for member in members {
        let size = u32::try_from(member.data.len())?;
        let name_size = u16::try_from(member.name.len())?;
        let offset = u32::try_from(output.len())?;
        let checksum = crc32(&member.data);
        integer(&mut output, 0x0403_4b50);
        short(&mut output, 20);
        short(&mut output, 0x0800);
        short(&mut output, 0);
        short(&mut output, 0);
        short(&mut output, 0x0021);
        integer(&mut output, checksum);
        integer(&mut output, size);
        integer(&mut output, size);
        short(&mut output, name_size);
        short(&mut output, 0);
        output.extend_from_slice(member.name.as_bytes());
        output.extend_from_slice(&member.data);

        integer(&mut directory, 0x0201_4b50);
        short(&mut directory, 20);
        short(&mut directory, 20);
        short(&mut directory, 0x0800);
        short(&mut directory, 0);
        short(&mut directory, 0);
        short(&mut directory, 0x0021);
        integer(&mut directory, checksum);
        integer(&mut directory, size);
        integer(&mut directory, size);
        short(&mut directory, name_size);
        short(&mut directory, 0);
        short(&mut directory, 0);
        short(&mut directory, 0);
        short(&mut directory, 0);
        integer(&mut directory, 0);
        integer(&mut directory, offset);
        directory.extend_from_slice(member.name.as_bytes());
    }
    let directory_offset = u32::try_from(output.len())?;
    let directory_size = u32::try_from(directory.len())?;
    output.extend_from_slice(&directory);
    integer(&mut output, 0x0605_4b50);
    short(&mut output, 0);
    short(&mut output, 0);
    short(&mut output, count);
    short(&mut output, count);
    integer(&mut output, directory_size);
    integer(&mut output, directory_offset);
    short(&mut output, 0);
    Ok(output)
}

/// An exclusively owned temporary directory containing one authored public-API input.
pub(super) struct Control {
    /// Unique directory removed when this control leaves scope.
    directory: PathBuf,
    /// Full path passed to the ordinary model or thumbnail API.
    pub(super) path: PathBuf,
}

impl Control {
    /// Write bounded authored input bytes under a unique process-local directory.
    ///
    /// # Errors
    /// Returns an error for an invalid extension or temporary filesystem failure.
    pub(super) fn new(bytes: &[u8], extension: &str) -> Result<Self> {
        if extension.is_empty() || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
            return Err("invalid authored FreeCAD control extension".into());
        }
        let directory = std::env::temp_dir().join(format!(
            "catwalk-freecad-control-{}-{}",
            std::process::id(),
            NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed),
        ));
        fs::create_dir(&directory)?;
        let path = directory.join(format!("control.{extension}"));
        fs::write(&path, bytes)?;
        Ok(Self { directory, path })
    }
}

impl Drop for Control {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}
