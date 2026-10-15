//! Text functions and methods, TextBuilder and BigText.
//!
//! Positions are 1-based and count UTF-16 code units, like AL. Every behaviour
//! cites the corpus test that pins it (text/TestStringMethodContracts.al,
//! TestTextExtended.al, TestTextOperations.al, TestTextProcessingContracts.al,
//! TestTextBuilder.al, TestBigText.al); cases outside the pinned ones, and every
//! error whose text is not pinned, raise `Signal::Unsupported`.

use crate::describe::Describe;
use crate::native::{Ctx, NativeFn, Signal, raise, text_of};
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
        .ok_or_else(|| Signal::Unsupported(format!("text argument {i} of type {}", a.get(i).describe())))
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
        // An Option converts to its ordinal (`SelectStr(Target, Text)` with an Option parameter in Base Application's
        // Calculate Standard Cost); the same conversion as `I := OptionValue` (ops::convert).
        Some(Value::Option(o, _)) => {
            crate::assumptions::assume(crate::assumptions::Assumption::OptionAsIntegerArgument);
            Ok(i64::from(*o))
        }
        other => unsupported(format!("integer argument {i} of type {}", other.describe())),
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
    // An empty text is found (types/TestFormatEvaluateTextContracts.al:Text_EmptyArguments_ContainsStartsEndsAreTrueIndexOfIsOne).
    Ok(Value::Boolean(s.contains(&sub)))
}

fn starts_with(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (s, sub) = (recv_text(&r)?, text(a, 0)?);
    // types/TestFormatEvaluateTextContracts.al:Text_EmptyArguments_ContainsStartsEndsAreTrueIndexOfIsOne.
    Ok(Value::Boolean(s.starts_with(&sub)))
}

fn ends_with(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (s, sub) = (recv_text(&r)?, text(a, 0)?);
    // types/TestFormatEvaluateTextContracts.al:Text_EmptyArguments_ContainsStartsEndsAreTrueIndexOfIsOne.
    Ok(Value::Boolean(s.ends_with(&sub)))
}

/// The .NET argument errors of the string methods (text/TestProbeTextDateFormatContracts.al:Text_SubstringBounds,
/// Text_IndexOfAndTextBuilderEdges).
fn out_of_range(msg: &str) -> Signal {
    crate::native::raise::<()>(msg.to_string(), "ArgumentOutOfRange").unwrap_err()
}

fn start_error(name: &str, v: i64) -> Signal {
    out_of_range(&format!(
        "{name} ('{v}') must be a non-negative value. (Parameter '{name}')\\Actual value was {v}."
    ))
}

/// An AL range error (`OutsidePermittedRange`), numbers grouped like the platform does.
fn permitted(name: &str, n: usize, cur: i64, lo: &str, hi: &str) -> Signal {
    let g = |v: i64| {
        let d = v.unsigned_abs().to_string();
        let mut o = String::new();
        for (i, c) in d.chars().enumerate() {
            if i > 0 && (d.len() - i) % 3 == 0 {
                o.push(',');
            }
            o.push(c);
        }
        if v < 0 { format!("-{o}") } else { o }
    };
    raise::<()>(
        format!(
            "The value of {name} parameter {n} is outside of the permitted range. The current value is: {}. The permitted range is: from {lo} to {hi}.",
            g(cur)
        ),
        "OutsidePermittedRange",
    )
    .unwrap_err()
}

const INT_MAX: &str = "2,147,483,647";

/// 1-based, 0 when absent (TestTextExtended.al TextIndexOf_*).
fn index_of(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (s, sub) = (u16s(&recv_text(&r)?), u16s(&text(a, 0)?));
    let from = if a.len() > 1 {
        let n = int(a, 1)?;
        // A start outside 1 to the length plus one raises (text/TestProbeTextDateFormatContracts.al:Text_IndexOfAndTextBuilderEdges).
        if n < 1 || n as usize > s.len() + 1 {
            return Err(out_of_range(
                "Index was out of range. Must be non-negative and less than or equal to the size of the collection. (Parameter 'startIndex')",
            ));
        }
        n as usize - 1
    } else {
        0
    };
    // The empty text is found at the start (types/TestFormatEvaluateTextContracts.al:Text_EmptyArguments_ContainsStartsEndsAreTrueIndexOfIsOne: IndexOf('') = 1),
    // at the start index with one (Text_IndexOfAndTextBuilderEdges).
    if sub.is_empty() {
        return Ok(Value::Integer(from as i32 + 1));
    }
    Ok(Value::Integer(find(&s, &sub, from).map_or(0, |p| p as i32 + 1)))
}

