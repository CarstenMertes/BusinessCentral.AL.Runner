//! Text functions and methods, TextBuilder and BigText.
//!
//! Positions are 1-based and count UTF-16 code units, like AL. Every behaviour
//! cites the corpus test that pins it (text/TestStringMethodContracts.al,
//! TestTextExtended.al, TestTextOperations.al, TestTextProcessingContracts.al,
//! TestTextBuilder.al, TestBigText.al); cases outside the pinned ones, and every
//! error whose text is not pinned, raise `Signal::Unsupported`.

use crate::native::{Ctx, NativeFn, Signal, text_of};
use crate::signature::Signature;
use crate::value::{LibraryValue, Value};
use std::cell::RefCell;
use std::rc::Rc;

type NResult = Result<Value, Signal>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn u16s(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

fn from_u16(u: &[u16]) -> String {
    String::from_utf16_lossy(u)
}

fn text(a: &[Value], i: usize) -> Result<String, Signal> {
    a.get(i)
        .and_then(text_of)
        .ok_or_else(|| Signal::Unsupported(format!("text argument {i} of type {:?}", a.get(i))))
}

fn recv_text(r: &Option<&mut Value>) -> Result<String, Signal> {
    match r.as_deref().and_then(text_of) {
        Some(s) => Ok(s),
        None => unsupported("Text method on something that is not text"),
    }
}

fn int(a: &[Value], i: usize) -> Result<i64, Signal> {
    match a.get(i) {
        Some(Value::Integer(n)) => Ok(i64::from(*n)),
        Some(Value::BigInteger(n)) => Ok(*n),
        Some(Value::Byte(n)) => Ok(i64::from(*n)),
        other => unsupported(format!("integer argument {i} of type {other:?}")),
    }
}

/// Index of `needle` in `hay` at or after `from` (0-based), by UTF-16 code units.
fn find(hay: &[u16], needle: &[u16], from: usize) -> Option<usize> {
    if from > hay.len() || needle.is_empty() {
        return None;
    }
    hay[from..]
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| p + from)
}

// ------------------------------------------------------ searching

fn contains(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (s, sub) = (recv_text(&r)?, text(a, 0)?);
    if sub.is_empty() {
        return unsupported("Contains of an empty text (not pinned)");
    }
    Ok(Value::Boolean(s.contains(&sub)))
}

fn starts_with(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (s, sub) = (recv_text(&r)?, text(a, 0)?);
    if sub.is_empty() {
        return unsupported("StartsWith of an empty text (not pinned)");
    }
    Ok(Value::Boolean(s.starts_with(&sub)))
}

fn ends_with(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (s, sub) = (recv_text(&r)?, text(a, 0)?);
    if sub.is_empty() {
        return unsupported("EndsWith of an empty text (not pinned)");
    }
    Ok(Value::Boolean(s.ends_with(&sub)))
}

/// 1-based, 0 when absent (TestTextExtended.al TextIndexOf_*).
fn index_of(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (s, sub) = (u16s(&recv_text(&r)?), u16s(&text(a, 0)?));
    if sub.is_empty() {
        return unsupported("IndexOf of an empty text (not pinned)");
    }
    let from = if a.len() > 1 {
        let n = int(a, 1)?;
        if n < 1 {
            return unsupported("IndexOf with a start index below 1 (not pinned)");
        }
        n as usize - 1
    } else {
        0
    };
    Ok(Value::Integer(find(&s, &sub, from).map_or(0, |p| p as i32 + 1)))
}

fn last_index_of(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if a.len() > 1 {
        return unsupported("LastIndexOf with a start index (not pinned)");
    }
    let (s, sub) = (u16s(&recv_text(&r)?), u16s(&text(a, 0)?));
    if sub.is_empty() {
        return unsupported("LastIndexOf of an empty text (not pinned)");
    }
    let pos = s.windows(sub.len()).rposition(|w| w == sub.as_slice());
    Ok(Value::Integer(pos.map_or(0, |p| p as i32 + 1)))
}

fn index_of_any(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if a.len() > 1 {
        return unsupported("IndexOfAny with a start index (not pinned)");
    }
    let s = u16s(&recv_text(&r)?);
    let set: Vec<u16> = match &a[0] {
        Value::List(items) => items
            .borrow()
            .iter()
            .map(|v| match v {
                Value::Char(c) => Ok(*c),
                other => unsupported(format!("IndexOfAny list element {other:?}")),
            })
            .collect::<Result<_, _>>()?,
        v => match text_of(v) {
            Some(t) => u16s(&t),
            None => return unsupported("IndexOfAny with something that is not a list of characters or text"),
        },
    };
    Ok(Value::Integer(
        s.iter().position(|c| set.contains(c)).map_or(0, |p| p as i32 + 1),
    ))
}

