use super::{Result, bytes, check_time, u16_le, u32_le};
use std::{
    collections::{HashMap, HashSet},
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    time::Instant,
};

const FREE: u32 = u32::MAX;
const END: u32 = u32::MAX - 1;
const MAX_DIRECTORY: usize = 4 * 1024 * 1024;

pub(super) struct Metadata {
    fat: Vec<u32>,
    mini_fat: Vec<u32>,
    streams: HashMap<String, (u32, u64)>,
    root_start: u32,
    root_size: u64,
    sector_size: usize,
    sector_count: usize,
}

impl Metadata {
    fn chain(&self, start: u32, size: u64, mini: bool, limit: u64) -> Result<()> {
        let unit = if mini { 64 } else { self.sector_size as u64 };
        let table = if mini { &self.mini_fat } else { &self.fat };
        let mut seen = HashSet::new();
        let mut current = start;
        let mut count = 0_u64;
        while current != END {
            if !seen.insert(current) {
                return Err("cyclic Inventor selected-stream sector chain".into());
            }
            count += 1;
            if count * unit > limit + unit - 1 {
                return Err("Inventor selected-stream chain exceeds its resource limit".into());
            }
            if mini {
                if (u64::from(current) + 1) * 64 > self.root_size {
                    return Err("Inventor mini sector lies outside the root mini-stream".into());
                }
            } else if current as usize >= self.sector_count {
                return Err("Inventor selected-stream sector lies outside the source".into());
            }
            current = *table
                .get(current as usize)
                .ok_or("Inventor selected-stream sector lies outside its FAT")?;
        }
        if count != size.div_ceil(unit) {
            return Err(
                "Inventor selected-stream chain length does not match its declared size".into(),
            );
        }
        Ok(())
    }

    pub(super) fn validate_stream(&self, name: &str, size: u64) -> Result<()> {
        let &(start, original_size) = self
            .streams
            .get(name)
            .ok_or("Inventor summary is absent from original root metadata")?;
        if original_size != size {
            return Err("Inventor summary stream metadata changed during parsing".into());
        }
        if size == 0 {
            return Ok(());
        }
        if size < 4096 {
            self.chain(self.root_start, self.root_size, false, 64 * 1024 * 1024)?;
            self.chain(start, size, true, super::MAX_BYTES as u64)
        } else {
            self.chain(start, size, false, super::MAX_BYTES as u64)
        }
    }
}

pub(super) struct BudgetFile {
    source: File,
    size: u64,
    started: Instant,
}

impl BudgetFile {
    pub(super) fn new(source: File, size: u64, started: Instant) -> Self {
        Self {
            source,
            size,
            started,
        }
    }
    fn check(&self) -> io::Result<()> {
        check_time(self.started).map_err(|error| io::Error::other(error.to_string()))
    }
}

impl Read for BudgetFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        self.check()?;
        self.source.read(buffer)
    }
}

impl Seek for BudgetFile {
    fn seek(&mut self, offset: SeekFrom) -> io::Result<u64> {
        self.check()?;
        let position = self.source.seek(offset)?;
        if position > self.size {
            return Err(io::Error::other("Inventor seek outside source"));
        }
        Ok(position)
    }
}

fn sector(source: &mut BudgetFile, id: u32, count: usize, size: usize) -> Result<Vec<u8>> {
    if id as usize >= count {
        return Err("invalid Inventor compound sector index".into());
    }
    source.seek(SeekFrom::Start((u64::from(id) + 1) * size as u64))?;
    let mut output = vec![0; size];
    source.read_exact(&mut output)?;
    Ok(output)
}

