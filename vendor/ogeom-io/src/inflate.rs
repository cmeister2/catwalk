//! Inflate: the decoder half of DEFLATE, the codec inside every ZIP.
//!
//! A package a slicer writes is deflated, entry by entry, so reading one
//! needs this and nothing more of compression: stored, fixed-Huffman and
//! dynamic-Huffman blocks, a 32 KiB window, and no encoder. Each Huffman
//! code is decoded through one flat table indexed by the next bits of the
//! stream, as wide as the longest code the block uses, which keeps a
//! multi-megabyte model part to a table lookup per symbol.

use ogeom_core::{OgeomResult, ogeom_bail};

/// Bits read least significant first, as DEFLATE packs them.
struct Bits<'a> {
    bytes: &'a [u8],
    at: usize,
    held: u64,
    count: u32,
}

impl<'a> Bits<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self {
            bytes,
            at: 0,
            held: 0,
            count: 0,
        }
    }

    /// Top the buffer up to at least `n` bits where the stream has them;
    /// past its end the buffer reads zeros, and `consume` says so.
    fn fill(&mut self, n: u32) {
        while self.count < n {
            let byte = self.bytes.get(self.at).copied().unwrap_or(0);
            self.at += 1;
            self.held |= u64::from(byte) << self.count;
            self.count += 8;
        }
    }

    fn peek(&mut self, n: u32) -> u64 {
        self.fill(n);
        self.held & ((1_u64 << n) - 1)
    }

    fn consume(&mut self, n: u32) -> OgeomResult<()> {
        self.held >>= n;
        self.count -= n;
        // Bytes still buffered are not yet read; only bits taken from past
        // the end of the stream make it truncated.
        if self.at * 8 - self.count as usize > self.bytes.len() * 8 {
            ogeom_bail!(Construction, "a deflated stream ends part-way through");
        }
        Ok(())
    }

    fn take(&mut self, n: u32) -> OgeomResult<u32> {
        if n == 0 {
            return Ok(0);
        }
        let value = self.peek(n);
        self.consume(n)?;
        Ok(u32::try_from(value).unwrap_or(u32::MAX))
    }

    /// Drop to the next byte boundary, as a stored block starts on one.
    fn align(&mut self) {
        let spare = self.count % 8;
        self.held >>= spare;
        self.count -= spare;
    }
}

/// A canonical Huffman code as a flat lookup: indexed by the next `width`
/// bits of the stream, each slot holds the symbol and its code's length.
struct Code {
    table: Vec<(u16, u8)>,
    width: u32,
}

impl Code {
    /// The code whose symbol `s` has length `lengths[s]`, zero for unused.
    fn new(lengths: &[u8]) -> OgeomResult<Self> {
        let width = u32::from(lengths.iter().copied().max().unwrap_or(0));
        if width == 0 {
            // A block may declare no distance codes at all, when it has
            // only literals; any use of the code is then an error.
            return Ok(Self {
                table: Vec::new(),
                width: 0,
            });
        }
        let mut count = [0_u32; 16];
        for &l in lengths {
            count[usize::from(l)] += 1;
        }
        count[0] = 0;
        let mut next = [0_u32; 16];
        let mut code = 0_u32;
        for bits in 1..16 {
            code = (code + count[bits - 1]) << 1;
            next[bits] = code;
        }
        let mut table = vec![(0_u16, 0_u8); 1 << width];
        for (symbol, &length) in lengths.iter().enumerate() {
            if length == 0 {
                continue;
            }
            let len = u32::from(length);
            let assigned = next[usize::from(length)];
            next[usize::from(length)] += 1;
            if assigned >= 1 << len {
                ogeom_bail!(
                    Construction,
                    "a deflated block declares an oversubscribed code"
                );
            }
            // Codes are packed most significant bit first; the stream is
            // read least significant first, so the index is the reversal.
            let reversed = (assigned.reverse_bits() >> (32 - len)) as usize;
            let symbol = u16::try_from(symbol).unwrap_or(u16::MAX);
            let mut slot = reversed;
            while slot < table.len() {
                table[slot] = (symbol, length);
                slot += 1 << len;
            }
        }
        Ok(Self { table, width })
    }