// ---------------------------------------------------- transforming

fn to_upper(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    Ok(Value::Text(recv_text(&r)?.to_uppercase()))
}

fn to_lower(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    Ok(Value::Text(recv_text(&r)?.to_lowercase()))
}

/// Trim removes white space (the corpus only has spaces); with a character set, those characters.
fn trim_with(r: &Option<&mut Value>, a: &[Value], start: bool, end: bool) -> NResult {
    let s = recv_text(r)?;
    let set: Option<Vec<char>> = match a.first() {
        Some(v) => Some(
            text_of(v)
                .ok_or_else(|| Signal::Unsupported("Trim characters that are not text".into()))?
                .chars()
                .collect(),
        ),
        None => None,
    };
    let strip = |c: char| set.as_ref().map_or(c.is_whitespace(), |cs| cs.contains(&c));
    let mut out: &str = &s;
    if start {
        out = out.trim_start_matches(strip);
    }
    if end {
        out = out.trim_end_matches(strip);
    }
    Ok(Value::Text(out.to_string()))
}

fn trim(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    trim_with(&r, a, true, true)
}
fn trim_start(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    trim_with(&r, a, true, false)
}
fn trim_end(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    trim_with(&r, a, false, true)
}

fn replace(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (s, old, new) = (recv_text(&r)?, text(a, 0)?, text(a, 1)?);
    if old.is_empty() {
        return unsupported("Replace of an empty text (not pinned)");
    }
    Ok(Value::Text(s.replace(&old, &new)))
}

/// 1-based `StartIndex`; `Count` defaults to the rest. Out-of-range arguments are not pinned.
fn substring(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let s = u16s(&recv_text(&r)?);
    let start = int(a, 0)?;
    let count = if a.len() > 1 { Some(int(a, 1)?) } else { None };
    if start < 1 || start as usize > s.len() + 1 {
        return unsupported("Substring with a start index outside the text (error text not pinned)");
    }
    let from = start as usize - 1;
    let to = match count {
        Some(c) if c < 0 || from + c as usize > s.len() => {
            return unsupported("Substring with a count outside the text (error text not pinned)");
        }
        Some(c) => from + c as usize,
        None => s.len(),
    };
    Ok(Value::Text(from_u16(&s[from..to])))
}

fn pad(r: &Option<&mut Value>, a: &[Value], left: bool) -> NResult {
    let s = recv_text(r)?;
    let width = int(a, 0)?;
    let fill = match a.get(1) {
        Some(v) => text_of(v).unwrap_or_default(),
        None => " ".to_string(),
    };
    let mut it = fill.chars();
    let (Some(f), None) = (it.next(), it.next()) else {
        return unsupported("PadLeft/PadRight with a fill that is not one character");
    };
    if width < 0 {
        return unsupported("PadLeft/PadRight with a negative width");
    }
    let have = Value::text_len(&s);
    let missing = (width as usize).saturating_sub(have);
    let fill: String = std::iter::repeat_n(f, missing).collect();
    Ok(Value::Text(if left { fill + &s } else { s + &fill }))
}

fn pad_left(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    pad(&r, a, true)
}
fn pad_right(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    pad(&r, a, false)
}

/// Separators are whole strings, tried in the order given at each position; a list of
/// characters separates at each character; no separator splits on white space. Empty parts
/// are kept (TestTextExtended.al TextSplit_*).
fn split(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let s = u16s(&recv_text(&r)?);
    let seps: Vec<Vec<u16>> = match a {
        [] => Vec::new(),
        [Value::List(items)] => items
            .borrow()
            .iter()
            .map(|v| match v {
                Value::Char(c) => Ok(vec![*c]),
                Value::Text(t) | Value::Code(t) => Ok(u16s(t)),
                other => unsupported(format!("Split separator {other:?}")),
            })
            .collect::<Result<_, _>>()?,
        many => many
            .iter()
            .map(|v| {
                text_of(v)
                    .map(|t| u16s(&t))
                    .ok_or_else(|| Signal::Unsupported("Split separator that is not text".into()))
            })
            .collect::<Result<_, _>>()?,
    };
    if seps.iter().any(Vec::is_empty) {
        return unsupported("Split with an empty separator (not pinned)");
    }
    let is_ws = |c: u16| char::from_u32(u32::from(c)).is_some_and(char::is_whitespace);
    let mut parts: Vec<Value> = Vec::new();
    let (mut start, mut i) = (0usize, 0usize);
    while i < s.len() {
        let hit = if seps.is_empty() {
            is_ws(s[i]).then_some(1)
        } else {
            seps.iter().find(|sep| s[i..].starts_with(sep)).map(|sep| sep.len())
        };
        match hit {
            Some(n) => {
                parts.push(Value::Text(from_u16(&s[start..i])));
                i += n;
                start = i;
            }
            None => i += 1,
        }
    }
    parts.push(Value::Text(from_u16(&s[start..])));
    Ok(Value::List(Rc::new(RefCell::new(parts))))
}

