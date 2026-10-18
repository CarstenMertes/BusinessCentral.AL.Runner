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

fn text(a: &[Value], i: usize) -> Result<crate::text::Txt, Signal> {
    a.get(i)
        .and_then(text_of)
        .ok_or_else(|| Signal::Unsupported(format!("text argument {i} of type {}", a.get(i).describe())))
}

/// The receiver of a text method: a shared handle, no copy of the text.
fn recv_text(r: &Option<&mut Value>) -> Result<crate::text::Txt, Signal> {
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
        // (corpus record/lt-record-conversion-assumptions/OptCopyStr_risky.al, OptPadStr_risky.al, OptSelectStr_risky.al)
        Some(Value::Option(o, _)) => Ok(i64::from(*o)),
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
    Ok(Value::Boolean(s.contains(sub.as_str())))
}

fn starts_with(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (s, sub) = (recv_text(&r)?, text(a, 0)?);
    // types/TestFormatEvaluateTextContracts.al:Text_EmptyArguments_ContainsStartsEndsAreTrueIndexOfIsOne.
    Ok(Value::Boolean(s.starts_with(sub.as_str())))
}

fn ends_with(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (s, sub) = (recv_text(&r)?, text(a, 0)?);
    // types/TestFormatEvaluateTextContracts.al:Text_EmptyArguments_ContainsStartsEndsAreTrueIndexOfIsOne.
    Ok(Value::Boolean(s.ends_with(sub.as_str())))
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
    let (st, sb) = (recv_text(&r)?, text(a, 0)?);
    // Text of ASCII only is searched where it lies, with no copy of it (the length and the flag are cached).
    if st.is_ascii() && sb.is_ascii() {
        let from = if a.len() > 1 {
            let n = int(a, 1)?;
            if n < 1 || n as usize > st.len() + 1 {
                return Err(out_of_range(
                    "Index was out of range. Must be non-negative and less than or equal to the size of the collection. (Parameter 'startIndex')",
                ));
            }
            n as usize - 1
        } else {
            0
        };
        if sb.is_empty() {
            return Ok(Value::Integer(from as i32 + 1));
        }
        return Ok(Value::Integer(
            st[from..].find(sb.as_str()).map_or(0, |p| (p + from) as i32 + 1),
        ));
    }
    let (s, sub) = (u16s(&st), u16s(&sb));
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
    let (st, sb) = (recv_text(&r)?, text(a, 0)?);
    // A start index is valid from 1 to one past the end (corpus
    // text/lt-text-instance/Search1.Codeunit.al:LastIndexOf_StartIndex: 10 of 9 characters is
    // allowed, 0, -1 and 100 are not; misc/lt-probes-needed-1/PnATextFunctions.al:LastIndexOf_Cases).
    // The empty text is found one past the start index (`'abcabcabc'.LastIndexOf('', 5) = 6`).
    let ascii = st.is_ascii() && sb.is_ascii();
    let len = if ascii { st.len() } else { u16s(&st).len() };
    let start = if a.len() > 1 {
        let n = int(a, 1)?;
        if n < 1 || n as usize > len + 1 {
            return Err(out_of_range(
                "Index was out of range. Must be non-negative and less than the size of the collection. (Parameter 'startIndex')",
            ));
        }
        n as usize
    } else {
        len + 1
    };
    if sb.is_empty() {
        return Ok(Value::Integer(start as i32 + if a.len() > 1 { 1 } else { 0 }));
    }
    let limit = start.min(len);
    // Text of ASCII only is searched where it lies, with no copy of it.
    if ascii {
        return Ok(Value::Integer(
            st[..limit].rfind(sb.as_str()).map_or(0, |p| p as i32 + 1),
        ));
    }
    let (s, sub) = (u16s(&st), u16s(&sb));
    let pos = s[..limit].windows(sub.len()).rposition(|w| w == sub.as_slice());
    Ok(Value::Integer(pos.map_or(0, |p| p as i32 + 1)))
}

fn index_of_any(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let s = u16s(&recv_text(&r)?);
    // The 1-based start index counts in the text: `'Hello, World'.IndexOfAny(['o', ','], 6)` is 6
    // (types/lt-variant-is-text-streams/LTP89TextConst.al:IndexOfAny_Overloads). A start outside the text is not pinned.
    let from = if a.len() > 1 {
        let n = int(a, 1)?;
        if n < 1 || n as usize > s.len() {
            return unsupported("IndexOfAny with a start index outside the text (not pinned)");
        }
        n as usize - 1
    } else {
        0
    };
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
        s[from..]
            .iter()
            .position(|c| set.contains(c))
            .map_or(0, |p| (p + from) as i32 + 1),
    ))
}

