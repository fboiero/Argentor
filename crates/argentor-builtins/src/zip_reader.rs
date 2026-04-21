//! Minimal ZIP archive reader for Office Open XML loaders.
//!
//! This is a pragmatic, dependency-free implementation that handles the
//! subset of ZIP features used by `.docx`, `.xlsx`, `.pptx`, and `.epub`
//! files: stored (no compression) and DEFLATE-compressed entries using
//! the central directory.
//!
//! It is NOT a general-purpose ZIP library — no encryption, no ZIP64 (unless
//! files are small), no multi-disk archives. For production workloads with
//! large archives, a dedicated crate like `zip` is recommended.
//!
//! The implementation uses only stdlib APIs and no external deps.

use std::collections::HashMap;

/// A single entry from the ZIP central directory.
#[derive(Debug, Clone)]
pub struct ZipEntry {
    /// Path inside the archive.
    pub name: String,
    /// Compression method (0=stored, 8=deflate).
    pub method: u16,
    /// Compressed size in bytes.
    pub compressed_size: u32,
    /// Uncompressed size in bytes.
    pub uncompressed_size: u32,
    /// Offset in the archive of the local file header.
    pub local_header_offset: u32,
}

/// Read the central directory and return a map from path -> `ZipEntry`.
pub fn read_central_directory(bytes: &[u8]) -> Result<HashMap<String, ZipEntry>, String> {
    // Find the End of Central Directory (EOCD) record.
    // Signature: 0x06054b50 ("PK\x05\x06")
    let eocd_sig: [u8; 4] = [0x50, 0x4b, 0x05, 0x06];
    let eocd_offset = find_last_signature(bytes, &eocd_sig)
        .ok_or_else(|| "EOCD signature not found".to_string())?;

    if bytes.len() < eocd_offset + 22 {
        return Err("Truncated EOCD record".to_string());
    }

    let total_entries = u16::from_le_bytes([bytes[eocd_offset + 10], bytes[eocd_offset + 11]]);
    let cd_offset = u32::from_le_bytes([
        bytes[eocd_offset + 16],
        bytes[eocd_offset + 17],
        bytes[eocd_offset + 18],
        bytes[eocd_offset + 19],
    ]) as usize;

    let mut entries = HashMap::new();
    let mut cursor = cd_offset;

    for _ in 0..total_entries {
        if cursor + 46 > bytes.len() {
            return Err("Truncated central directory entry".to_string());
        }
        // Central directory header signature: 0x02014b50
        let sig = u32::from_le_bytes([
            bytes[cursor],
            bytes[cursor + 1],
            bytes[cursor + 2],
            bytes[cursor + 3],
        ]);
        if sig != 0x02014b50 {
            return Err(format!("Invalid CD signature at offset {cursor}: {sig:#x}"));
        }
        let method = u16::from_le_bytes([bytes[cursor + 10], bytes[cursor + 11]]);
        let compressed_size = u32::from_le_bytes([
            bytes[cursor + 20],
            bytes[cursor + 21],
            bytes[cursor + 22],
            bytes[cursor + 23],
        ]);
        let uncompressed_size = u32::from_le_bytes([
            bytes[cursor + 24],
            bytes[cursor + 25],
            bytes[cursor + 26],
            bytes[cursor + 27],
        ]);
        let name_len = u16::from_le_bytes([bytes[cursor + 28], bytes[cursor + 29]]) as usize;
        let extra_len = u16::from_le_bytes([bytes[cursor + 30], bytes[cursor + 31]]) as usize;
        let comment_len = u16::from_le_bytes([bytes[cursor + 32], bytes[cursor + 33]]) as usize;
        let local_header_offset = u32::from_le_bytes([
            bytes[cursor + 42],
            bytes[cursor + 43],
            bytes[cursor + 44],
            bytes[cursor + 45],
        ]);

        let name_start = cursor + 46;
        let name_end = name_start + name_len;
        if name_end > bytes.len() {
            return Err("Truncated entry name".to_string());
        }
        let name = String::from_utf8_lossy(&bytes[name_start..name_end]).to_string();

        entries.insert(
            name.clone(),
            ZipEntry {
                name,
                method,
                compressed_size,
                uncompressed_size,
                local_header_offset,
            },
        );

        cursor = name_end + extra_len + comment_len;
    }

    Ok(entries)
}