// -------------------------------------------------------- functions

/// Simultaneous mapping: each character of `FromCharacters` becomes the character at the same
/// position of `ToCharacters` (TestTextProcessingContracts.al ConvertStr_Swap_IsSimultaneous).
fn convert_str(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (s, from, to) = (text(a, 0)?, u16s(&text(a, 1)?), u16s(&text(a, 2)?));
    if from.len() != to.len() {
        return unsupported("ConvertStr with different lengths of From and To (not pinned)");
    }
    let mapped: Vec<u16> = u16s(&s)
        .into_iter()
        .map(|c| from.iter().position(|f| *f == c).map_or(c, |i| to[i]))
        .collect();
    Ok(Value::Text(from_u16(&mapped)))
}

/// `DelChr(String [, Where [, Which]])`: `Where` is `<` (leading), `>` (trailing), `<>` (both) or
/// `=` (all); `Which` is the set of characters (a space when omitted: TestTextExtended.al
/// DelChr_TrimLeadingAndTrailingSpaces).
fn del_chr(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let s = text(a, 0)?;
    let wh = if a.len() > 1 { text(a, 1)? } else { "=".to_string() };
    let which: Vec<char> = if a.len() > 2 {
        text(a, 2)?.chars().collect()
    } else {
        vec![' ']
    };
    let strip = |c: char| which.contains(&c);
    Ok(Value::Text(match wh.as_str() {
        "=" => s.chars().filter(|c| !strip(*c)).collect(),
        "<" => s.trim_start_matches(strip).to_string(),
        ">" => s.trim_end_matches(strip).to_string(),
        "<>" | "><" => s.trim_matches(strip).to_string(),
        other => return unsupported(format!("DelChr with Where `{other}` (not pinned)")),
    }))
}

/// Increments the number the text ends with, keeping its width; no trailing number gives an
/// empty text (TestTextExtended.al IncStr_*, TestTextProcessingContracts.al IncStr_*).
fn inc_str(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if a.len() > 1 {
        return unsupported("IncStr with an increment (not pinned)");
    }
    let s = text(a, 0)?;
    let chars: Vec<char> = s.chars().collect();
    let digits = chars.iter().rev().take_while(|c| c.is_ascii_digit()).count();
    if digits == 0 {
        if chars.iter().any(char::is_ascii_digit) {
            return unsupported("IncStr of a text with digits that are not at the end (not pinned)");
        }
        return Ok(Value::Text(String::new()));
    }
    let (head, num) = chars.split_at(chars.len() - digits);
    let mut digs: Vec<u8> = num.iter().map(|c| *c as u8 - b'0').collect();
    let mut carry = true;
    for d in digs.iter_mut().rev() {
        if carry {
            if *d == 9 {
                *d = 0;
            } else {
                *d += 1;
                carry = false;
            }
        }
    }
    let mut out: String = head.iter().collect();
    if carry {
        out.push('1');
    }
    out.extend(digs.iter().map(|d| char::from(b'0' + d)));
    Ok(Value::Text(out))
}

fn pad_str(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let s = text(a, 0)?;
    let len = int(a, 1)?;
    let fill = if a.len() > 2 { text(a, 2)? } else { " ".to_string() };
    let mut it = fill.chars();
    let (Some(f), None) = (it.next(), it.next()) else {
        return unsupported("PadStr with a fill that is not one character (not pinned)");
    };
    let want = usize::try_from(len).map_err(|_| Signal::Unsupported("PadStr with a negative length".into()))?;
    let u = u16s(&s);
    // Shorter than the text truncates (TestTextProcessingContracts.al PadStr_LengthLessThanStringLen_Truncates).
    if want <= u.len() {
        return Ok(Value::Text(from_u16(&u[..want])));
    }
    let mut out = s;
    out.extend(std::iter::repeat_n(f, want - u.len()));
    Ok(Value::Text(out))
}

