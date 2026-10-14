//! In-memory streams over Blob fields, plus the array helpers that share their
//! corpus folder (`streams/`).
//!
//! A Blob field holds a shared buffer (`BlobRef`); `CreateOutStream` returns a
//! stream that appends to that same buffer, so a write is visible through the
//! field at once. Whether a table keeps the record's own buffer (temporary
//! tables) or a copy (database tables) is decided by the VM at the store
//! boundary (streams/TestBlobUncommittedWriteIsolation.al). An `InStream`
//! is a live view of that buffer (streams/TestStreamsRuntimeContracts.al:InStream_CreatedBeforeAWrite_SeesTheWrite).
//!
//! `CreateOutStream` does not empty the buffer: a stream writes from the start and overwrites what
//! was there (:BlobField_CreateOutStream_DoesNotEmptyExistingData). Text is written in
//! Windows-1252 unless an encoding says otherwise (:WriteText_DefaultEncodingIsWindows1252_NotUtf8).

use crate::native::{Ctx, NativeFn, Signal};
use crate::signature::Signature;
use crate::value::{LibraryValue, Value};
use std::cell::RefCell;
use std::rc::Rc;

/// What a Blob field holds.
#[derive(Debug)]
pub struct BlobBuf {
    pub bytes: Vec<u8>,
    /// Where the most recent `OutStream` on this buffer writes next. `CreateOutStream` sets it to 0.
    pub write_pos: usize,
    /// False after the record was read from a table: the content then has to be
    /// fetched with `CalcFields` before a stream reads it
    /// (streams/TestBlob.al Blob_Get_WithoutCalcFields_InStreamReadsEmpty).
    pub loaded: bool,
}

pub type BlobRef = Rc<RefCell<BlobBuf>>;

pub fn new_blob() -> BlobRef {
    Rc::new(RefCell::new(BlobBuf {
        bytes: Vec::new(),
        write_pos: 0,
        loaded: true,
    }))
}

/// A copy with its own buffer (what a database table stores and returns).
pub fn copy_blob(b: &BlobRef, loaded: bool) -> BlobRef {
    Rc::new(RefCell::new(BlobBuf {
        bytes: b.borrow().bytes.clone(),
        write_pos: 0,
        loaded,
    }))
}

/// Writes `bytes` at the buffer's write position (over what is there, then past the end) and
/// moves the position on.
pub fn write_bytes(b: &BlobRef, bytes: &[u8]) {
    let mut buf = b.borrow_mut();
    let at = buf.write_pos.min(buf.bytes.len());
    let overlap = bytes.len().min(buf.bytes.len() - at);
    buf.bytes[at..at + overlap].copy_from_slice(&bytes[..overlap]);
    buf.bytes.extend_from_slice(&bytes[overlap..]);
    buf.write_pos = at + bytes.len();
    buf.loaded = true;
}

/// How text becomes bytes in a stream. Windows is cp1252 and the default; MSDos gives the same
/// bytes as Windows; UTF16 is little endian without a byte order mark
/// (streams/TestStreamsRuntimeContracts.al:WriteText_ExplicitEncodings_Bytes).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Encoding {
    Utf8,
    #[default]
    Windows,
    MsDos,
    Utf16,
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
            Encoding::Utf16 => s.encode_utf16().flat_map(u16::to_le_bytes).collect(),
            Encoding::Windows | Encoding::MsDos => s
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
            Encoding::Utf16 => {
                let units: Vec<u16> = b.chunks_exact(2).map(|p| u16::from_le_bytes([p[0], p[1]])).collect();
                String::from_utf16_lossy(&units)
            }
            Encoding::Windows | Encoding::MsDos => b
                .iter()
                .map(|&x| match x {
                    0x80..=0x9F => char::from_u32(u32::from(CP1252_HIGH[usize::from(x - 0x80)])).unwrap_or('?'),
                    x => char::from(x),
                })
                .collect(),
        }
    }

    /// Bytes per code unit of the line terminator search.
    fn unit(self) -> usize {
        if self == Encoding::Utf16 { 2 } else { 1 }
    }

    /// From the `Encoding` argument of `CreateOutStream` / `CreateInStream` (an option value).
    pub(crate) fn from_arg(a: &[Value]) -> Result<Encoding, Signal> {
        match a.get(1) {
            None => Ok(Encoding::default()),
            // Ordinals: MSDos 0, UTF8 1, UTF16 2, Windows 3 (:TextEncoding_Ordinals).
            Some(Value::Option(0, _)) => Ok(Encoding::MsDos),
            Some(Value::Option(1, _)) => Ok(Encoding::Utf8),
            Some(Value::Option(2, _)) => Ok(Encoding::Utf16),
            Some(Value::Option(3, _)) => Ok(Encoding::Windows),
            Some(other) => unsupported(format!("text encoding {other:?}")),
        }
    }
}

