use super::{Result, bytes, check_time, u16_le, u32_le};
use std::{
    collections::HashSet,
    fs::File,
    io::{self, Read, Seek, SeekFrom},
    time::Instant,
};

const FREE: u32 = u32::MAX;
const END: u32 = u32::MAX - 1;
const MAX_DIRECTORY: usize = 4 * 1024 * 1024;

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
pub(super) fn preflight(source: &mut BudgetFile, source_size: u64) -> Result<bool> {
    let mut header = [0; 512];
    source.read_exact(&mut header)?;
    if &header[..8] != b"\xd0\xcf\x11\xe0\xa1\xb1\x1a\xe1" {
        return Ok(false);
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
        .chunks_exact(4)
        .map(|value| u32::from_le_bytes(value.try_into().unwrap()))
        .filter(|&id| id != FREE)
        .collect::<Vec<_>>();
    let mut difat = u32_le(&header, 68)?;
    let mut seen = HashSet::new();
    for _ in 0..difat_count {
        if !seen.insert(difat) {
            return Err("cyclic Inventor DIFAT".into());
        }
        let data = sector(source, difat, sector_count, sector_size)?;
        for value in data[..sector_size - 4].chunks_exact(4) {
            let id = u32::from_le_bytes(value.try_into()?);
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
            data.chunks_exact(4)
                .map(|value| u32::from_le_bytes(value.try_into().unwrap())),
        );
    }
    let mut directory = u32_le(&header, 48)?;
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
            let root_size = if major == 3 {
                u64::from(u32_le(&data, 120)?)
            } else {
                u64::from_le_bytes(bytes(&data, 120, 8)?.try_into()?)
            };
            if root_size > 64 * 1024 * 1024 {
                return Err("Inventor root mini-stream exceeds the 64 MiB limit".into());
            }
        }
        directory = *fat
            .get(directory as usize)
            .ok_or("Inventor directory points outside FAT")?;
    }
    // Permissive cfb opening reads the actual miniFAT chain, independently of
    // its declared length. Bound that chain before its entry vector allocates.
    let mut mini = u32_le(&header, 60)?;
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
        mini = *fat
            .get(mini as usize)
            .ok_or("Inventor miniFAT points outside FAT")?;
    }
    if count != mini_count {
        return Err("Inventor miniFAT chain length does not match its header".into());
    }
    Ok(true)
}