fn del_str(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let s = u16s(&text(a, 0)?);
    let pos = int(a, 1)?;
    if pos < 1 || pos as usize > s.len() {
        return unsupported("DelStr with a position outside the text (not pinned)");
    }
    let from = pos as usize - 1;
    let to = if a.len() > 2 {
        let n = int(a, 2)?;
        if n < 0 || from + n as usize > s.len() {
            return unsupported("DelStr with a length outside the text (not pinned)");
        }
        from + n as usize
    } else {
        s.len()
    };
    let mut out = s[..from].to_vec();
    out.extend_from_slice(&s[to..]);
    Ok(Value::Text(from_u16(&out)))
}

fn ins_str(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let s = u16s(&text(a, 0)?);
    let sub = u16s(&text(a, 1)?);
    let pos = int(a, 2)?;
    if pos < 1 || pos as usize > s.len() {
        return unsupported("InsStr with a position outside the text (not pinned)");
    }
    let at = pos as usize - 1;
    let mut out = s[..at].to_vec();
    out.extend_from_slice(&sub);
    out.extend_from_slice(&s[at..]);
    Ok(Value::Text(from_u16(&out)))
}

/// Success only: the error for an index outside the list has a text the corpus pins only in part
/// (TestTextProcessingContracts.al `does not contain a value for index`).
fn select_str(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let n = int(a, 0)?;
    let list = text(a, 1)?;
    let parts: Vec<&str> = list.split(',').collect();
    match usize::try_from(n - 1).ok().and_then(|i| parts.get(i)) {
        Some(p) => Ok(Value::Text((*p).to_string())),
        None => unsupported("SelectStr with an index outside the list (error text only partly pinned)"),
    }
}

// ----------------------------------------------- TextBuilder, BigText

fn string_of(r: Option<&mut Value>) -> Result<Rc<RefCell<LibraryValue>>, Signal> {
    match r {
        Some(Value::Library(_, h)) => {
            if matches!(&*h.borrow(), LibraryValue::Empty) {
                *h.borrow_mut() = LibraryValue::Text(String::new());
            }
            if matches!(&*h.borrow(), LibraryValue::Text(_)) {
                Ok(h.clone())
            } else {
                unsupported("TextBuilder or BigText method on something else")
            }
        }
        other => unsupported(format!("TextBuilder or BigText method on {other:?}")),
    }
}

fn with_string<T>(h: &Rc<RefCell<LibraryValue>>, f: impl FnOnce(&mut String) -> T) -> T {
    match &mut *h.borrow_mut() {
        LibraryValue::Text(s) => f(s),
        _ => unreachable!("checked by string_of"),
    }
}

fn tb_append(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (h, t) = (string_of(r)?, text(a, 0)?);
    with_string(&h, |s| s.push_str(&t));
    Ok(Value::Boolean(true))
}

/// The line terminator is not pinned (docs/probes-needed.md): CRLF.
fn tb_append_line(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    let t = if a.is_empty() { String::new() } else { text(a, 0)? };
    with_string(&h, |s| {
        s.push_str(&t);
        s.push_str("\r\n");
    });
    Ok(Value::Boolean(true))
}

fn tb_to_text(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    let s = with_string(&h, |s| s.clone());
    if a.is_empty() {
        return Ok(Value::Text(s));
    }
    let u = u16s(&s);
    let (start, count) = (int(a, 0)?, int(a, 1)?);
    if start < 1 || count < 0 || (start as usize - 1) + count as usize > u.len() {
        return unsupported("TextBuilder.ToText outside the text (error text not pinned)");
    }
    let from = start as usize - 1;
    Ok(Value::Text(from_u16(&u[from..from + count as usize])))
}

fn tb_length(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if !a.is_empty() {
        return unsupported("setting TextBuilder.Length (not pinned)");
    }
    let h = string_of(r)?;
    Ok(Value::Integer(with_string(&h, |s| Value::text_len(s)) as i32))
}

fn tb_clear(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    with_string(&h, String::clear);
    Ok(Value::Null)
}

/// `Insert(Position, Text)` puts the text before 1-based `Position` (TestTextBuilder.al
/// TextBuilder_Insert_InsertsAtPosition, TestTextProcessingContracts.al TextBuilder_Insert_AtOne_Prepends).
fn tb_insert(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    let (pos, t) = (int(a, 0)?, text(a, 1)?);
    let u = with_string(&h, |s| u16s(s));
    if pos < 1 || pos as usize > u.len() + 1 {
        return unsupported("TextBuilder.Insert outside the text (error text not pinned)");
    }
    let at = pos as usize - 1;
    let mut out = u[..at].to_vec();
    out.extend(t.encode_utf16());
    out.extend_from_slice(&u[at..]);
    with_string(&h, |s| *s = from_u16(&out));
    Ok(Value::Boolean(true))
}