/// Where an `InStream` reads from.
#[derive(Debug)]
enum Source {
    /// Bytes taken once (a resource, a download).
    Snapshot(Vec<u8>),
    /// A Blob buffer: later writes show (the content of a buffer that was not loaded is empty).
    Live(BlobRef),
}

/// Read position over a source.
#[derive(Debug)]
pub struct InState {
    source: Source,
    pub pos: usize,
    pub enc: Encoding,
}

impl InState {
    pub fn snapshot(data: Vec<u8>, enc: Encoding) -> InState {
        InState {
            source: Source::Snapshot(data),
            pos: 0,
            enc,
        }
    }

    pub fn live(blob: BlobRef, enc: Encoding) -> InState {
        InState {
            source: Source::Live(blob),
            pos: 0,
            enc,
        }
    }

    /// Runs `f` on the bytes the stream sees now.
    pub fn with_data<T>(&self, f: impl FnOnce(&[u8]) -> T) -> T {
        match &self.source {
            Source::Snapshot(d) => f(d),
            Source::Live(b) => {
                let buf = b.borrow();
                f(if buf.loaded { &buf.bytes } else { &[] })
            }
        }
    }

    pub fn len(&self) -> usize {
        self.with_data(<[u8]>::len)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// The bytes from the read position on.
    pub fn rest(&self) -> Vec<u8> {
        let pos = self.pos;
        self.with_data(|d| d[pos.min(d.len())..].to_vec())
    }

    /// Takes the bytes from the read position on and moves to the end.
    pub fn drain(&mut self) -> Vec<u8> {
        let rest = self.rest();
        self.pos = self.len();
        rest
    }
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
        // The content stays (:BlobField_CreateOutStream_DoesNotEmptyExistingData).
        let mut buf = b.borrow_mut();
        buf.write_pos = 0;
        buf.loaded = true;
    }
    set_stream(&a[0], LibraryValue::OutStream(b, enc))?;
    Ok(Value::Null)
}

fn blob_create_in(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let enc = Encoding::from_arg(a)?;
    let b = blob_of(r)?;
    set_stream(&a[0], LibraryValue::InStream(InState::live(b, enc)))?;
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
        // Before CalcFields the length is the stored size, which is 6 more than the content
        // (:Blob_Length_WithoutCalcFields_ReportsTheStoredSize, one measurement: 5 bytes read as 11).
        // Whether the 6 is constant is docs/probes-needed.md; an empty blob is refused.
        if buf.bytes.is_empty() {
            return unsupported("Blob.Length of an empty field that has not been loaded (not pinned)");
        }
        return Ok(Value::Integer(buf.bytes.len() as i32 + 6));
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
    with_in(r, |s| Ok(Value::Boolean(s.pos >= s.len())))
}

fn in_reset(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    with_in(r, |s| {
        s.pos = 0;
        Ok(Value::Boolean(true))
    })
}

fn in_length(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    with_in(r, |s| Ok(Value::BigInteger(s.len() as i64)))
}

fn in_position(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if !a.is_empty() {
        return unsupported("InStream.Position with a new position (not pinned)");
    }
    with_in(r, |s| Ok(Value::BigInteger(s.pos as i64)))
}