/// `Remove(StartIndex [, Count])`: 1-based, `'Hello, World'.Remove(6)` is `Hello` and `Remove(6, 3)` is `Helloorld`
/// (types/lt-variant-is-text-streams/LTP89TextConst.al:Edit_1). Arguments outside the text are not pinned.
fn remove(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let s = u16s(&recv_text(&r)?);
    let start = int(a, 0)?;
    if start < 1 || start as usize > s.len() {
        return unsupported("Text.Remove with a start index outside the text (not pinned)");
    }
    let from = start as usize - 1;
    let to = if a.len() > 1 {
        let n = int(a, 1)?;
        if n < 0 || from + n as usize > s.len() {
            return unsupported("Text.Remove with a count outside the text (not pinned)");
        }
        from + n as usize
    } else {
        s.len()
    };
    let mut out = s[..from].to_vec();
    out.extend_from_slice(&s[to..]);
    Ok(Value::Text(from_u16(&out).into()))
}

// ---------------------------------------------------- transforming

fn to_upper(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    Ok(Value::Text(crate::native::method_upper(&recv_text(&r)?).into()))
}

fn to_lower(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> NResult {
    Ok(Value::Text(crate::native::method_lower(&recv_text(&r)?).into()))
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
    Ok(Value::Text(out.to_string().into()))
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
    Ok(Value::Text(s.replace(old.as_str(), new.as_str()).into()))
}

/// 1-based `StartIndex`; `Count` defaults to the rest. Out-of-range arguments are not pinned.
fn substring(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let st = recv_text(&r)?;
    let ascii = st.is_ascii();
    // Text of ASCII only is sliced where it lies; anything else goes through its UTF-16 units.
    let s: Vec<u16> = if ascii { Vec::new() } else { u16s(&st) };
    let len = if ascii { st.len() } else { s.len() };
    let start = int(a, 0)?;
    let count = if a.len() > 1 { Some(int(a, 1)?) } else { None };
    if start > len as i64 + 1 {
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
        Some(c) => len.min(from + c as usize),
        None => len,
    };
    if ascii {
        return Ok(Value::Text(st[from..to].to_string().into()));
    }
    Ok(Value::Text(from_u16(&s[from..to]).into()))
}

fn pad(r: &Option<&mut Value>, a: &[Value], left: bool) -> NResult {
    let s = recv_text(r)?;
    let width = int(a, 0)?;
    let fill = match a.get(1) {
        Some(v) => text_of(v).unwrap_or_default(),
        None => " ".to_string().into(),
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
    Ok(Value::Text(if left {
        (fill + &s).into()
    } else {
        (s.to_string() + &fill).into()
    }))
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
                parts.push(Value::Text(from_u16(&s[start..i]).into()));
                i += n;
                start = i;
            }
            None => i += 1,
        }
    }
    parts.push(Value::Text(from_u16(&s[start..]).into()));
    Ok(Value::List(Rc::new(RefCell::new(parts.into()))))
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
    Ok(Value::Text(from_u16(&mapped).into()))
}