/// Read the raw bytes of a single entry, decompressing with INFLATE if needed.
pub fn read_entry(bytes: &[u8], entry: &ZipEntry) -> Result<Vec<u8>, String> {
    let offset = entry.local_header_offset as usize;
    if offset + 30 > bytes.len() {
        return Err("Truncated local file header".to_string());
    }
    // Local file header signature: 0x04034b50
    let sig = u32::from_le_bytes([
        bytes[offset],
        bytes[offset + 1],
        bytes[offset + 2],
        bytes[offset + 3],
    ]);
    if sig != 0x04034b50 {
        return Err(format!("Invalid local header signature: {sig:#x}"));
    }
    let name_len = u16::from_le_bytes([bytes[offset + 26], bytes[offset + 27]]) as usize;
    let extra_len = u16::from_le_bytes([bytes[offset + 28], bytes[offset + 29]]) as usize;
    let data_start = offset + 30 + name_len + extra_len;
    let data_end = data_start + entry.compressed_size as usize;
    if data_end > bytes.len() {
        return Err("Truncated entry data".to_string());
    }
    let raw = &bytes[data_start..data_end];

    match entry.method {
        0 => Ok(raw.to_vec()),
        8 => inflate(raw),
        m => Err(format!("Unsupported compression method: {m}")),
    }
}

/// Read and decode an entry as UTF-8 text.
pub fn read_entry_utf8(bytes: &[u8], entry: &ZipEntry) -> Result<String, String> {
    let raw = read_entry(bytes, entry)?;
    Ok(String::from_utf8_lossy(&raw).to_string())
}

/// Build a minimal "stored" (uncompressed) ZIP archive for testing.
/// Not exposed for production use — intentionally bypasses CRC32 calculation.
#[doc(hidden)]
pub fn build_stored_zip_for_tests(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut archive: Vec<u8> = Vec::new();
    let mut cd: Vec<u8> = Vec::new();
    let mut offsets: Vec<u32> = Vec::new();

    for (name, content) in entries {
        let offset = archive.len() as u32;
        offsets.push(offset);
        // Local file header
        archive.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]);
        archive.extend_from_slice(&[20, 0]);
        archive.extend_from_slice(&[0, 0]);
        archive.extend_from_slice(&[0, 0]); // stored
        archive.extend_from_slice(&[0, 0, 0, 0]);
        archive.extend_from_slice(&[0, 0, 0, 0]); // crc32 placeholder
        archive.extend_from_slice(&(content.len() as u32).to_le_bytes());
        archive.extend_from_slice(&(content.len() as u32).to_le_bytes());
        archive.extend_from_slice(&(name.len() as u16).to_le_bytes());
        archive.extend_from_slice(&[0, 0]);
        archive.extend_from_slice(name.as_bytes());
        archive.extend_from_slice(content);
    }

    let cd_offset = archive.len() as u32;
    for (i, (name, content)) in entries.iter().enumerate() {
        cd.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]);
        cd.extend_from_slice(&[20, 0]);
        cd.extend_from_slice(&[20, 0]);
        cd.extend_from_slice(&[0, 0]);
        cd.extend_from_slice(&[0, 0]);
        cd.extend_from_slice(&[0, 0, 0, 0]);
        cd.extend_from_slice(&[0, 0, 0, 0]);
        cd.extend_from_slice(&(content.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(content.len() as u32).to_le_bytes());
        cd.extend_from_slice(&(name.len() as u16).to_le_bytes());
        cd.extend_from_slice(&[0, 0]);
        cd.extend_from_slice(&[0, 0]);
        cd.extend_from_slice(&[0, 0]);
        cd.extend_from_slice(&[0, 0]);
        cd.extend_from_slice(&[0, 0, 0, 0]);
        cd.extend_from_slice(&offsets[i].to_le_bytes());
        cd.extend_from_slice(name.as_bytes());
    }

    let cd_size = cd.len() as u32;
    archive.extend_from_slice(&cd);

    // EOCD
    archive.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
    archive.extend_from_slice(&[0, 0]);
    archive.extend_from_slice(&[0, 0]);
    archive.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    archive.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    archive.extend_from_slice(&cd_size.to_le_bytes());
    archive.extend_from_slice(&cd_offset.to_le_bytes());
    archive.extend_from_slice(&[0, 0]);

    archive
}

/// Find the last occurrence of a 4-byte signature in the buffer.
fn find_last_signature(bytes: &[u8], sig: &[u8; 4]) -> Option<usize> {
    if bytes.len() < 4 {
        return None;
    }
    (0..=bytes.len() - 4)
        .rev()
        .find(|&i| &bytes[i..i + 4] == sig)
}

