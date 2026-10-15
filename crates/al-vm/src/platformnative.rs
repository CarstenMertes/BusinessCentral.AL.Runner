//! `[Native]` procedures of platform codeunits whose AL body is empty (the service tier implements them).
//!
//! Only `Base64Convert` (codeunit 2000000024 of System.app, BC 28 and later: Microsoft's `Base64 Convert Impl.` calls it).
//! Its answers are .NET's `Convert.ToBase64String` and `FromBase64String`. No corpus test calls it yet, so every path
//! through it is an unprobed assumption: Microsoft's tests of the System Application go through it (the Base64 Convert
//! tests, Text Encoding), see docs/probes-needed.md, row "Base64Convert". The probe wins when it lands.

use crate::Session;
use al_builtins::describe::Describe;
use al_builtins::native::Signal;
use al_builtins::stream::{Encoding, write_bytes};
use al_builtins::value::{LibraryValue, Value};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

/// .NET's `Convert.ToBase64String`; with line breaks, a CR LF after every 76 characters (none at the end).
fn encode(bytes: &[u8], line_breaks: bool) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4 + bytes.len() / 57 * 2);
    let mut col = 0;
    for chunk in bytes.chunks(3) {
        let n = (u32::from(chunk[0]) << 16)
            | (u32::from(*chunk.get(1).unwrap_or(&0)) << 8)
            | u32::from(*chunk.get(2).unwrap_or(&0));
        for i in 0..4 {
            if line_breaks && col == 76 {
                out.push_str("\r\n");
                col = 0;
            }
            if i <= chunk.len() {
                out.push(char::from(ALPHABET[((n >> (18 - 6 * i)) & 63) as usize]));
            } else {
                out.push('=');
            }
            col += 1;
        }
    }
    out
}