// Bound metadata before cfb allocates its complete FAT/directory tables. It then
// performs the standard compound-file structural checks during opening.
pub(super) fn preflight(source: &mut BudgetFile, source_size: u64) -> Result<Option<Metadata>> {
    let mut header = [0; 512];
    source.read_exact(&mut header)?;
    if &header[..8] != b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1" {
        return Ok(None);
    }
    let major = u16_le(&header, 26)?;
    let shift = u16_le(&header, 30)?;
    if u16_le(&header, 28)? != 0xfffe
        || !matches!((major, shift), (3, 9) | (4, 12))
        || u16_le(&header, 32)? != 6
    {
        return Err("invalid Inventor compound-file header".into());
    }
    let sector_size = 1_usize << shift;
    if !source_size.is_multiple_of(sector_size as u64) || source_size < 2 * sector_size as u64 {
        return Err("truncated Inventor compound file".into());
    }
    let sector_count = source_size as usize / sector_size - 1;
    if u32_le(&header, 56)? != 4096 {
        return Err("unsupported Inventor mini-stream cutoff".into());
    }
    let fat_count = u32_le(&header, 44)? as usize;
    let difat_count = u32_le(&header, 72)? as usize;
    let mini_count = u32_le(&header, 64)? as usize;
    if fat_count == 0 || fat_count > sector_count || fat_count * sector_size > 8 * 1024 * 1024 {
        return Err("Inventor FAT metadata exceeds the extraction limit".into());
    }
    if difat_count > sector_count
        || difat_count * sector_size > MAX_DIRECTORY
        || mini_count * sector_size > MAX_DIRECTORY
    {
        return Err("Inventor DIFAT or miniFAT exceeds the 4 MiB limit".into());
    }
    let mut fat_sectors = header[76..512]
        .as_chunks::<4>()
        .0
        .iter()
        .map(|value| u32::from_le_bytes(*value))
        .filter(|&id| id != FREE)
        .collect::<Vec<_>>();
    let mut difat = u32_le(&header, 68)?;
    let mut seen = HashSet::new();
    for _ in 0..difat_count {
        if !seen.insert(difat) {
            return Err("cyclic Inventor DIFAT".into());
        }
        let data = sector(source, difat, sector_count, sector_size)?;
        for value in data[..sector_size - 4].as_chunks::<4>().0.iter() {
            let id = u32::from_le_bytes(*value);
            if id != FREE {
                fat_sectors.push(id);
            }
        }
        if fat_sectors.len() > fat_count {
            return Err("Inventor DIFAT lists excess FAT sectors".into());
        }
        difat = u32_le(&data, sector_size - 4)?;
    }
    if ![END, FREE].contains(&difat)
        || fat_sectors.len() != fat_count
        || fat_sectors.iter().collect::<HashSet<_>>().len() != fat_count
    {
        return Err("invalid Inventor FAT sector list".into());
    }
    if fat_count * (sector_size / 4) < sector_count {
        return Err("Inventor FAT cannot address the source".into());
    }
    let mut fat = Vec::with_capacity(fat_count * sector_size / 4);
    for id in fat_sectors {
        let data = sector(source, id, sector_count, sector_size)?;
        fat.extend(
            data.as_chunks::<4>()
                .0
                .iter()
                .map(|value| u32::from_le_bytes(*value)),
        );
    }
    let mut directory = u32_le(&header, 48)?;
    let mut directory_bytes = Vec::new();
    let mut root_start = END;
    let mut root_size = 0;
    let mut seen = HashSet::new();
    let mut count = 0;
    while directory != END {
        if !seen.insert(directory) {
            return Err("cyclic Inventor directory sector chain".into());
        }
        count += 1;
        if count * sector_size > MAX_DIRECTORY {
            return Err("Inventor directory exceeds the 4 MiB limit".into());
        }
        let data = sector(source, directory, sector_count, sector_size)?;
        if count == 1 {
            if data[66] != 5 {
                return Err("Inventor compound file has no root storage".into());
            }
            root_start = u32_le(&data, 116)?;
            root_size = if major == 3 {
                u64::from(u32_le(&data, 120)?)
            } else {
                u64::from_le_bytes(bytes(&data, 120, 8)?.try_into()?)
            };
            if root_size > 64 * 1024 * 1024 {
                return Err("Inventor root mini-stream exceeds the 64 MiB limit".into());
            }
        }
        directory_bytes.extend_from_slice(&data);
        directory = *fat
            .get(directory as usize)
            .ok_or("Inventor directory points outside FAT")?;
    }
    // Permissive cfb opening reads the actual miniFAT chain, independently of
    // its declared length. Bound that chain before its entry vector allocates.
    let mut mini = u32_le(&header, 60)?;
    let mut mini_fat = Vec::new();
    let mut seen = HashSet::new();
    let mut count = 0;
    while mini != END {
        if !seen.insert(mini) {
            return Err("cyclic Inventor miniFAT chain".into());
        }
        count += 1;
        if count * sector_size > MAX_DIRECTORY {
            return Err("Inventor actual miniFAT exceeds the 4 MiB limit".into());
        }
        if mini as usize >= sector_count {
            return Err("Inventor miniFAT points outside the source".into());
        }
        let data = sector(source, mini, sector_count, sector_size)?;
        mini_fat.extend(
            data.as_chunks::<4>()
                .0
                .iter()
                .map(|value| u32::from_le_bytes(*value)),
        );
        mini = *fat
            .get(mini as usize)
            .ok_or("Inventor miniFAT points outside FAT")?;
    }
    if count != mini_count {
        return Err("Inventor miniFAT chain length does not match its header".into());
    }
    let mut streams = HashMap::new();
    let mut pending = vec![u32_le(&directory_bytes, 76)?];
    let mut seen = HashSet::new();
    while let Some(id) = pending.pop() {
        if id == FREE {
            continue;
        }
        if !seen.insert(id) {
            return Err("cyclic Inventor root directory tree".into());
        }
        let entry = bytes(&directory_bytes, id as usize * 128, 128)?;
        if ![1, 2].contains(&entry[66]) {
            return Err("invalid Inventor root directory child".into());
        }
        pending.push(u32_le(entry, 68)?);
        pending.push(u32_le(entry, 72)?);
        if entry[66] != 2 {
            continue;
        }
        let name_size = usize::from(u16_le(entry, 64)?);
        if !(2..=64).contains(&name_size)
            || !name_size.is_multiple_of(2)
            || bytes(entry, name_size - 2, 2)? != [0, 0]
        {
            return Err("invalid Inventor directory name".into());
        }
        let characters = bytes(entry, 0, name_size - 2)?
            .as_chunks::<2>()
            .0
            .iter()
            .map(|value| u16::from_le_bytes(*value))
            .collect::<Vec<_>>();
        let name = String::from_utf16(&characters)?;
        let size = if major == 3 {
            u64::from(u32_le(entry, 120)?)
        } else {
            u64::from_le_bytes(bytes(entry, 120, 8)?.try_into()?)
        };
        streams.insert(name, (u32_le(entry, 116)?, size));
    }
    Ok(Some(Metadata {
        fat,
        mini_fat,
        streams,
        root_start,
        root_size,
        sector_size,
        sector_count,
    }))
}
