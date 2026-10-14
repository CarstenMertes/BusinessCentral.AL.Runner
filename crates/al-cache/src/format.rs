//! The entry file: header, index, dictionary, blocks.
//!
//! ```text
//! header  (140 bytes)  magic, format, key digest, where the index is, its length, raw length and
//!                      hash, file length, hash of the header so far
//! dict    the zstd dictionary the blocks share
//! blocks  zstd(postcard(DepBlock)) per object, with the dictionary and a content checksum
//! index   zstd(postcard(Index)): the facts of every object, where each block sits
//! ```
//!
//! Open reads the header and the index. A damaged header, index or dictionary is refused at open
//! (a hash each). A damaged block is refused when it is read (zstd's content checksum, then the
//! decoder). A file of the wrong length is refused at open. Nothing in a refused file is used.

use crate::Key;
use al_sema::provided::{DepBlock, DepFacts, DepSet, DepSource};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

const MAGIC: &[u8; 8] = b"ALRCDEP\0";
const HEADER: usize = 8 + 4 + 32 + 8 + 8 + 8 + 32 + 8 + 32;
const LEVEL: i32 = 3;
const DICT_SIZE: usize = 512 * 1024;
/// The training samples: about this many bytes of blocks, taken evenly.
const SAMPLE_BYTES: usize = 24 << 20;

/// Why a cache file cannot be used. The caller ignores the file and builds a new one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CacheError {
    /// Not a cache file, or cut short.
    Truncated(String),
    /// Another layout of the file.
    Format { found: u32, expected: u32 },
    /// Made for other inputs (apps, versions, sources, target or compiler).
    KeyMismatch,
    /// A hash or checksum does not match, or a decoder gave up.
    Damaged(String),
}

impl std::fmt::Display for CacheError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CacheError::Truncated(w) => write!(f, "truncated or not a cache file ({w})"),
            CacheError::Format { found, expected } => write!(f, "format {found}, this build reads {expected}"),
            CacheError::KeyMismatch => write!(f, "made for other inputs"),
            CacheError::Damaged(w) => write!(f, "damaged ({w})"),
        }
    }
}

impl std::error::Error for CacheError {}

#[derive(Serialize, Deserialize)]
struct BlockRef {
    offset: u64,
    len: u32,
    raw_len: u32,
}

#[derive(Serialize, Deserialize)]
struct Index {
    facts: Vec<DepFacts>,
    blocks: Vec<BlockRef>,
    dict_offset: u64,
    dict_len: u64,
    dict_sha: [u8; 32],
}

fn sha(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn zstd_err(e: std::io::Error) -> CacheError {
    CacheError::Damaged(format!("zstd: {e}"))
}

/// The bytes of the entry file for `set`. The same set and key give the same bytes.
pub fn build(set: &DepSet, key: &Key) -> Vec<u8> {
    assert_eq!(set.facts.len(), set.blocks.len(), "one block per object");
    let raw: Vec<Vec<u8>> = set
        .blocks
        .par_iter()
        .map(|b| postcard::to_stdvec(b).expect("a block serializes"))
        .collect();
    // Train the dictionary on blocks taken evenly across the set.
    let total: usize = raw.iter().map(Vec::len).sum();
    let stride = (total / SAMPLE_BYTES).max(1);
    let samples: Vec<&[u8]> = raw.iter().step_by(stride).map(Vec::as_slice).collect();
    let dict = if samples.len() >= 8 {
        zstd::dict::from_samples(&samples, DICT_SIZE).unwrap_or_default()
    } else {
        Vec::new()
    };
    let compressed: Vec<Vec<u8>> = raw
        .par_iter()
        .map_init(
            || {
                let mut c = zstd::bulk::Compressor::with_dictionary(LEVEL, &dict).expect("a zstd compressor");
                c.include_checksum(true).expect("checksum flag");
                c
            },
            |c, bytes| c.compress(bytes).expect("a block compresses"),
        )
        .collect();
    // The layout: header, dictionary, blocks, index. The index holds the offsets and comes last,
    // so nothing before it depends on its size.
    let mut at = HEADER as u64;
    let dict_offset = at;
    at += dict.len() as u64;
    let mut block_refs = Vec::with_capacity(compressed.len());
    for (r, c) in raw.iter().zip(&compressed) {
        block_refs.push(BlockRef {
            offset: at,
            len: c.len() as u32,
            raw_len: r.len() as u32,
        });
        at += c.len() as u64;
    }
    let index = Index {
        facts: set.facts.clone(),
        blocks: block_refs,
        dict_offset,
        dict_len: dict.len() as u64,
        dict_sha: sha(&dict),
    };
    let index_raw = postcard::to_stdvec(&index).expect("the index serializes");
    let index_bytes = zstd::bulk::compress(&index_raw, LEVEL).expect("the index compresses");
    let index_offset = at;
    let total_len = index_offset + index_bytes.len() as u64;
    let mut out = Vec::with_capacity(total_len as usize);
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&key.format.to_le_bytes());
    out.extend_from_slice(&key.digest());
    out.extend_from_slice(&index_offset.to_le_bytes());
    out.extend_from_slice(&(index_bytes.len() as u64).to_le_bytes());
    out.extend_from_slice(&(index_raw.len() as u64).to_le_bytes());
    out.extend_from_slice(&sha(&index_bytes));
    out.extend_from_slice(&total_len.to_le_bytes());
    let header_hash = sha(&out);
    out.extend_from_slice(&header_hash);
    debug_assert_eq!(out.len(), HEADER);
    out.extend_from_slice(&dict);
    for c in &compressed {
        out.extend_from_slice(c);
    }
    out.extend_from_slice(&index_bytes);
    assert_eq!(out.len() as u64, total_len, "the layout is as planned");
    out
}