/// Reads up to the next line feed or the end of the stream. The line feed is consumed and every
/// carriage return in the line is dropped, so CR LF is one terminator, a lone CR is not one, and a
/// final CR adds no line (streams/TestStreamsRuntimeContracts.al:InStream_ReadText_SplitsAtLineFeed_AndDropsALoneCarriageReturn).
fn in_read_text(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if a.len() > 1 {
        return unsupported("InStream.ReadText with a length (not pinned)");
    }
    let text = with_in(r, |s| {
        // A byte order mark at the start decides the encoding, whatever the stream was created with,
        // as .NET's StreamReader does: the XmlPort export is UTF-16 with a mark, and the corpus reads
        // it back with the default encoding (xmlport/TestXmlPortObject.al:ReadBlobText).
        if s.pos == 0 {
            let bom = s.with_data(|d| {
                if d.starts_with(&[0xFF, 0xFE]) {
                    Some((Encoding::Utf16, 2))
                } else if d.starts_with(&[0xEF, 0xBB, 0xBF]) {
                    Some((Encoding::Utf8, 3))
                } else {
                    None
                }
            });
            if let Some((e, n)) = bom {
                s.enc = e;
                s.pos = n;
            }
        }
        let (enc, pos) = (s.enc, s.pos);
        let unit = enc.unit();
        let (text, consumed) = s.with_data(|data| {
            let rest = &data[pos.min(data.len())..];
            let mut end = 0;
            while end + unit <= rest.len() && !is_unit(&rest[end..end + unit], b'\n') {
                end += unit;
            }
            let line: Vec<u8> = rest[..end]
                .chunks(unit)
                .filter(|u| !is_unit(u, b'\r'))
                .flatten()
                .copied()
                .collect();
            let consumed = (end + unit).min(rest.len());
            (enc.decode(&line), consumed)
        });
        s.pos = pos + consumed;
        Ok(text)
    })?;
    let n = Value::text_len(&text) as i32;
    a[0] = Value::Text(text);
    Ok(Value::Integer(n))
}

/// Whether a code unit of the stream (one byte, or two for UTF-16) is the ASCII character `c`.
fn is_unit(u: &[u8], c: u8) -> bool {
    match u {
        [b] => *b == c,
        [b, 0] => *b == c,
        _ => false,
    }
}

/// `Read` into an Integer (4 bytes, little endian: streams/TestInOutStream.al
/// InOutStream_Write_Integer_ReadInteger) or a Byte (one byte: the HexOf helper of
/// streams/TestStreamsRuntimeContracts.al reads a stream byte by byte). Other types are not pinned.
fn in_read(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if a.len() > 1 {
        return unsupported("InStream.Read with a length (not pinned)");
    }
    let width = match a[0] {
        Value::Integer(_) => 4,
        Value::Byte(_) => 1,
        // streams/TestStreamsBinaryDateDecimalContracts.al:117-123.
        Value::Decimal(_) => 12,
        _ => {
            return unsupported(format!(
                "InStream.Read into {:?} (only Integer, Byte and Decimal are pinned)",
                a[0]
            ));
        }
    };
    let bytes = with_in(r, |s| {
        let pos = s.pos;
        let got = s
            .with_data(|d| d.get(pos..pos + width).map(<[u8]>::to_vec))
            .ok_or_else(|| Signal::Unsupported("InStream.Read past the end (error text not pinned)".into()))?;
        s.pos += width;
        Ok(got)
    })?;
    a[0] = match a[0] {
        Value::Byte(_) => Value::Byte(bytes[0]),
        Value::Decimal(_) => Value::Decimal(decimal_from_bytes(&bytes)?),
        _ => Value::Integer(i32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])),
    };
    Ok(Value::Integer(width as i32))
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

/// `WriteText(Text)` writes the text in the stream's encoding and no terminator; `WriteText()` writes
/// CR LF (:OutStream_WriteText_WithoutArgument_WritesCarriageReturnLineFeed).
fn out_write_text(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if a.len() > 1 {
        return unsupported("OutStream.WriteText with a length (not pinned)");
    }
    let text = match a.first() {
        None => "\r\n".to_string(),
        Some(v) => match crate::native::text_of(v) {
            Some(t) => t,
            None => return unsupported("OutStream.WriteText of a value that is not text"),
        },
    };
    let (b, enc) = out_buf(r)?;
    let bytes = enc.encode(&text);
    write_bytes(&b, &bytes);
    Ok(Value::Integer(bytes.len() as i32))
}

/// A Date as 4 bytes little endian: 0 for `0D`, else twice the day number (`0001-01-01` is day 1)
/// plus 731, plus 1 for a closing date. Fits every measured value: 1753-01-01, 1900-01-01, 2000-01-01,
/// 2024-03-05, 9999-12-31 and the closing date of 2024-12-31 (streams/TestStreamsBinaryDateDecimalContracts.al:80-90).
fn date_bytes(day: i32, closing: bool) -> Result<Vec<u8>, Signal> {
    let n = match (day, closing) {
        (0, false) => 0,
        (0, true) => return unsupported("OutStream.Write of the closing date of no date (not pinned)"),
        (d, c) => 2 * d + 731 + i32::from(c),
    };
    Ok(n.to_le_bytes().to_vec())
}