/// `LastIndexOf(Value [, StartIndex])`. The empty text is found one past the end
/// (`'hello'.LastIndexOf('') = 6`); with a 1-based start index the match lies within the first
/// `StartIndex` characters (`'hello'.LastIndexOf('l', 3) = 3`), both in
/// types/TestFormatEvaluateTextContracts.al:Text_EmptyArguments_ContainsStartsEndsAreTrueIndexOfIsOne.
fn last_index_of(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (s, sub) = (u16s(&recv_text(&r)?), u16s(&text(a, 0)?));
    let limit = if a.len() > 1 {
        let n = int(a, 1)?;
        if n < 1 || n as usize > s.len() {
            return Err(out_of_range(
                "Index was out of range. Must be non-negative and less than the size of the collection. (Parameter 'startIndex')",
            ));
        }
        n as usize
    } else {
        s.len()
    };
    if sub.is_empty() {
        return Ok(Value::Integer(s.len() as i32 + 1));
    }
    let pos = s[..limit].windows(sub.len()).rposition(|w| w == sub.as_slice());
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
                other => unsupported(format!("IndexOfAny list element {}", other.describe())),
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
    // An empty set trims white space like no set (text/TestProbeTextDateFormatContracts.al:Text_TrimWithCharacterSet).
    let strip = |c: char| {
        set.as_ref()
            .filter(|cs| !cs.is_empty())
            .map_or(c.is_whitespace(), |cs| cs.contains(&c))
    };
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
        // types/TestFormatEvaluateTextContracts.al:Text_EmptyArguments_ContainsStartsEndsAreTrueIndexOfIsOne.
        return raise(
            "The value cannot be an empty string. (Parameter 'oldValue')",
            "Argument",
        );
    }
    Ok(Value::Text(s.replace(&old, &new)))
}

