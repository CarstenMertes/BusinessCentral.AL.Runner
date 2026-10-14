//! The byte source the readers work on. A `.bak` is read by positional access (the
//! page map points into the file), a `.bacpac` by its zip central directory. Neither
//! needs more than "give me these bytes", so the core takes a trait and the command
//! line supplies a memory map or a file (ADR-006: the core does no OS access).

use crate::error::{Error, Result};
use std::borrow::Cow;

pub trait ByteSource: Send + Sync {
    fn len(&self) -> u64;

    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Fill `buf` with the bytes at `offset`. A short source is an error.
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()>;

    /// The bytes at `offset`. A memory-backed source returns a borrow, so a page read is
    /// a bounds check and nothing else; a file-backed source reads into a fresh buffer.
    fn slice(&self, offset: u64, len: usize) -> Result<Cow<'_, [u8]>> {
        let mut v = vec![0u8; len];
        self.read_at(offset, &mut v)?;
        Ok(Cow::Owned(v))
    }
}

fn eof(offset: u64) -> Error {
    Error::Io(format!("unexpected end of file at offset 0x{offset:x}"))
}

impl ByteSource for [u8] {
    fn len(&self) -> u64 {
        <[u8]>::len(self) as u64
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        let s = self.slice(offset, buf.len())?;
        buf.copy_from_slice(&s);
        Ok(())
    }
    fn slice(&self, offset: u64, len: usize) -> Result<Cow<'_, [u8]>> {
        let start = usize::try_from(offset).map_err(|_| eof(offset))?;
        let end = start.checked_add(len).ok_or_else(|| eof(offset))?;
        match <[u8]>::get(self, start..end) {
            Some(s) => Ok(Cow::Borrowed(s)),
            None => Err(eof(offset + (<[u8]>::len(self).saturating_sub(start)) as u64)),
        }
    }
}

impl ByteSource for Vec<u8> {
    fn len(&self) -> u64 {
        Vec::len(self) as u64
    }
    fn read_at(&self, offset: u64, buf: &mut [u8]) -> Result<()> {
        self.as_slice().read_at(offset, buf)
    }
    fn slice(&self, offset: u64, len: usize) -> Result<Cow<'_, [u8]>> {
        ByteSource::slice(self.as_slice(), offset, len)
    }
}

/// Little-endian readers over a byte slice, used throughout. Out-of-range offsets are a
/// corrupt page, not a panic.
pub(crate) mod le {
    #[inline]
    pub fn u16(b: &[u8], o: usize) -> u16 {
        u16::from_le_bytes([b[o], b[o + 1]])
    }
    #[inline]
    pub fn i16(b: &[u8], o: usize) -> i16 {
        i16::from_le_bytes([b[o], b[o + 1]])
    }
    #[inline]
    pub fn u32(b: &[u8], o: usize) -> u32 {
        u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    }
    #[inline]
    pub fn i32(b: &[u8], o: usize) -> i32 {
        i32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
    }
    #[inline]
    pub fn u64(b: &[u8], o: usize) -> u64 {
        u64::from_le_bytes(b[o..o + 8].try_into().unwrap())
    }
    #[inline]
    pub fn i64(b: &[u8], o: usize) -> i64 {
        i64::from_le_bytes(b[o..o + 8].try_into().unwrap())
    }
}
