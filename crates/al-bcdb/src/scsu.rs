//! Decoder for SCSU (Standard Compression Scheme for Unicode), Unicode Technical
//! Standard #6. SQL Server's "Unicode compression" for nvarchar/nchar under row/page
//! compression is SCSU with one convention on top: a value whose SCSU form would have
//! even length gets a trailing 0x10 tag (SC0, which emits nothing) so stored SCSU is
//! always odd-length; even-length stored values are plain UTF-16LE. Both facts were
//! validated against SQL Server SELECT output (bcdb PROVENANCE.md "Unicode compression
//! is SCSU"); this is a port of `Scsu.cs`.

use crate::error::{Error, Result};

/// Static window offsets for SQn quoting (UTS #6 table 2).
const STATIC_OFFSETS: [u32; 8] = [0x0000, 0x0080, 0x0100, 0x0300, 0x2000, 0x2080, 0x2100, 0x3000];
/// Initial dynamic window offsets (UTS #6 table 3).
const INITIAL_DYNAMIC_OFFSETS: [u32; 8] = [0x0080, 0x00C0, 0x0400, 0x0600, 0x0900, 0x3040, 0x30A0, 0xFF00];

/// Window offset for an SDn/UDn index byte (UTS #6 section 3.8.4, table 4).
fn window_offset(x: u8, context: &str) -> Result<u32> {
    Ok(match x {
        0x01..=0x67 => x as u32 * 0x80,
        0x68..=0xA7 => x as u32 * 0x80 + 0xAC00,
        0xF9 => 0x00C0,
        0xFA => 0x0250,
        0xFB => 0x0370,
        0xFC => 0x0530,
        0xFD => 0x3040,
        0xFE => 0x30A0,
        0xFF => 0xFF60,
        _ => return Err(crate::invalid!("SCSU: reserved window index 0x{x:02x} {context}")),
    })
}

fn need(b: &[u8], i: usize, extra: usize) -> Result<()> {
    if i + extra >= b.len() {
        return Err(crate::invalid!("SCSU: truncated tag sequence"));
    }
    Ok(())
}

/// A code point to UTF-16 units appended to `out`. A window offset >= 0x10000 gives a
/// surrogate pair; a lone surrogate (which .NET's `char.ConvertFromUtf32` rejects) is an error.
fn push_cp(out: &mut Vec<u16>, cp: u32) -> Result<()> {
    if cp < 0x10000 {
        if (0xD800..0xE000).contains(&cp) {
            return Err(crate::invalid!("SCSU: code point U+{cp:04X} is a surrogate"));
        }
        out.push(cp as u16);
    } else if cp <= 0x10FFFF {
        let v = cp - 0x10000;
        out.push(0xD800 + (v >> 10) as u16);
        out.push(0xDC00 + (v & 0x3FF) as u16);
    } else {
        return Err(crate::invalid!("SCSU: code point U+{cp:X} is beyond U+10FFFF"));
    }
    Ok(())
}

/// Decode SCSU bytes to a string. Lone surrogates produced by Unicode-mode units are
/// kept as U+FFFD (the lossy form .NET's `StringBuilder` + `ToString` would hold as a
/// lone surrogate and a writer would later replace).
pub fn decode(b: &[u8]) -> Result<String> {
    let mut out: Vec<u16> = Vec::with_capacity(b.len());
    let mut dynamic = INITIAL_DYNAMIC_OFFSETS;
    let mut win = 0usize;
    let mut unicode_mode = false;
    let mut i = 0usize;
    while i < b.len() {
        let c = b[i];
        if !unicode_mode {
            if c >= 0x80 {
                push_cp(&mut out, dynamic[win] + c as u32 - 0x80)?;
                i += 1;
                continue;
            }
            match c {
                0x00 | 0x09 | 0x0A | 0x0D | 0x20..=0x7F => {
                    out.push(c as u16);
                    i += 1;
                }
                0x01..=0x08 => {
                    // SQn: quote one char from static/dynamic window n
                    need(b, i, 1)?;
                    let q = b[i + 1];
                    let n = (c - 1) as usize;
                    let cp = if q < 0x80 {
                        STATIC_OFFSETS[n] + q as u32
                    } else {
                        dynamic[n] + q as u32 - 0x80
                    };
                    push_cp(&mut out, cp)?;
                    i += 2;
                }
                0x0E => {
                    // SQU: quote one UTF-16BE unit
                    need(b, i, 2)?;
                    out.push(((b[i + 1] as u16) << 8) | b[i + 2] as u16);
                    i += 3;
                }
                0x0F => {
                    unicode_mode = true; // SCU
                    i += 1;
                }
                0x10..=0x17 => {
                    win = (c - 0x10) as usize; // SCn
                    i += 1;
                }
                0x18..=0x1F => {
                    // SDn: define dynamic window n
                    need(b, i, 1)?;
                    win = (c - 0x18) as usize;
                    dynamic[win] = window_offset(b[i + 1], "in SDn")?;
                    i += 2;
                }
                _ => return Err(crate::invalid!("SCSU: reserved single-byte-mode tag 0x{c:02x}")),
            }
        } else {
            match c {
                0xE0..=0xE7 => {
                    unicode_mode = false; // UCn
                    win = (c - 0xE0) as usize;
                    i += 1;
                }
                0xE8..=0xEF => {
                    // UDn
                    need(b, i, 1)?;
                    unicode_mode = false;
                    win = (c - 0xE8) as usize;
                    dynamic[win] = window_offset(b[i + 1], "in UDn")?;
                    i += 2;
                }
                0xF0 => {
                    // UQU: quote one unit that would otherwise be a tag
                    need(b, i, 2)?;
                    out.push(((b[i + 1] as u16) << 8) | b[i + 2] as u16);
                    i += 3;
                }
                0xF1 => {
                    // UDX: define extended window
                    need(b, i, 2)?;
                    let v = ((b[i + 1] as u32) << 8) | b[i + 2] as u32;
                    unicode_mode = false;
                    win = (v >> 13) as usize;
                    dynamic[win] = 0x10000 + (v & 0x1FFF) * 0x80;
                    i += 3;
                }
                0xF2 => return Err(Error::InvalidData("SCSU: reserved Unicode-mode tag 0xF2".into())),
                _ => {
                    // plain UTF-16BE unit
                    need(b, i, 1)?;
                    out.push(((c as u16) << 8) | b[i + 1] as u16);
                    i += 2;
                }
            }
        }
    }
    Ok(String::from_utf16_lossy(&out))
}