// ---------------------------------------------------------------------------
// Minimal INFLATE decoder (RFC 1951)
// ---------------------------------------------------------------------------

/// Decompress RFC 1951 DEFLATE data.
///
/// This is a from-scratch, allocation-friendly implementation sufficient for
/// small OOXML parts (`document.xml`, etc). Not optimized for multi-MB payloads.
pub fn inflate(data: &[u8]) -> Result<Vec<u8>, String> {
    let mut reader = BitReader::new(data);
    let mut out: Vec<u8> = Vec::with_capacity(data.len() * 3);
    loop {
        let bfinal = reader.read_bits(1)?;
        let btype = reader.read_bits(2)?;
        match btype {
            0 => {
                // Stored block
                reader.byte_align();
                let len = reader.read_u16()?;
                let nlen = reader.read_u16()?;
                if len ^ 0xffff != nlen {
                    return Err("Stored block LEN/NLEN mismatch".into());
                }
                for _ in 0..len {
                    out.push(reader.read_byte()?);
                }
            }
            1 => {
                inflate_block(&mut reader, &mut out, fixed_litlen(), fixed_dist())?;
            }
            2 => {
                let (litlen, dist) = read_dynamic_huffman(&mut reader)?;
                inflate_block(&mut reader, &mut out, &litlen, &dist)?;
            }
            _ => return Err("Invalid block type".into()),
        }
        if bfinal == 1 {
            break;
        }
    }
    Ok(out)
}

struct BitReader<'a> {
    data: &'a [u8],
    byte_pos: usize,
    bit_pos: u8,
}

impl<'a> BitReader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self {
            data,
            byte_pos: 0,
            bit_pos: 0,
        }
    }

    fn read_bits(&mut self, n: u8) -> Result<u32, String> {
        let mut value: u32 = 0;
        for i in 0..n {
            if self.byte_pos >= self.data.len() {
                return Err("Unexpected end of stream".into());
            }
            let bit = (self.data[self.byte_pos] >> self.bit_pos) & 1;
            value |= (bit as u32) << i;
            self.bit_pos += 1;
            if self.bit_pos == 8 {
                self.bit_pos = 0;
                self.byte_pos += 1;
            }
        }
        Ok(value)
    }

    fn byte_align(&mut self) {
        if self.bit_pos > 0 {
            self.bit_pos = 0;
            self.byte_pos += 1;
        }
    }

    fn read_u16(&mut self) -> Result<u16, String> {
        let lo = self.read_byte()? as u16;
        let hi = self.read_byte()? as u16;
        Ok((hi << 8) | lo)
    }

    fn read_byte(&mut self) -> Result<u8, String> {
        if self.byte_pos >= self.data.len() {
            return Err("Unexpected end of stream".into());
        }
        let b = self.data[self.byte_pos];
        self.byte_pos += 1;
        Ok(b)
    }
}

#[derive(Clone)]
struct HuffmanTable {
    /// List of `(code, code_length, symbol)` tuples, sorted by length then code.
    entries: Vec<(u32, u8, u16)>,
    /// Max code length in this table.
    max_len: u8,
}

impl HuffmanTable {
    fn from_lengths(lengths: &[u8]) -> Result<Self, String> {
        let max_len = *lengths.iter().max().unwrap_or(&0);
        if max_len == 0 {
            return Ok(Self {
                entries: Vec::new(),
                max_len: 0,
            });
        }

        // Canonical Huffman code generation per RFC 1951 §3.2.2
        let mut bl_count = vec![0u32; (max_len + 1) as usize];
        for &l in lengths {
            if l > 0 {
                bl_count[l as usize] += 1;
            }
        }
        let mut next_code = vec![0u32; (max_len + 1) as usize];
        let mut code: u32 = 0;
        for bits in 1..=max_len as usize {
            code = (code + bl_count[bits - 1]) << 1;
            next_code[bits] = code;
        }

        let mut entries: Vec<(u32, u8, u16)> = Vec::new();
        for (sym, &len) in lengths.iter().enumerate() {
            if len > 0 {
                let c = next_code[len as usize];
                next_code[len as usize] += 1;
                entries.push((c, len, sym as u16));
            }
        }
        // Sort primarily by length (shortest codes checked first)
        entries.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));

        Ok(Self { entries, max_len })
    }

    fn decode(&self, reader: &mut BitReader<'_>) -> Result<u16, String> {
        let mut code: u32 = 0;
        for len in 1..=self.max_len {
            let bit = reader.read_bits(1)?;
            code = (code << 1) | bit;
            // Linear scan of entries with this length
            for &(c, clen, sym) in &self.entries {
                if clen == len && c == code {
                    return Ok(sym);
                }
                if clen > len {
                    break;
                }
            }
        }
        Err("Invalid Huffman code".into())
    }
}