/// A Decimal as 12 bytes (streams/TestStreamsBinaryDateDecimalContracts.al:7-12, :94-106): byte 0 holds
/// the sign (0x80) and the decimal exponent of the first digit plus 0x40 (1 is 0x40, 100 is 0x42, 0.01
/// is 0x3E); bytes 1 to 9 hold 18 digits as packed BCD, the first digit in the high nibble of byte 9, the
/// digits after the last significant one are zero; bytes 10 and 11 are zero. Zero is twelve zero bytes.
fn decimal_bytes(d: &rust_decimal::Decimal) -> Result<Vec<u8>, Signal> {
    let mut out = vec![0u8; 12];
    if d.is_zero() {
        return Ok(out);
    }
    let digits = d.mantissa().unsigned_abs().to_string();
    let exponent = digits.len() as i32 - 1 - d.scale() as i32;
    let digits = digits.trim_end_matches('0');
    if digits.len() > 18 {
        return unsupported("OutStream.Write of a Decimal with more than 18 digits (not pinned)");
    }
    out[0] = (0x40 + exponent) as u8 | if d.is_sign_negative() { 0x80 } else { 0 };
    for (i, c) in digits.bytes().enumerate() {
        let v = c - b'0';
        let at = 9 - i / 2;
        out[at] |= if i % 2 == 0 { v << 4 } else { v };
    }
    Ok(out)
}

/// The inverse of [`decimal_bytes`].
fn decimal_from_bytes(b: &[u8]) -> Result<rust_decimal::Decimal, Signal> {
    if b.iter().all(|&x| x == 0) {
        return Ok(rust_decimal::Decimal::ZERO);
    }
    let exponent = i32::from(b[0] & 0x7F) - 0x40;
    let mut mantissa: i128 = 0;
    for i in 0..18 {
        let byte = b[9 - i / 2];
        let v = if i % 2 == 0 { byte >> 4 } else { byte & 0xF };
        mantissa = mantissa * 10 + i128::from(v);
    }
    let scale = 17 - exponent;
    let bad = || Signal::Unsupported("InStream.Read of a Decimal out of range (not pinned)".into());
    let mut d = if scale >= 0 {
        rust_decimal::Decimal::try_from_i128_with_scale(mantissa, scale as u32).map_err(|_| bad())?
    } else {
        let m = mantissa.checked_mul(10i128.pow((-scale) as u32)).ok_or_else(bad)?;
        rust_decimal::Decimal::try_from_i128_with_scale(m, 0).map_err(|_| bad())?
    };
    d.normalize_assign();
    if b[0] & 0x80 != 0 {
        d.set_sign_negative(true);
    }
    Ok(d)
}

/// `Write(Value)`: Integer and Boolean take 4 bytes, BigInteger 8, a Time 4 (its value, which is the
/// millisecond of the day plus one), a Text its characters and a zero byte
/// (:OutStream_Write_BinaryFormsOfEachType, all little endian), a Date 4 and a Decimal 12 (see
/// [`date_bytes`], [`decimal_bytes`]).
fn out_write(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if a.len() != 1 {
        return unsupported("OutStream.Write with a length (not pinned)");
    }
    let (b, enc) = out_buf(r)?;
    let bytes: Vec<u8> = match &a[0] {
        Value::Integer(n) => n.to_le_bytes().to_vec(),
        Value::Boolean(v) => i32::from(*v).to_le_bytes().to_vec(),
        Value::BigInteger(n) => n.to_le_bytes().to_vec(),
        Value::Time(t) => i32::try_from(*t)
            .map_err(|_| Signal::Unsupported("OutStream.Write of a Time outside 32 bits".into()))?
            .to_le_bytes()
            .to_vec(),
        Value::Date(d, closing) => date_bytes(*d, *closing)?,
        Value::Decimal(d) => decimal_bytes(d)?,
        Value::Text(t) | Value::Code(t) if t.is_ascii() => {
            let mut v = enc.encode(t);
            v.extend(std::iter::repeat_n(0, enc.unit()));
            v
        }
        other => return unsupported(format!("OutStream.Write of {other:?} (its binary form is not pinned)")),
    };
    write_bytes(&b, &bytes);
    Ok(Value::Integer(bytes.len() as i32))
}

/// `CopyStream(OutStream, InStream [, BytesToRead])`: copies what remains of the source.
fn copy_stream(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if a.len() > 2 {
        return unsupported("CopyStream with a byte count (not pinned)");
    }
    let (out, _) = out_buf(Some(&mut a[0]))?;
    let bytes = with_in(Some(&mut a[1]), |s| Ok(s.drain()))?;
    write_bytes(&out, &bytes);
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
