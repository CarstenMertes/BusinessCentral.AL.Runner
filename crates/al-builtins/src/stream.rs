//! In-memory streams over Blob fields, plus the array helpers that share their
//! corpus folder (`streams/`).
//!
//! A Blob field holds a shared buffer (`BlobRef`); `CreateOutStream` returns a
//! stream that appends to that same buffer, so a write is visible through the
//! field at once. Whether a table keeps the record's own buffer (temporary
//! tables) or a copy (database tables) is decided by the VM at the store
//! boundary (streams/TestBlobUncommittedWriteIsolation.al). An `InStream`
//! reads a snapshot taken when it is created.
//!
//! Text is written as UTF-8 and `Write(Integer)` as 4 little-endian bytes; the
//! corpus only observes round trips and `Length() > 0`.

use crate::native::{Ctx, NativeFn, Signal};
use crate::signature::Signature;
use crate::value::{LibraryValue, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// What a Blob field holds.
#[derive(Debug)]
pub struct BlobBuf {
    pub bytes: Vec<u8>,
    /// False after the record was read from a table: the content then has to be
    /// fetched with `CalcFields` before a stream reads it
    /// (streams/TestBlob.al Blob_Get_WithoutCalcFields_InStreamReadsEmpty).
    pub loaded: bool,
}

pub type BlobRef = Rc<RefCell<BlobBuf>>;

pub fn new_blob() -> BlobRef {
    Rc::new(RefCell::new(BlobBuf {
        bytes: Vec::new(),
        loaded: true,
    }))
}

/// A copy with its own buffer (what a database table stores and returns).
pub fn copy_blob(b: &BlobRef, loaded: bool) -> BlobRef {
    Rc::new(RefCell::new(BlobBuf {
        bytes: b.borrow().bytes.clone(),
        loaded,
    }))
}

/// How text becomes bytes in a stream. The corpus pins UTF-8 and Windows (cp1252: one byte per
/// representable character, text/TestTextEncodingWindows.al); the others are refused.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Encoding {
    #[default]
    Utf8,
    Windows,
}

/// Unicode of cp1252 bytes 0x80..=0x9F (the undefined ones map to themselves, as .NET does).
const CP1252_HIGH: [u16; 32] = [
    0x20AC, 0x0081, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0x008D,
    0x017D, 0x008F, 0x0090, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161, 0x203A,
    0x0153, 0x009D, 0x017E, 0x0178,
];

impl Encoding {
    pub fn encode(self, s: &str) -> Vec<u8> {
        match self {
            Encoding::Utf8 => s.as_bytes().to_vec(),
            Encoding::Windows => s
                .chars()
                .map(|c| {
                    let cp = c as u32;
                    if cp < 0x80 || (0xA0..=0xFF).contains(&cp) {
                        cp as u8
                    } else {
                        CP1252_HIGH
                            .iter()
                            .position(|u| u32::from(*u) == cp)
                            .map_or(b'?', |i| 0x80 + i as u8)
                    }
                })
                .collect(),
        }
    }

    pub fn decode(self, b: &[u8]) -> String {
        match self {
            Encoding::Utf8 => String::from_utf8_lossy(b).into_owned(),
            Encoding::Windows => b
                .iter()
                .map(|&x| match x {
                    0x80..=0x9F => char::from_u32(u32::from(CP1252_HIGH[usize::from(x - 0x80)])).unwrap_or('?'),
                    x => char::from(x),
                })
                .collect(),
        }
    }

    /// From the `Encoding` argument of `CreateOutStream` / `CreateInStream` (an option value).
    fn from_arg(a: &[Value]) -> Result<Encoding, Signal> {
        match a.get(1) {
            None => Ok(Encoding::Utf8),
            // Ordinals as documented: MSDos 0, UTF8 1, UTF16 2, Windows 3.
            Some(Value::Option(1, _)) => Ok(Encoding::Utf8),
            Some(Value::Option(3, _)) => Ok(Encoding::Windows),
            Some(other) => unsupported(format!("text encoding {other:?} (only UTF8 and Windows are pinned)")),
        }
    }
}

/// Read position over a snapshot.
#[derive(Debug, Default)]
pub struct InState {
    pub data: Vec<u8>,
    pub pos: usize,
    pub enc: Encoding,
}

type NResult = Result<Value, Signal>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn blob_of(r: Option<&mut Value>) -> Result<BlobRef, Signal> {
    match r {
        Some(Value::Blob(b)) => Ok(b.clone()),
        other => unsupported(format!("Blob method on {other:?}")),
    }
}

fn set_stream(slot: &Value, content: LibraryValue) -> Result<(), Signal> {
    match slot {
        Value::Library(_, h) => {
            *h.borrow_mut() = content;
            Ok(())
        }
        other => unsupported(format!("a stream variable was expected, got {other:?}")),
    }
}