// Length and distance base values per RFC 1951 §3.2.5
const LENGTH_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131,
    163, 195, 227, 258,
];
const LENGTH_EXTRA: [u8; 29] = [
    0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0,
];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537,
    2049, 3073, 4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] = [
    0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13,
    13,
];

fn inflate_block(
    reader: &mut BitReader<'_>,
    out: &mut Vec<u8>,
    litlen: &HuffmanTable,
    dist: &HuffmanTable,
) -> Result<(), String> {
    loop {
        let sym = litlen.decode(reader)?;
        if sym < 256 {
            out.push(sym as u8);
        } else if sym == 256 {
            break;
        } else {
            let idx = (sym - 257) as usize;
            if idx >= LENGTH_BASE.len() {
                return Err("Invalid length symbol".into());
            }
            let extra = reader.read_bits(LENGTH_EXTRA[idx])? as u16;
            let length = LENGTH_BASE[idx] + extra;

            let dsym = dist.decode(reader)?;
            let didx = dsym as usize;
            if didx >= DIST_BASE.len() {
                return Err("Invalid distance symbol".into());
            }
            let dextra = reader.read_bits(DIST_EXTRA[didx])? as u16;
            let distance = DIST_BASE[didx] + dextra;

            let start = out.len().saturating_sub(distance as usize);
            if start == out.len() && distance as usize > out.len() {
                return Err("Invalid back-reference".into());
            }
            for i in 0..length as usize {
                let byte = out[start + i];
                out.push(byte);
            }
        }
    }
    Ok(())
}

fn read_dynamic_huffman(
    reader: &mut BitReader<'_>,
) -> Result<(HuffmanTable, HuffmanTable), String> {
    let hlit = reader.read_bits(5)? as usize + 257;
    let hdist = reader.read_bits(5)? as usize + 1;
    let hclen = reader.read_bits(4)? as usize + 4;

    // Code length code order
    let code_length_order = [
        16u8, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15,
    ];
    let mut code_lengths = vec![0u8; 19];
    for i in 0..hclen {
        code_lengths[code_length_order[i] as usize] = reader.read_bits(3)? as u8;
    }
    let cl_tree = HuffmanTable::from_lengths(&code_lengths)?;

    let total = hlit + hdist;
    let mut lengths = Vec::with_capacity(total);
    while lengths.len() < total {
        let sym = cl_tree.decode(reader)?;
        match sym {
            0..=15 => lengths.push(sym as u8),
            16 => {
                let repeat = reader.read_bits(2)? as usize + 3;
                let last = *lengths.last().ok_or("Repeat with no previous length")?;
                for _ in 0..repeat {
                    lengths.push(last);
                }
            }
            17 => {
                let repeat = reader.read_bits(3)? as usize + 3;
                for _ in 0..repeat {
                    lengths.push(0);
                }
            }
            18 => {
                let repeat = reader.read_bits(7)? as usize + 11;
                for _ in 0..repeat {
                    lengths.push(0);
                }
            }
            _ => return Err(format!("Invalid code length symbol: {sym}")),
        }
    }

    let litlen = HuffmanTable::from_lengths(&lengths[..hlit])?;
    let dist = HuffmanTable::from_lengths(&lengths[hlit..])?;
    Ok((litlen, dist))
}

// Fixed Huffman tables (RFC 1951 §3.2.6)
static FIXED_LITLEN: std::sync::OnceLock<HuffmanTable> = std::sync::OnceLock::new();
static FIXED_DIST: std::sync::OnceLock<HuffmanTable> = std::sync::OnceLock::new();

#[allow(dead_code)]
fn fixed_litlen() -> &'static HuffmanTable {
    FIXED_LITLEN.get_or_init(|| {
        let mut lens = vec![0u8; 288];
        for i in 0..=143 {
            lens[i] = 8;
        }
        for i in 144..=255 {
            lens[i] = 9;
        }
        for i in 256..=279 {
            lens[i] = 7;
        }
        for i in 280..=287 {
            lens[i] = 8;
        }
        HuffmanTable::from_lengths(&lens).unwrap_or(HuffmanTable {
            entries: Vec::new(),
            max_len: 0,
        })
    })
}