    fn decode(&self, bits: &mut Bits<'_>) -> OgeomResult<u16> {
        if self.width == 0 {
            ogeom_bail!(
                Construction,
                "a deflated block uses a code it declared empty"
            );
        }
        let index = usize::try_from(bits.peek(self.width)).unwrap_or(0);
        let (symbol, length) = self.table[index];
        if length == 0 {
            ogeom_bail!(Construction, "a deflated stream holds a code no symbol has");
        }
        bits.consume(u32::from(length))?;
        Ok(symbol)
    }
}

const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DISTANCE_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DISTANCE_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];
/// The order the code-length code's lengths are stored in.
const CODE_LENGTH_ORDER: [usize; 19] = [
    16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
];

/// Inflate a raw DEFLATE stream, as a ZIP entry holds it: no zlib or gzip
/// wrapper. `expected` is the size the archive promises, which sizes the
/// output and checks it.
///
/// # Errors
///
/// [`OgeomError::Construction`](ogeom_core::OgeomError::Construction) for a
/// stream that is malformed, truncated, or reaches back before its start.
pub(crate) fn inflate(bytes: &[u8], expected: usize) -> OgeomResult<Vec<u8>> {
    // DEFLATE expands at most 1032 to 1, so no honest entry inflates past
    // that; a promise beyond it reserves nothing it cannot back.
    let mut out: Vec<u8> = Vec::with_capacity(expected.min(bytes.len().saturating_mul(1032)));
    let mut bits = Bits::new(bytes);
    loop {
        let last = bits.take(1)? == 1;
        match bits.take(2)? {
            0 => stored(&mut bits, &mut out, expected)?,
            1 => {
                let (literals, distances) = fixed_codes()?;
                compressed(&mut bits, &mut out, &literals, &distances, expected)?;
            }
            2 => {
                let (literals, distances) = dynamic_codes(&mut bits)?;
                compressed(&mut bits, &mut out, &literals, &distances, expected)?;
            }
            _ => ogeom_bail!(Construction, "a deflated block has the reserved type"),
        }
        if last {
            break;
        }
    }
    if out.len() != expected {
        ogeom_bail!(
            Construction,
            "a deflated stream inflates to {} bytes where its archive says {expected}",
            out.len()
        );
    }
    Ok(out)
}

/// Refuse a stream the moment it inflates past what its archive promised,
/// rather than after inflating all of a bomb.
fn within(out: &[u8], expected: usize) -> OgeomResult<()> {
    if out.len() > expected {
        ogeom_bail!(
            Construction,
            "a deflated stream inflates past the {expected} bytes its archive says"
        );
    }
    Ok(())
}

fn stored(bits: &mut Bits<'_>, out: &mut Vec<u8>, expected: usize) -> OgeomResult<()> {
    bits.align();
    let length = bits.take(16)?;
    let complement = bits.take(16)?;
    if length != !complement & 0xFFFF {
        ogeom_bail!(
            Construction,
            "a stored block's length and its check disagree"
        );
    }
    for _ in 0..length {
        out.push(u8::try_from(bits.take(8)?).unwrap_or(0));
    }
    within(out, expected)
}

fn fixed_codes() -> OgeomResult<(Code, Code)> {
    let mut lengths = [0_u8; 288];
    lengths[..144].fill(8);
    lengths[144..256].fill(9);
    lengths[256..280].fill(7);
    lengths[280..].fill(8);
    Ok((Code::new(&lengths)?, Code::new(&[5; 30])?))
}

fn dynamic_codes(bits: &mut Bits<'_>) -> OgeomResult<(Code, Code)> {
    let literal_count = bits.take(5)? as usize + 257;
    let distance_count = bits.take(5)? as usize + 1;
    let length_count = bits.take(4)? as usize + 4;
    let mut code_lengths = [0_u8; 19];
    for &slot in &CODE_LENGTH_ORDER[..length_count] {
        code_lengths[slot] = u8::try_from(bits.take(3)?).unwrap_or(0);
    }
    let lengths_code = Code::new(&code_lengths)?;
    let mut lengths = vec![0_u8; literal_count + distance_count];
    let mut i = 0;
    while i < lengths.len() {
        let symbol = lengths_code.decode(bits)?;
        let (value, repeat) = match symbol {
            0..=15 => (u8::try_from(symbol).unwrap_or(0), 1),
            16 => {
                if i == 0 {
                    ogeom_bail!(Construction, "a deflated block repeats a length before any");
                }
                (lengths[i - 1], 3 + bits.take(2)? as usize)
            }
            17 => (0, 3 + bits.take(3)? as usize),
            18 => (0, 11 + bits.take(7)? as usize),
            _ => ogeom_bail!(Construction, "a deflated block has a bad code length"),
        };
        if i + repeat > lengths.len() {
            ogeom_bail!(Construction, "a deflated block's code lengths overrun");
        }
        lengths[i..i + repeat].fill(value);
        i += repeat;
    }
    if lengths[256] == 0 {
        ogeom_bail!(Construction, "a deflated block has no end-of-block code");
    }
    Ok((
        Code::new(&lengths[..literal_count])?,
        Code::new(&lengths[literal_count..])?,
    ))
}