fn blob_create_out(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let enc = Encoding::from_arg(a)?;
    let b = blob_of(r)?;
    {
        let mut buf = b.borrow_mut();
        buf.bytes.clear();
        buf.loaded = true;
    }
    set_stream(&a[0], LibraryValue::OutStream(b, enc))?;
    Ok(Value::Null)
}

fn blob_create_in(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let enc = Encoding::from_arg(a)?;
    let b = blob_of(r)?;
    let data = {
        let buf = b.borrow();
        if buf.loaded { buf.bytes.clone() } else { Vec::new() }
    };
    set_stream(&a[0], LibraryValue::InStream(InState { data, pos: 0, enc }))?;
    Ok(Value::Null)
}

fn blob_has_value(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    // streams/TestInOutStream.al InOutStream_WriteText_BlobHasValue: true after Get without CalcFields.
    Ok(Value::Boolean(!blob_of(r)?.borrow().bytes.is_empty()))
}

fn blob_length(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let b = blob_of(r)?;
    let buf = b.borrow();
    if !buf.loaded {
        return unsupported("Blob.Length of a field that has not been loaded with CalcFields (not pinned)");
    }
    Ok(Value::Integer(buf.bytes.len() as i32))
}

fn in_state<'a>(r: Option<&'a mut Value>) -> Result<std::cell::RefMut<'a, LibraryValue>, Signal> {
    match r {
        Some(Value::Library(_, h)) => Ok(h.borrow_mut()),
        other => unsupported(format!("InStream method on {other:?}")),
    }
}

fn with_in<T>(r: Option<&mut Value>, f: impl FnOnce(&mut InState) -> Result<T, Signal>) -> Result<T, Signal> {
    let mut g = in_state(r)?;
    match &mut *g {
        LibraryValue::InStream(s) => f(s),
        _ => unsupported("use of an InStream that was never created"),
    }
}

fn in_eos(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    with_in(r, |s| Ok(Value::Boolean(s.pos >= s.data.len())))
}

fn in_reset(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    with_in(r, |s| {
        s.pos = 0;
        Ok(Value::Boolean(true))
    })
}

fn in_length(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    with_in(r, |s| Ok(Value::BigInteger(s.data.len() as i64)))
}

fn in_position(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if !a.is_empty() {
        return unsupported("InStream.Position with a new position (not pinned)");
    }
    with_in(r, |s| Ok(Value::BigInteger(s.pos as i64)))
}

/// Reads up to the next line break (CR, LF or CRLF, which is consumed) or the end of the
/// stream. The corpus only reads text written without a terminator (streams/TestInOutStream.al,
/// TestArrayStreamContracts.al: "ReadText reads until CR/LF or EOS").
fn in_read_text(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if a.len() > 1 {
        return unsupported("InStream.ReadText with a length (not pinned)");
    }
    let text = with_in(r, |s| {
        let rest = &s.data[s.pos..];
        let end = rest
            .iter()
            .position(|&b| b == b'\r' || b == b'\n')
            .unwrap_or(rest.len());
        let text = s.enc.decode(&rest[..end]);
        let mut consumed = end;
        if consumed < rest.len() {
            consumed += if rest[consumed] == b'\r' && rest.get(consumed + 1) == Some(&b'\n') {
                2
            } else {
                1
            };
        }
        s.pos += consumed;
        Ok(text)
    })?;
    let n = Value::text_len(&text) as i32;
    a[0] = Value::Text(text);
    Ok(Value::Integer(n))
}

/// Only `Integer` (4 bytes, little endian) is pinned (streams/TestInOutStream.al
/// InOutStream_Write_Integer_ReadInteger).
fn in_read(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if a.len() > 1 {
        return unsupported("InStream.Read with a length (not pinned)");
    }
    if !matches!(a[0], Value::Integer(_)) {
        return unsupported(format!("InStream.Read into {:?} (only Integer is pinned)", a[0]));
    }
    let n = with_in(r, |s| {
        let bytes: [u8; 4] = s
            .data
            .get(s.pos..s.pos + 4)
            .and_then(|b| b.try_into().ok())
            .ok_or_else(|| Signal::Unsupported("InStream.Read past the end (error text not pinned)".into()))?;
        s.pos += 4;
        Ok(i32::from_le_bytes(bytes))
    })?;
    a[0] = Value::Integer(n);
    Ok(Value::Integer(4))
}

fn out_buf(r: Option<&mut Value>) -> Result<(BlobRef, Encoding), Signal> {
    match r {
        Some(Value::Library(_, h)) => match &*h.borrow() {
            LibraryValue::OutStream(b, e) => Ok((b.clone(), *e)),
            _ => unsupported("use of an OutStream that was never created"),
        },
        other => unsupported(format!("OutStream method on {other:?}")),
    }
}