/// 1-based `StartIndex`; `Count` defaults to the rest. Out-of-range arguments are not pinned.
fn substring(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let s = u16s(&recv_text(&r)?);
    let start = int(a, 0)?;
    let count = if a.len() > 1 { Some(int(a, 1)?) } else { None };
    if start > s.len() as i64 + 1 {
        // types/TestFormatEvaluateTextContracts.al:Text_OutOfRangeArguments.
        return raise(
            "startIndex cannot be larger than length of string. (Parameter 'startIndex')",
            "ArgumentOutOfRange",
        );
    }
    if start < 1 {
        return Err(start_error("startIndex", start - 1));
    }
    let from = start as usize - 1;
    let to = match count {
        Some(c) if c < 0 => return Err(start_error("length", c)),
        // A count past the end is clamped (types/TestFormatEvaluateTextContracts.al:Text_OutOfRangeArguments:
        // 'hello'.Substring(2, 10) = 'ello').
        Some(c) => s.len().min(from + c as usize),
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
                other => unsupported(format!("Split separator {}", other.describe())),
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
    // An empty separator never matches; a list of only empty separators does not split at all
    // (types/TestFormatEvaluateTextContracts.al:Text_Split_TriesSeparatorsInOrderAndKeepsEmptyParts:
    // 'hello'.Split('') has one part).
    let whitespace = seps.is_empty();
    let seps: Vec<Vec<u16>> = seps.into_iter().filter(|s| !s.is_empty()).collect();
    let is_ws = |c: u16| char::from_u32(u32::from(c)).is_some_and(char::is_whitespace);
    let mut parts: Vec<Value> = Vec::new();
    let (mut start, mut i) = (0usize, 0usize);
    while i < s.len() {
        let hit = if whitespace {
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
        // types/TestFormatEvaluateTextContracts.al:Text_OutOfRangeArguments.
        return raise(
            format!(
                "ALConvertStr expects 2 strings in the conversion to be of equal length: Converting from {} to {}.",
                from_u16(&from),
                from_u16(&to)
            ),
            "InvalidStringFormat",
        );
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

/// Increments the last block of digits, keeping its width; no digit gives an empty text
/// (TestTextExtended.al IncStr_*, TestTextProcessingContracts.al IncStr_*; digits that are not at
/// the end: types/TestFormatEvaluateTextContracts.al:IncStr_IncrementsTheLastDigitBlock,
/// `IncStr('A1B') = 'A2B'`).
fn inc_str(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let by: i128 = if a.len() > 1 { i128::from(int(a, 1)?) } else { 1 };
    let s = text(a, 0)?;
    let chars: Vec<char> = s.chars().collect();
    let Some(last) = chars.iter().rposition(char::is_ascii_digit) else {
        return Ok(Value::Text(String::new()));
    };
    let first = chars[..=last]
        .iter()
        .rposition(|c| !c.is_ascii_digit())
        .map_or(0, |p| p + 1);
    let (head, rest) = chars.split_at(first);
    let (num, tail) = rest.split_at(last + 1 - first);
    // The digit block plus the increment, keeping the width of the block (`A099` gives `A100`, `A9` gives `A10`);
    // a negative increment subtracts (text/TestProbeTextDateFormatContracts.al:Text_IncStrAndConvertStr).
    let digits: String = num.iter().collect();
    let Ok(value) = digits.parse::<i128>() else {
        return unsupported("IncStr of a digit block beyond 38 digits (not pinned)");
    };
    let sum = value + by;
    if sum < 0 {
        return unsupported("IncStr below zero (not pinned)");
    }
    let mut out: String = head.iter().collect();
    out.push_str(&format!("{sum:0width$}", width = num.len()));
    out.extend(tail.iter());
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
    // text/TestProbeTextDateFormatContracts.al:Text_DelStrInsStr: the position must be 1 or more (the upper bound the
    // message names is 2048 without a length and the largest Integer with one); a length must be 0 or more.
    if pos < 1 {
        return Err(permitted(
            "DELSTR",
            2,
            pos,
            "1",
            if a.len() > 2 { INT_MAX } else { "2048" },
        ));
    }
    if a.len() > 2 {
        let n = int(a, 2)?;
        if n < 0 {
            return Err(permitted("DELSTR", 3, n, "0", INT_MAX));
        }
    }
    // A position past the end deletes nothing (types/TestFormatEvaluateTextContracts.al:Text_OutOfRangeArguments).
    if pos as usize > s.len() {
        return Ok(Value::Text(from_u16(&s)));
    }
    let from = pos as usize - 1;
    let to = if a.len() > 2 {
        s.len().min(from + int(a, 2)? as usize)
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
    if pos < 1 {
        return Err(permitted("INSSTR", 3, pos, "1", INT_MAX));
    }
    // A position past the end appends (types/TestFormatEvaluateTextContracts.al:Text_OutOfRangeArguments).
    let at = (pos as usize - 1).min(s.len());
    let mut out = s[..at].to_vec();
    out.extend_from_slice(&sub);
    out.extend_from_slice(&s[at..]);
    Ok(Value::Text(from_u16(&out)))
}

/// An index outside the list raises with the text and code of
/// types/TestFormatEvaluateTextContracts.al:SelectStr_IndexOutsideTheList_Throws (indexes 4, 0 and -1).
fn select_str(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let n = int(a, 0)?;
    let list = text(a, 1)?;
    let parts: Vec<&str> = list.split(',').collect();
    match usize::try_from(n - 1).ok().and_then(|i| parts.get(i)) {
        Some(p) => Ok(Value::Text((*p).to_string())),
        None => raise(
            format!("The SELECTSTR comma-string {list} does not contain a value for index {n}."),
            "InvalidStringFormat",
        ),
    }
}

// ----------------------------------------------------------- SecretText

fn secret(content: String) -> Value {
    Value::Library("SecretText".into(), Rc::new(RefCell::new(LibraryValue::Text(content))))
}

/// The content of a SecretText value; a never-assigned one is empty.
fn secret_content(v: &Value) -> Result<String, Signal> {
    match v {
        Value::Library(_, h) => match &*h.borrow() {
            LibraryValue::Empty => Ok(String::new()),
            LibraryValue::Text(s) => Ok(s.clone()),
            _ => unsupported("SecretText holding something else"),
        },
        other => unsupported(format!("{} used as a SecretText", other.describe())),
    }
}

/// The text of a Text or SecretText argument (IsolatedStorage takes both).
pub fn secret_or_text(v: &Value) -> Result<String, Signal> {
    match crate::native::text_of(v) {
        Some(t) => Ok(t),
        None => secret_content(v),
    }
}

/// Emptiness is zero length: a single space is content (types/TestSecretText.al).
fn secret_is_empty(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    match r {
        Some(v) => Ok(Value::Boolean(secret_content(v)?.is_empty())),
        None => unsupported("SecretText.IsEmpty without a receiver"),
    }
}

/// `Unwrap()`: the text the secret carries, "the one API that returns the plain text"
/// (types/TestSecretText.al:43); a never-assigned secret is empty.
fn secret_unwrap(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    match r {
        Some(v) => Ok(Value::Text(secret_content(v)?)),
        None => unsupported("SecretText.Unwrap without a receiver"),
    }
}

fn secret_str_subst_no(c: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let template = text(a, 0)?;
    let args: Vec<Value> = a[1..]
        .iter()
        .map(|v| secret_content(v).map(Value::Text))
        .collect::<Result<_, _>>()?;
    Ok(secret(crate::native::substitute(&template, &args, c)?))
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
        other => unsupported(format!("TextBuilder or BigText method on {}", other.describe())),
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

/// The line terminator is a line feed only (types/TestFormatEvaluateTextContracts.al:TextBuilder_AppendLine_EndsWithLineFeedOnly).
fn tb_append_line(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    let t = if a.is_empty() { String::new() } else { text(a, 0)? };
    with_string(&h, |s| {
        s.push_str(&t);
        s.push('\n');
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
        return crate::native::raise::<Value>(
            "An invalid argument was passed to a 'TextBuilder' data type method.".to_string(),
            "ArgumentOutOfRange",
        );
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
        return crate::native::raise::<Value>(
            "An invalid argument was passed to a 'TextBuilder' data type method.".to_string(),
            "ArgumentOutOfRange",
        );
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
        return crate::native::raise::<Value>(
            "An invalid argument was passed to a 'TextBuilder' data type method.".to_string(),
            "Argument",
        );
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

/// `Read(InStream)`: answers true and REPLACES the text the BigText holds
/// (corpus types/PEOptionEnumBigText.al:463-512).
fn bt_read(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    let text = crate::stream::read_all_text(&mut a[0])?;
    with_string(&h, |s| *s = text);
    Ok(Value::Boolean(true))
}

fn bt_length(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    Ok(Value::Integer(with_string(&h, |s| Value::text_len(s)) as i32))
}

/// 1-based position of the first occurrence, 0 when absent (session/TestSystemExtended.al BigTextTextPos_*).
fn bt_text_pos(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    let (hay, needle) = (with_string(&h, |s| u16s(s)), u16s(&text(a, 0)?));
    if needle.is_empty() {
        return unsupported("BigText.TextPos of an empty text (not pinned)");
    }
    Ok(Value::Integer(find(&hay, &needle, 0).map_or(0, |p| p as i32 + 1)))
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
        ("SecretText", "IsEmpty", []) => secret_is_empty,
        ("SecretText", "Unwrap", []) => secret_unwrap,
        ("SecretText", "SecretStrSubstNo", _) => secret_str_subst_no,
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
        ("BigText", "Read", [_]) => bt_read,
        ("BigText", "Length", []) => bt_length,
        ("BigText", "TextPos", [_]) => bt_text_pos,
        ("BigText", "GetSubText", ["Text", ..]) => bt_get_sub_text,
        _ => return None,
    };
    Some(f)
}