/// `FromBase64String`: white space is ignored, the length must be a multiple of four, `=` only at the end.
fn decode(text: &str) -> Option<Vec<u8>> {
    let chars: Vec<u8> = text
        .bytes()
        .filter(|b| !matches!(b, b' ' | b'\t' | b'\r' | b'\n'))
        .collect();
    if chars.len() % 4 != 0 {
        return None;
    }
    let mut out = Vec::with_capacity(chars.len() / 4 * 3);
    for (i, quad) in chars.chunks(4).enumerate() {
        let last = (i + 1) * 4 == chars.len();
        let pad = quad.iter().rev().take_while(|b| **b == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (k, b) in quad.iter().enumerate() {
            let v = if k >= 4 - pad {
                0
            } else {
                ALPHABET.iter().position(|a| a == b)? as u32
            };
            n = (n << 6) | v;
        }
        out.push((n >> 16) as u8);
        if pad < 2 {
            out.push((n >> 8) as u8);
        }
        if pad < 1 {
            out.push(n as u8);
        }
    }
    Some(out)
}

fn encoding(enc: Option<&Value>, codepage: Option<&Value>) -> Result<Encoding, Signal> {
    let e = match enc {
        Some(Value::Option(0, _)) => Encoding::MsDos,
        Some(Value::Option(1, _)) => Encoding::Utf8,
        Some(Value::Option(2, _)) => Encoding::Utf16,
        Some(Value::Option(3, _)) => Encoding::Windows,
        other => {
            return unsupported(format!(
                "Base64Convert with the text encoding {}",
                other.map_or("none".into(), |v| v.describe())
            ));
        }
    };
    // Only the code page the streams model (cp1252 for Windows and MSDos) is served.
    match (e, codepage) {
        (Encoding::Windows | Encoding::MsDos, Some(Value::Integer(c))) if !matches!(*c, 0 | 1252) => unsupported(
            format!("Base64Convert with the code page {c} (only 0 and 1252 are modelled)"),
        ),
        _ => Ok(e),
    }
}

fn text_arg(v: Option<&Value>) -> Result<String, Signal> {
    match v {
        Some(Value::Text(t)) | Some(Value::Code(t)) => Ok(t.clone()),
        other => unsupported(format!(
            "Base64Convert with {} as the text",
            other.map_or("nothing".into(), |v| v.describe())
        )),
    }
}

fn flag(v: Option<&Value>) -> Result<bool, Signal> {
    match v {
        Some(Value::Boolean(b)) => Ok(*b),
        _ => unsupported("Base64Convert with a line break flag that is no Boolean"),
    }
}

fn is_in_stream(v: Option<&Value>) -> bool {
    matches!(v, Some(Value::Library(_, h)) if matches!(&*h.borrow(), LibraryValue::InStream(_)))
}

fn drain(v: Option<&Value>) -> Result<Vec<u8>, Signal> {
    match v {
        Some(Value::Library(_, h)) => match &mut *h.borrow_mut() {
            LibraryValue::InStream(s) => Ok(s.drain()),
            _ => unsupported("Base64Convert with a stream that is no InStream"),
        },
        _ => unsupported("Base64Convert with an InStream that is not one"),
    }
}

fn write_out(v: Option<&Value>, bytes: &[u8]) -> Result<(), Signal> {
    match v {
        Some(Value::Library(_, h)) => match &*h.borrow() {
            LibraryValue::OutStream(blob, _) => {
                write_bytes(blob, bytes);
                Ok(())
            }
            _ => unsupported("Base64Convert with a stream that is no OutStream"),
        },
        _ => unsupported("Base64Convert with an OutStream that is not one"),
    }
}

fn decoded(text: &str) -> Result<Vec<u8>, Signal> {
    decode(text).map_or_else(
        || unsupported("FromBase64 of text that is no valid Base64 (the error is not pinned)"),
        Ok,
    )
}

impl Session {
    /// Runs the `[Native]` procedure `name` of `Base64Convert` on its arguments, by their kinds.
    pub(crate) fn base64_native(&mut self, name: &str, a: &[Value]) -> Result<Value, Signal> {
        let _ = self;
        match name.to_ascii_lowercase().as_str() {
            "tobase64" if !is_in_stream(a.first()) => {
                let bytes = encoding(a.get(2), a.get(3))?.encode(&text_arg(a.first())?);
                let out = encode(&bytes, flag(a.get(1))?);
                if a.len() == 5 {
                    write_out(a.get(4), out.as_bytes())?;
                    return Ok(Value::Null);
                }
                Ok(Value::Text(out))
            }
            "tobase64" => {
                let bytes = drain(a.first())?;
                let out = encode(&bytes, flag(a.get(1))?);
                if a.len() == 3 {
                    write_out(a.get(2), out.as_bytes())?;
                    return Ok(Value::Null);
                }
                Ok(Value::Text(out))
            }
            "frombase64" if matches!(a.first(), Some(Value::Text(_) | Value::Code(_))) => {
                let bytes = decoded(&text_arg(a.first())?)?;
                match a.len() {
                    // (Text, TextEncoding, CodePage): Text
                    3 => Ok(Value::Text(encoding(a.get(1), a.get(2))?.decode(&bytes))),
                    // (Text, TextEncoding, CodePage, OutStream)
                    4 => {
                        encoding(a.get(1), a.get(2))?;
                        write_out(a.get(3), &bytes)?;
                        Ok(Value::Null)
                    }
                    // (Text, OutStream)
                    _ => {
                        write_out(a.get(1), &bytes)?;
                        Ok(Value::Null)
                    }
                }
            }
            "frombase64" => {
                let text = String::from_utf8_lossy(&drain(a.first())?).into_owned();
                let bytes = decoded(&text)?;
                if a.len() == 2 {
                    write_out(a.get(1), &bytes)?;
                    return Ok(Value::Null);
                }
                Ok(Value::Text(Encoding::Utf8.decode(&bytes)))
            }
            other => unsupported(format!("Base64Convert.{other}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_matches_the_dotnet_forms() {
        assert_eq!(encode(b"", false), "");
        assert_eq!(encode(b"f", false), "Zg==");
        assert_eq!(encode(b"fo", false), "Zm8=");
        assert_eq!(encode(b"foo", false), "Zm9v");
        assert_eq!(encode(b"foobar", false), "Zm9vYmFy");
        assert_eq!(decode("Zm9vYmFy").unwrap(), b"foobar");
        assert_eq!(decode("Zg==").unwrap(), b"f");
        assert_eq!(decode("Zm8=").unwrap(), b"fo");
        assert!(decode("Zg=").is_none());
        assert!(decode("Z!==").is_none());
        // 57 bytes make one 76 character line; a longer input breaks with CR LF and has none at the end.
        let long = encode(&[b'a'; 58], true);
        assert_eq!(long.matches("\r\n").count(), 1);
        assert!(!long.ends_with("\r\n"));
    }
}