fn out_write_text(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if a.len() != 1 {
        return unsupported("OutStream.WriteText without a text or with a length (not pinned)");
    }
    let Some(text) = crate::native::text_of(&a[0]) else {
        return unsupported("OutStream.WriteText of a value that is not text");
    };
    let (b, enc) = out_buf(r)?;
    let bytes = enc.encode(&text);
    b.borrow_mut().bytes.extend_from_slice(&bytes);
    Ok(Value::Integer(bytes.len() as i32))
}

fn out_write(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if a.len() != 1 {
        return unsupported("OutStream.Write with a length (not pinned)");
    }
    let Value::Integer(n) = a[0] else {
        return unsupported(format!("OutStream.Write of {:?} (only Integer is pinned)", a[0]));
    };
    out_buf(r)?.0.borrow_mut().bytes.extend_from_slice(&n.to_le_bytes());
    Ok(Value::Integer(4))
}

/// `CopyStream(OutStream, InStream [, BytesToRead])`: copies what remains of the source.
fn copy_stream(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if a.len() > 2 {
        return unsupported("CopyStream with a byte count (not pinned)");
    }
    let (out, _) = out_buf(Some(&mut a[0]))?;
    let bytes = with_in(Some(&mut a[1]), |s| {
        let rest = s.data[s.pos..].to_vec();
        s.pos = s.data.len();
        Ok(rest)
    })?;
    out.borrow_mut().bytes.extend_from_slice(&bytes);
    Ok(Value::Boolean(true))
}

// ------------------------------------------------------- arrays and strings

fn array_of(v: &Value) -> Result<Rc<RefCell<Vec<Value>>>, Signal> {
    match v {
        Value::Array(a) => Ok(a.clone()),
        other => unsupported(format!("{other:?} used as an array")),
    }
}

/// `CompressArray(StringArray)`: empty strings are removed, the rest shifts left, and the tail is
/// blank; returns the number of non-empty strings (streams/TestArrayStreamContracts.al).
fn compress_array(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let arr = array_of(&a[0])?;
    let mut arr = arr.borrow_mut();
    let blank = |v: &Value| matches!(v, Value::Text(s) | Value::Code(s) if s.is_empty());
    let mut kept: Vec<Value> = arr.iter().filter(|v| !blank(v)).cloned().collect();
    let n = kept.len();
    for v in arr.iter().skip(n) {
        if !matches!(v, Value::Text(_) | Value::Code(_)) {
            return unsupported("CompressArray of an array that is not text");
        }
    }
    while kept.len() < arr.len() {
        kept.push(match &arr[kept.len()] {
            Value::Code(_) => Value::Code(String::new()),
            _ => Value::Text(String::new()),
        });
    }
    *arr = kept;
    Ok(Value::Integer(n as i32))
}

/// `CopyArray(NewArray, Array, Position [, Length])` (1-based Position).
fn copy_array(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (dst, src) = (array_of(&a[0])?, array_of(&a[1])?);
    let Value::Integer(pos) = a[2] else {
        return unsupported("CopyArray position that is not an Integer");
    };
    let src = src.borrow();
    let pos = usize::try_from(pos - 1).map_err(|_| Signal::Unsupported("CopyArray position below 1".into()))?;
    let len = match a.get(3) {
        Some(Value::Integer(n)) => {
            usize::try_from(*n).map_err(|_| Signal::Unsupported("CopyArray negative length".into()))?
        }
        None => src.len().saturating_sub(pos),
        Some(other) => return unsupported(format!("CopyArray length {other:?}")),
    };
    let mut dst = dst.borrow_mut();
    if pos + len > src.len() || len > dst.len() {
        return unsupported("CopyArray range outside the arrays (error text not pinned)");
    }
    for i in 0..len {
        dst[i] = src[pos + i].clone();
    }
    Ok(Value::Null)
}

// --------------------------------------------------------------- dispatch

pub fn native_for(sig: &Signature) -> Option<NativeFn> {
    let params: Vec<&str> = sig.params.iter().map(|p| p.ty.as_str()).collect();
    let f: NativeFn = match (sig.receiver.as_str(), sig.name.as_str(), params.as_slice()) {
        ("Blob", "CreateOutStream", ["OutStream", ..]) => blob_create_out,
        ("Blob", "CreateInStream", ["InStream", ..]) => blob_create_in,
        ("Blob", "HasValue", []) => blob_has_value,
        ("Blob", "Length", []) => blob_length,
        ("InStream", "EOS", []) => in_eos,
        ("InStream", "ResetPosition", []) => in_reset,
        ("InStream", "Length", []) => in_length,
        ("InStream", "Position", _) => in_position,
        ("InStream", "ReadText", _) => in_read_text,
        ("InStream", "Read", _) => in_read,
        ("OutStream", "WriteText", _) => out_write_text,
        ("OutStream", "Write", _) => out_write,
        ("System", "CopyStream", _) => copy_stream,
        ("System", "CompressArray", _) => compress_array,
        ("System", "CopyArray", _) => copy_array,
        _ => return None,
    };
    Some(f)
}