fn compressed(
    bits: &mut Bits<'_>,
    out: &mut Vec<u8>,
    literals: &Code,
    distances: &Code,
    expected: usize,
) -> OgeomResult<()> {
    loop {
        within(out, expected)?;
        let symbol = literals.decode(bits)?;
        match symbol {
            0..=255 => out.push(u8::try_from(symbol).unwrap_or(0)),
            256 => return Ok(()),
            257..=285 => {
                let k = usize::from(symbol - 257);
                let length =
                    usize::from(LENGTH_BASE[k]) + bits.take(u32::from(LENGTH_EXTRA[k]))? as usize;
                let d = usize::from(distances.decode(bits)?);
                if d >= 30 {
                    ogeom_bail!(
                        Construction,
                        "a deflated stream names a distance code past 29"
                    );
                }
                let distance = usize::from(DISTANCE_BASE[d])
                    + bits.take(u32::from(DISTANCE_EXTRA[d]))? as usize;
                if distance > out.len() {
                    ogeom_bail!(
                        Construction,
                        "a deflated stream reaches back before its start"
                    );
                }
                let from = out.len() - distance;
                if distance >= length {
                    out.extend_from_within(from..from + length);
                } else {
                    // An overlapping copy repeats what it is writing.
                    for j in 0..length {
                        out.push(out[from + j]);
                    }
                }
            }
            _ => ogeom_bail!(
                Construction,
                "a deflated stream holds a length code past 285"
            ),
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, reason = "test code")]
    use super::inflate;

    const TEXT: &[u8] = b"a stored block, then the same again: a stored block";

    #[test]
    fn a_stored_block_is_copied() {
        let stream = [
            1, 51, 0, 204, 255, 97, 32, 115, 116, 111, 114, 101, 100, 32, 98, 108, 111, 99, 107,
            44, 32, 116, 104, 101, 110, 32, 116, 104, 101, 32, 115, 97, 109, 101, 32, 97, 103, 97,
            105, 110, 58, 32, 97, 32, 115, 116, 111, 114, 101, 100, 32, 98, 108, 111, 99, 107,
        ];
        assert_eq!(inflate(&stream, TEXT.len()).unwrap(), TEXT);
    }

    #[test]
    fn a_fixed_code_block_reaches_back_for_its_repeat() {
        let stream = [
            75, 84, 40, 46, 201, 47, 74, 77, 81, 72, 202, 201, 79, 206, 214, 81, 40, 201, 72, 205,
            3, 17, 10, 197, 137, 185, 169, 10, 137, 233, 137, 153, 121, 86, 10, 137, 40, 138, 0,
        ];
        assert_eq!(inflate(&stream, TEXT.len()).unwrap(), TEXT);
        // Cut short, it is refused rather than padded.
        assert!(inflate(&stream[..30], TEXT.len()).is_err());
    }

    /// A stream that inflates past the size its archive promises is
    /// refused, whatever it promised: a hundred bytes where ten were said.
    #[test]
    fn a_stream_longer_than_its_promise_is_refused() {
        let mut stream = vec![0x01, 100, 0, !100, 0xFF];
        stream.extend(std::iter::repeat_n(b'x', 100));
        assert!(inflate(&stream, 10).is_err());
        assert_eq!(inflate(&stream, 100).unwrap().len(), 100);
        // A promise of more than any stream this short could inflate to
        // reserves nothing it cannot back, and the length check refuses it.
        assert!(inflate(&stream, usize::MAX).is_err());
    }
}