/// An opened entry file. The blocks are read and checked when asked for.
pub struct Entry {
    bytes: Vec<u8>,
    index: Index,
    dict: zstd::dict::DecoderDictionary<'static>,
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("8 bytes"))
}

impl Entry {
    /// Opens `bytes` as the entry for `key`. Reads the header, the index and the dictionary;
    /// nothing else.
    pub fn open(bytes: Vec<u8>, key: &Key) -> Result<Entry, CacheError> {
        if bytes.len() < HEADER {
            return Err(CacheError::Truncated("shorter than the header".into()));
        }
        if &bytes[..8] != MAGIC {
            return Err(CacheError::Truncated("no magic".into()));
        }
        if sha(&bytes[..HEADER - 32]) != bytes[HEADER - 32..HEADER] {
            return Err(CacheError::Damaged("header".into()));
        }
        let format = u32::from_le_bytes(bytes[8..12].try_into().expect("4 bytes"));
        if format != key.format {
            return Err(CacheError::Format {
                found: format,
                expected: key.format,
            });
        }
        if bytes[12..44] != key.digest() {
            return Err(CacheError::KeyMismatch);
        }
        let index_offset = u64_at(&bytes, 44) as usize;
        let index_len = u64_at(&bytes, 52) as usize;
        let index_raw_len = u64_at(&bytes, 60) as usize;
        let total = u64_at(&bytes, 100);
        if total != bytes.len() as u64 {
            return Err(CacheError::Truncated(format!(
                "{} bytes, the header says {total}",
                bytes.len()
            )));
        }
        let index_end = index_offset
            .checked_add(index_len)
            .filter(|&e| e == bytes.len() && index_offset >= HEADER)
            .ok_or_else(|| CacheError::Damaged("index span".into()))?;
        let index_bytes = &bytes[index_offset..index_end];
        if sha(index_bytes) != bytes[68..100] {
            return Err(CacheError::Damaged("index".into()));
        }
        let raw = zstd::bulk::decompress(index_bytes, index_raw_len).map_err(zstd_err)?;
        let index: Index = postcard::from_bytes(&raw).map_err(|e| CacheError::Damaged(format!("index decode: {e}")))?;
        let dict_end = (index.dict_offset as usize)
            .checked_add(index.dict_len as usize)
            .filter(|&e| e <= index_offset && index.dict_offset as usize >= HEADER)
            .ok_or_else(|| CacheError::Damaged("dictionary span".into()))?;
        let dict_bytes = &bytes[index.dict_offset as usize..dict_end];
        if sha(dict_bytes) != index.dict_sha {
            return Err(CacheError::Damaged("dictionary".into()));
        }
        if index.facts.len() != index.blocks.len() {
            return Err(CacheError::Damaged("object count".into()));
        }
        for b in &index.blocks {
            let end = (b.offset as usize).checked_add(b.len as usize);
            if end.is_none_or(|e| e > index_offset) || (b.offset as usize) < dict_end {
                return Err(CacheError::Damaged("block span".into()));
            }
        }
        let dict = zstd::dict::DecoderDictionary::copy(dict_bytes);
        Ok(Entry { bytes, index, dict })
    }

    /// Reads every block. For CI and `cache verify`: a damaged block anywhere is found.
    pub fn verify(&self) -> Result<(), CacheError> {
        (0..self.len())
            .into_par_iter()
            .try_for_each(|i| self.read_block(i).map(|_| ()))
    }

    fn read_block(&self, i: usize) -> Result<DepBlock, CacheError> {
        let r = &self.index.blocks[i];
        let packed = &self.bytes[r.offset as usize..r.offset as usize + r.len as usize];
        let mut d = zstd::bulk::Decompressor::with_prepared_dictionary(&self.dict).map_err(zstd_err)?;
        let raw = d.decompress(packed, r.raw_len as usize).map_err(zstd_err)?;
        postcard::from_bytes(&raw).map_err(|e| CacheError::Damaged(format!("block {i} decode: {e}")))
    }

    /// The size of the file.
    pub fn size(&self) -> usize {
        self.bytes.len()
    }
}

impl DepSource for Entry {
    fn len(&self) -> usize {
        self.index.facts.len()
    }
    fn facts(&self, object: usize) -> &DepFacts {
        &self.index.facts[object]
    }
    fn block(&self, object: usize) -> Result<DepBlock, String> {
        self.read_block(object).map_err(|e| e.to_string())
    }
}