/// `Remove(StartIndex, Count)`: 1-based start (TestTextBuilder.al TextBuilder_Remove_RemovesChars).
fn tb_remove(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    let (start, count) = (int(a, 0)?, int(a, 1)?);
    let u = with_string(&h, |s| u16s(s));
    if start < 1 || count < 0 || (start as usize - 1) + count as usize > u.len() {
        return unsupported("TextBuilder.Remove outside the text (error text not pinned)");
    }
    let from = start as usize - 1;
    let mut out = u[..from].to_vec();
    out.extend_from_slice(&u[from + count as usize..]);
    with_string(&h, |s| *s = from_u16(&out));
    Ok(Value::Boolean(true))
}

fn tb_ensure_capacity(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    string_of(r)?;
    Ok(Value::Boolean(true))
}

fn tb_replace(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    let (old, new) = (text(a, 0)?, text(a, 1)?);
    if old.is_empty() {
        return unsupported("TextBuilder.Replace of an empty text (not pinned)");
    }
    with_string(&h, |s| *s = s.replace(&old, &new));
    Ok(Value::Boolean(true))
}

fn bt_add_text(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if a.len() > 1 {
        return unsupported("BigText.AddText at a position (not pinned)");
    }
    let (h, t) = (string_of(r)?, text(a, 0)?);
    with_string(&h, |s| s.push_str(&t));
    Ok(Value::Null)
}

fn bt_length(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    Ok(Value::Integer(with_string(&h, |s| Value::text_len(s)) as i32))
}

/// `GetSubText(var Text, Position [, Length])`; returns the number of characters.
fn bt_get_sub_text(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    let u = with_string(&h, |s| u16s(s));
    let pos = int(a, 1)?;
    let len = if a.len() > 2 {
        int(a, 2)?
    } else {
        u.len() as i64 - pos + 1
    };
    if pos < 1 || len < 0 || (pos as usize - 1) + len as usize > u.len() {
        return unsupported("BigText.GetSubText outside the text (error text not pinned)");
    }
    let from = pos as usize - 1;
    a[0] = Value::Text(from_u16(&u[from..from + len as usize]));
    Ok(Value::Integer(len as i32))
}

// --------------------------------------------------------------- dispatch

pub fn native_for(sig: &Signature) -> Option<NativeFn> {
    let params: Vec<&str> = sig.params.iter().map(|p| p.ty.as_str()).collect();
    let f: NativeFn = match (sig.receiver.as_str(), sig.name.as_str(), params.as_slice()) {
        ("Text", "Contains", [_]) => contains,
        ("Text", "StartsWith", [_]) => starts_with,
        ("Text", "EndsWith", [_]) => ends_with,
        ("Text", "IndexOf", _) => index_of,
        ("Text", "LastIndexOf", _) => last_index_of,
        ("Text", "IndexOfAny", _) => index_of_any,
        ("Text", "ToUpper", []) => to_upper,
        ("Text", "ToLower", []) => to_lower,
        ("Text", "Trim", []) => trim,
        ("Text", "TrimStart", _) => trim_start,
        ("Text", "TrimEnd", _) => trim_end,
        ("Text", "Replace", [_, _]) => replace,
        ("Text", "Substring", _) => substring,
        ("Text", "PadLeft", _) => pad_left,
        ("Text", "PadRight", _) => pad_right,
        ("Text", "Split", _) => split,
        ("Text", "ConvertStr", [_, _, _]) => convert_str,
        ("Text", "DelChr", _) => del_chr,
        ("Text", "IncStr", _) => inc_str,
        ("Text", "PadStr", _) => pad_str,
        ("Text", "DelStr", _) => del_str,
        ("Text", "InsStr", [_, _, _]) => ins_str,
        ("Text", "SelectStr", [_, _]) => select_str,
        ("TextBuilder", "Append", [_]) => tb_append,
        ("TextBuilder", "AppendLine", _) => tb_append_line,
        ("TextBuilder", "ToText", _) => tb_to_text,
        ("TextBuilder", "Length", _) => tb_length,
        ("TextBuilder", "Clear", []) => tb_clear,
        ("TextBuilder", "Insert", [_, _]) => tb_insert,
        ("TextBuilder", "Remove", [_, _]) => tb_remove,
        ("TextBuilder", "EnsureCapacity", [_]) => tb_ensure_capacity,
        ("TextBuilder", "Replace", [_, _]) => tb_replace,
        ("BigText", "AddText", ["Text", ..]) => bt_add_text,
        ("BigText", "Length", []) => bt_length,
        ("BigText", "GetSubText", ["Text", ..]) => bt_get_sub_text,
        _ => return None,
    };
    Some(f)
}