/// `DelChr(String [, Where [, Which]])`: `Where` is `<` (leading), `>` (trailing), `<>` (both) or
/// `=` (all); `Which` is the set of characters (a space when omitted: TestTextExtended.al
/// DelChr_TrimLeadingAndTrailingSpaces).
fn del_chr(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let s = text(a, 0)?;
    let wh = if a.len() > 1 { text(a, 1)? } else { "=".into() };
    let which: Vec<char> = if a.len() > 2 {
        text(a, 2)?.chars().collect()
    } else {
        vec![' ']
    };
    let strip = |c: char| which.contains(&c);
    // Where is up to three of `=`, `<`, `>` in any order: an `=` deletes everywhere, else `<` and `>` trim that end
    // (errors/lt-ms-refusals-136/P136Text.al:P136_DelChr_ThreeCharWhere and P136_DelChr_DoubledAndOddWhere: `=<>`, `<=>`,
    // `=>`, `<<`, `>>`, `==`, `<>>`, `<<>`, `><`). An empty Where changes nothing; any other character is an error.
    // Longer texts of the three characters are not pinned.
    if wh.chars().any(|c| !matches!(c, '=' | '<' | '>')) {
        return raise("Incorrect use of the where parameter.", "ALSystemString");
    }
    if wh.chars().count() > 3 {
        return unsupported(format!(
            "DelChr with Where `{wh}` (more than three characters is not pinned)"
        ));
    }
    let (all, lead, trail) = (wh.contains('='), wh.contains('<'), wh.contains('>'));
    Ok(Value::Text(match (all, lead, trail) {
        (true, ..) => s.chars().filter(|c| !strip(*c)).collect(),
        (false, true, true) => s.trim_matches(strip).to_string().into(),
        (false, true, false) => s.trim_start_matches(strip).to_string().into(),
        (false, false, true) => s.trim_end_matches(strip).to_string().into(),
        (false, false, false) => s,
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
        return Ok(Value::Text(String::new().into()));
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
    Ok(Value::Text(out.into()))
}

fn pad_str(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let s = text(a, 0)?;
    let len = int(a, 1)?;
    let fill = if a.len() > 2 { text(a, 2)? } else { " ".into() };
    let mut it = fill.chars();
    let (Some(f), None) = (it.next(), it.next()) else {
        return unsupported("PadStr with a fill that is not one character (not pinned)");
    };
    let want = usize::try_from(len).map_err(|_| Signal::Unsupported("PadStr with a negative length".into()))?;
    let u = u16s(&s);
    // Shorter than the text truncates (TestTextProcessingContracts.al PadStr_LengthLessThanStringLen_Truncates).
    if want <= u.len() {
        return Ok(Value::Text(from_u16(&u[..want]).into()));
    }
    let mut out = s.into_string();
    out.extend(std::iter::repeat_n(f, want - u.len()));
    Ok(Value::Text(out.into()))
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
        return Ok(Value::Text(from_u16(&s).into()));
    }
    let from = pos as usize - 1;
    let to = if a.len() > 2 {
        s.len().min(from + int(a, 2)? as usize)
    } else {
        s.len()
    };
    let mut out = s[..from].to_vec();
    out.extend_from_slice(&s[to..]);
    Ok(Value::Text(from_u16(&out).into()))
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
    Ok(Value::Text(from_u16(&out).into()))
}

/// An index outside the list raises with the text and code of
/// types/TestFormatEvaluateTextContracts.al:SelectStr_IndexOutsideTheList_Throws (indexes 4, 0 and -1).
fn select_str(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let n = int(a, 0)?;
    let list = text(a, 1)?;
    let parts: Vec<&str> = list.split(',').collect();
    match usize::try_from(n - 1).ok().and_then(|i| parts.get(i)) {
        Some(p) => Ok(Value::Text((*p).to_string().into())),
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
        Some(t) => Ok(t.to_string()),
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
        Some(v) => Ok(Value::Text(secret_content(v)?.into())),
        None => unsupported("SecretText.Unwrap without a receiver"),
    }
}

fn secret_str_subst_no(c: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let template = text(a, 0)?;
    let args: Vec<Value> = a[1..]
        .iter()
        .map(|v| secret_content(v).map(|s| Value::Text(s.into())))
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
            if matches!(&*h.borrow(), LibraryValue::Text(_) | LibraryValue::TextCap(..)) {
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
        LibraryValue::Text(s) | LibraryValue::TextCap(s, _) => f(s),
        _ => unreachable!("checked by string_of"),
    }
}

/// A TextBuilder whose text outgrew its capacity takes the larger of the length it needs and twice the capacity it had (types/
/// lt-format-codes-textbuilder-pagerun-labels/P144TextBuilderCapacity.al:P144_Growth_ByAppend: 16, 32, 64, 128, then 1000 for an append
/// that needs 1000; P144_Growth_BigAppendAtOnce: 40, 500, then 1000 for the 503 an AppendLine needs).
fn grow_capacity(h: &Rc<RefCell<LibraryValue>>) {
    let (len, cap) = match &*h.borrow() {
        LibraryValue::Text(s) => (Value::text_len(s) as i64, 16i64),
        LibraryValue::TextCap(s, c) => (Value::text_len(s) as i64, i64::from(*c)),
        _ => return,
    };
    if len > cap {
        let new = len.max(cap * 2).min(i64::from(i32::MAX)) as i32;
        let s = with_string(h, std::mem::take);
        *h.borrow_mut() = LibraryValue::TextCap(s, new);
    }
}

fn tb_append(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let (h, t) = (string_of(r)?, text(a, 0)?);
    with_string(&h, |s| s.push_str(&t));
    grow_capacity(&h);
    Ok(Value::Boolean(true))
}

/// `Append` of a Variant (lowering picks this for an argument that is a Variant variable or parameter): a Variant holding an Integer, BigInteger, Decimal, Boolean, Date, Time or DateTime raises the
/// Variant-to-Text conversion error and appends nothing; a Text or Code appends. Pinned for Date (also blank), DateTime (also blank),
/// Time, Decimal, Boolean and Integer, and for the fields of every such type through `RecordRef.FieldIndex(i).Value()`
/// (text/lt-textbuilder-variant/TextBuilderAppendVariant.al:Variant_Dates, Variant_Decimals, Variant_OtherTypes, RecordRef_FieldIndexValue;
/// types/lt-number-to-text/NumberToTextIsFormat.al:193-216, TextBuilderAppend_VariantNumber_Result). Other kinds keep the plain
/// `Append`. A `FieldRef.Value()` written directly as the argument stays on `tb_append`, which refuses a number (only the form through
/// a Variant parameter is pinned).
pub fn tb_append_variant(c: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    if let Some(
        Value::Integer(_)
        | Value::BigInteger(_)
        | Value::Decimal(_)
        | Value::Boolean(_)
        | Value::Date(..)
        | Value::Time(_)
        | Value::DateTime(_),
    ) = a.first()
    {
        // Any value of the Variant-to-Text conversion that raises gives the pinned error text.
        crate::ops::convert(Value::Integer(0), &crate::value::RtType::Text(None))?;
    }
    tb_append(c, r, a)
}

/// The line terminator is a carriage return and a line feed (types/TestFormatEvaluateTextContracts.al:
/// TextBuilder_AppendLine_EndsWithNewLineOnly, text/TestTextBuilder.al:TextBuilder_AppendLine_AddsNewline).
fn tb_append_line(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    let t = if a.is_empty() {
        String::new()
    } else {
        text(a, 0)?.to_string()
    };
    with_string(&h, |s| {
        s.push_str(&t);
        s.push_str("\r\n");
    });
    grow_capacity(&h);
    Ok(Value::Boolean(true))
}

fn tb_to_text(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    let s = with_string(&h, |s| s.clone());
    if a.is_empty() {
        return Ok(Value::Text(s.into()));
    }
    let u = u16s(&s);
    let (start, count) = (int(a, 0)?, int(a, 1)?);
    if start < 1 || count < 0 || (start as usize - 1) + count as usize > u.len() {
        return unsupported("TextBuilder.ToText outside the text (error text not pinned)");
    }
    let from = start as usize - 1;
    Ok(Value::Text(from_u16(&u[from..from + count as usize]).into()))
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
    // Clear keeps the capacity (text/lt-label-textbuilder/TxCTextBuilderBasics.al:Clear_Basics `capKept`).
    let cap = capacity_of(&h);
    *h.borrow_mut() = LibraryValue::TextCap(String::new(), cap);
    Ok(Value::Null)
}

/// The capacity of a TextBuilder: 16 until it is set (TxCTextBuilderBasics.al:Capacity_Defaults, also after `Append('abc')`),
/// then the set value; it grows by doubling when the text outgrows it (only "at least the length" is pinned:
/// Capacity_GrowsWithAppend, Length_Larger_Than_Capacity; docs/probes-needed.md "TextBuilder.Capacity").
fn capacity_of(h: &Rc<RefCell<LibraryValue>>) -> i32 {
    match &*h.borrow() {
        LibraryValue::TextCap(_, c) => *c,
        _ => 16,
    }
}

/// `Capacity()` reads and `Capacity(N)` sets the capacity (TxCTextBuilderBasics.al:Capacity_Setter: `Capacity(100)` reads 100;
/// `Capacity(6)` on six characters reads 6). A capacity below the length is not pinned.
fn tb_capacity(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    if a.is_empty() {
        return Ok(Value::Integer(capacity_of(&h)));
    }
    let n = int(a, 0)?;
    let len = with_string(&h, |s| Value::text_len(s)) as i64;
    if n < len || n > i64::from(i32::MAX) {
        return unsupported("TextBuilder.Capacity set below the length or above the maximum (not pinned)");
    }
    let s = with_string(&h, std::mem::take);
    *h.borrow_mut() = LibraryValue::TextCap(s, n as i32);
    Ok(Value::Integer(n as i32))
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
    grow_capacity(&h);
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

/// `EnsureCapacity(N)` makes the capacity at least N and answers true; N above the maximum is an overflow; zero and a negative N
/// change nothing (TxCTextBuilderBasics.al:EnsureCapacity_Basics).
fn tb_ensure_capacity(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    let n = int(a, 0)?;
    if n > i64::from(i32::MAX) {
        return crate::native::raise("Arithmetic operation resulted in an overflow.", "AL:Overflow");
    }
    // A negative capacity on a builder that holds text is an argument error (types/lt-format-codes-textbuilder-pagerun-labels/
    // P144TextBuilderCapacity.al:P144_EnsureCapacity_Negative: 'abc' appended, capacity 16); on an empty builder with capacity 50 it
    // changes nothing (text/lt-label-textbuilder/TxCTextBuilderBasics.al:EnsureCapacity_Basics). The two files differ in the text
    // held and the capacity; which of them decides is not probed (docs/probes-needed.md).
    if n < 0 && with_string(&h, |s| !s.is_empty()) {
        return crate::native::raise::<Value>(
            "An invalid argument was passed to a 'TextBuilder' data type method.".to_string(),
            "ArgumentOutOfRange",
        );
    }
    if n > i64::from(capacity_of(&h)) {
        let s = with_string(&h, std::mem::take);
        *h.borrow_mut() = LibraryValue::TextCap(s, n as i32);
    }
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
    with_string(&h, |s| *s = s.replace(old.as_str(), new.as_str()));
    grow_capacity(&h);
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

/// `Write(OutStream)`: only ASCII text on a fresh stream is pinned (see `stream::write_bigtext`).
fn bt_write(_: &mut dyn Ctx, r: Option<&mut Value>, a: &mut [Value]) -> NResult {
    let h = string_of(r)?;
    let text = with_string(&h, |s| s.clone());
    crate::stream::write_bigtext(&mut a[0], &text)?;
    Ok(Value::Null)
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
    a[0] = Value::Text(from_u16(&u[from..from + len as usize]).into());
    Ok(Value::Integer(len as i32))
}

// --------------------------------------------------------------- dispatch

pub fn native_for(sig: &Signature) -> Option<NativeFn> {
    let params: Vec<&str> = sig.params.iter().map(|p| p.ty.as_str()).collect();
    let f: NativeFn = match (sig.receiver.as_str(), sig.name.as_str(), params.as_slice()) {
        ("Text" | "Label", "Contains", [_]) => contains,
        ("Text" | "Label", "StartsWith", [_]) => starts_with,
        ("Text" | "Label", "EndsWith", [_]) => ends_with,
        ("Text" | "Label", "IndexOf", _) => index_of,
        ("Text" | "Label", "LastIndexOf", _) => last_index_of,
        ("Text" | "Label", "IndexOfAny", _) => index_of_any,
        ("Text" | "Label", "ToUpper", []) => to_upper,
        ("Text" | "Label", "ToLower", []) => to_lower,
        ("Text" | "Label", "Trim", []) => trim,
        ("Text" | "Label", "TrimStart", _) => trim_start,
        ("Text" | "Label", "TrimEnd", _) => trim_end,
        ("Text" | "Label", "Replace", [_, _]) => replace,
        ("Text" | "Label", "Remove", _) => remove,
        ("Text" | "Label", "Substring", _) => substring,
        ("Text" | "Label", "PadLeft", _) => pad_left,
        ("Text" | "Label", "PadRight", _) => pad_right,
        // A Label has Split like a Text (corpus types/PVLabelSplitForeach.al, staging 8217373).
        ("Text" | "Label", "Split", _) => split,
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
        ("TextBuilder", "Capacity", _) => tb_capacity,
        ("TextBuilder", "Insert", [_, _]) => tb_insert,
        ("TextBuilder", "Remove", [_, _]) => tb_remove,
        ("TextBuilder", "EnsureCapacity", [_]) => tb_ensure_capacity,
        ("TextBuilder", "Replace", [_, _]) => tb_replace,
        ("BigText", "AddText", ["Text", ..]) => bt_add_text,
        ("BigText", "Read", [_]) => bt_read,
        ("BigText", "Write", [_]) => bt_write,
        ("BigText", "Length", []) => bt_length,
        ("BigText", "TextPos", [_]) => bt_text_pos,
        ("BigText", "GetSubText", ["Text", ..]) => bt_get_sub_text,
        _ => return None,
    };
    Some(f)
}