#[allow(dead_code)]
fn fixed_dist() -> &'static HuffmanTable {
    FIXED_DIST.get_or_init(|| {
        let lens = vec![5u8; 30];
        HuffmanTable::from_lengths(&lens).unwrap_or(HuffmanTable {
            entries: Vec::new(),
            max_len: 0,
        })
    })
}

// Because HuffmanTable is not Copy, redefine the statics with different init
// strategy: use once_cell-like wrapper via nested closure.
// NOTE: the statics above are used via fixed_litlen()/fixed_dist() but the
// inflate_block signature takes references — switch to functions.

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Test a known stored-method ZIP archive (hand-crafted, minimal).
    #[test]
    fn test_read_stored_zip() {
        // Build a ZIP with one stored entry: name="a.txt", content="hi"
        let mut archive: Vec<u8> = Vec::new();
        let name = b"a.txt";
        let content = b"hi";

        // Local file header
        archive.extend_from_slice(&[0x50, 0x4b, 0x03, 0x04]); // signature
        archive.extend_from_slice(&[20, 0]); // version needed
        archive.extend_from_slice(&[0, 0]); // flags
        archive.extend_from_slice(&[0, 0]); // method: stored
        archive.extend_from_slice(&[0, 0, 0, 0]); // mod time + date
        archive.extend_from_slice(&[0, 0, 0, 0]); // crc32 (ignored here)
        archive.extend_from_slice(&(content.len() as u32).to_le_bytes()); // compressed
        archive.extend_from_slice(&(content.len() as u32).to_le_bytes()); // uncompressed
        archive.extend_from_slice(&(name.len() as u16).to_le_bytes()); // name len
        archive.extend_from_slice(&[0, 0]); // extra len
        archive.extend_from_slice(name);
        archive.extend_from_slice(content);

        let cd_offset = archive.len() as u32;

        // Central directory
        archive.extend_from_slice(&[0x50, 0x4b, 0x01, 0x02]); // signature
        archive.extend_from_slice(&[20, 0]); // version made by
        archive.extend_from_slice(&[20, 0]); // version needed
        archive.extend_from_slice(&[0, 0]); // flags
        archive.extend_from_slice(&[0, 0]); // method
        archive.extend_from_slice(&[0, 0, 0, 0]); // mod time+date
        archive.extend_from_slice(&[0, 0, 0, 0]); // crc32
        archive.extend_from_slice(&(content.len() as u32).to_le_bytes()); // compressed
        archive.extend_from_slice(&(content.len() as u32).to_le_bytes()); // uncompressed
        archive.extend_from_slice(&(name.len() as u16).to_le_bytes()); // name len
        archive.extend_from_slice(&[0, 0]); // extra
        archive.extend_from_slice(&[0, 0]); // comment len
        archive.extend_from_slice(&[0, 0]); // disk number
        archive.extend_from_slice(&[0, 0]); // internal attrs
        archive.extend_from_slice(&[0, 0, 0, 0]); // external attrs
        archive.extend_from_slice(&[0, 0, 0, 0]); // local header offset
        archive.extend_from_slice(name);

        let cd_end = archive.len() as u32;
        let cd_size = cd_end - cd_offset;

        // End of central directory
        archive.extend_from_slice(&[0x50, 0x4b, 0x05, 0x06]);
        archive.extend_from_slice(&[0, 0]); // disk
        archive.extend_from_slice(&[0, 0]); // cd disk
        archive.extend_from_slice(&[1, 0]); // entries this disk
        archive.extend_from_slice(&[1, 0]); // entries total
        archive.extend_from_slice(&cd_size.to_le_bytes());
        archive.extend_from_slice(&cd_offset.to_le_bytes());
        archive.extend_from_slice(&[0, 0]); // comment len

        let entries = read_central_directory(&archive).unwrap();
        assert_eq!(entries.len(), 1);
        let entry = entries.get("a.txt").unwrap();
        assert_eq!(entry.method, 0);
        assert_eq!(entry.uncompressed_size, 2);

        let data = read_entry(&archive, entry).unwrap();
        assert_eq!(&data, b"hi");
    }

    #[test]
    fn test_read_missing_eocd() {
        let bad = b"not a zip archive at all";
        let r = read_central_directory(bad);
        assert!(r.is_err());
    }
}
