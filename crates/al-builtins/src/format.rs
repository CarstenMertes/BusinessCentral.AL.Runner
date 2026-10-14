//! `Format(Value [, Length] [, FormatNumber])`. Every rule cites the corpus
//! test or the doc table ("Formatting values, dates, and time") that pins it;
//! where the two disagree the corpus wins (it runs on real BC).

use crate::value::Value;

/// What formatting needs from the running program.
pub trait FormatCtx {
    /// Member name of enum `id` at `ordinal`.
    fn enum_name(&self, id: u32, ordinal: i32) -> String;
}

/// Why a format did not produce a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FormatFail {
    /// The form is not pinned yet: the test is not run.
    Unsupported(String),
    /// BC raises this error.
    Error { message: String, code: String },
}

impl From<String> for FormatFail {
    fn from(s: String) -> Self {
        FormatFail::Unsupported(s)
    }
}

/// `Err` names the value whose format is not pinned yet.
pub fn try_format(v: &Value, length: i32, standard: i32, ctx: &dyn FormatCtx) -> Result<String, String> {
    format_value(v, length, standard, ctx).map_err(|f| match f {
        FormatFail::Unsupported(s) => s,
        FormatFail::Error { message, .. } => message,
    })
}

/// `Format(Value, Length, Standard)` with BC's own errors kept apart from refusals.
pub fn format_value(v: &Value, length: i32, standard: i32, ctx: &dyn FormatCtx) -> Result<String, FormatFail> {
    let s = match v {
        Value::Null => String::new(),
        Value::Boolean(b) => match (standard, b) {
            (2, true) => "1".into(),
            (2, false) => "0".into(),
            (9, b) => b.to_string(),
            (_, true) => "Yes".into(),
            (_, false) => "No".into(),
        },
        Value::Integer(i) => i.to_string(),
        Value::BigInteger(i) => i.to_string(),
        Value::Byte(b) => b.to_string(),
        Value::Char(c) => char::from_u32(u32::from(*c)).map(String::from).unwrap_or_default(),
        Value::Decimal(d) => format_decimal(d, standard)?,
        Value::Text(t) | Value::Code(t) => t.clone(),
        Value::Option(o, members) => match standard {
            2 | 9 => o.to_string(),
            _ => members.get(*o as usize).cloned().unwrap_or_else(|| o.to_string()),
        },
        Value::Enum(id, o) => match standard {
            2 | 9 => o.to_string(),
            _ => ctx.enum_name(*id, *o),
        },
        Value::Guid(g) => format_guid(g),
        Value::Date(0, _) | Value::Time(0) | Value::DateTime(0) => String::new(),
        Value::Date(day, closing) => format_date(*day, *closing, standard)?,
        Value::Time(t) => format_time(t - 1, standard)?,
        Value::DateTime(t) => format_datetime(t - 1, standard)?,
        Value::Duration(ms) => format_duration(*ms, standard)?,
        // A formula prints without its angle brackets (types/TestDateRoundFormatStringContracts.al:
        // DateFormula_Evaluate_ThenFormat_PrintsTheNormalizedFormula).
        Value::DateFormula(f) => crate::datefns::formula_text(f),
        // A Version reads `major.minor.build.revision` (session/TestBCPlatformContracts.al:120-136
        // compares the formatted AppVersion with the manifest's `1.0.0.0`).
        Value::Library(name, l) if name == "Version" => match &*l.borrow() {
            crate::value::LibraryValue::Version(v) => format!("{}.{}.{}.{}", v[0], v[1], v[2], v[3]),
            _ => return Err("Format of a Version nobody set".to_string().into()),
        },
        // A SessionSettings renders its contents; the layout is not pinned, only that it shows the
        // time zone and differs when a property differs (session/TestSessionSettings.al:457-493).
        Value::Library(name, l) if name == "SessionSettings" => match &*l.borrow() {
            crate::value::LibraryValue::SessionSettings(s) => format!(
                "Company={};LanguageId={};LocaleId={};TimeZone={};ProfileId={};ProfileAppId={}",
                s.company,
                s.language_id.map_or(String::new(), |v| v.to_string()),
                s.locale_id.map_or(String::new(), |v| v.to_string()),
                s.time_zone.clone().unwrap_or_default(),
                s.profile_id.clone().unwrap_or_default(),
                s.profile_app_id.map_or(String::new(), |g| format_guid(&g)),
            ),
            _ => "Company=;LanguageId=;LocaleId=;TimeZone=;ProfileId=;ProfileAppId=".to_string(),
        },
        // The text form of a FilterPageBuilder is only known to be non-empty
        // (filterpage/TestFilterPageBuilder.al:FilterPageBuilder_Format_ReturnsANonEmptyRendering).
        Value::Library(name, _) if name == "FilterPageBuilder" => "FilterPageBuilder".to_string(),
        // A WebServiceActionContext has no text form (session/TestWebServiceActionContext.al:615-650).
        Value::Library(name, _) if name == "WebServiceActionContext" => String::new(),
        // A RecordId reads `Table: key1,key2` (corpus record/TestRecordRecordId.al:42-56 only needs
        // the key values in it; the layout is docs/probes-needed.md).
        Value::Library(name, l) if name == "RecordId" => match &*l.borrow() {
            crate::value::LibraryValue::RecordId { name, key, .. } => {
                let key = key
                    .iter()
                    .map(|k| format_value(k, 0, 0, ctx))
                    .collect::<Result<Vec<_>, _>>()?;
                format!("{name}: {}", key.join(","))
            }
            _ => String::new(),
        },
        // Duration and the other reference types get their formats with
        // the corpus tests that pin them.
        other => {
            return Err(format!("Format of {other:?} (standard format {standard}) is not pinned yet").into());
        }
    };
    Ok(pad(s, length))
}

/// A positive length right-aligns in that many characters (text/TestFormat.al:
/// `Format(42, 5) = '   42'`).
pub(crate) fn pad(s: String, length: i32) -> String {
    let n = crate::value::Value::text_len(&s);
    if length > 0 && (length as usize) > n {
        format!("{}{s}", " ".repeat(length as usize - n))
    } else {
        s
    }
}

/// Format 0 groups thousands (`1,234,567.891`); formats 1, 2 and 9 do not
/// (types/TestFormatEvaluateTextContracts.al:Format_Decimal_StandardFormats). Trailing zeros of the
/// decimals are not printed. The other numbers are not pinned.
fn format_decimal(d: &rust_decimal::Decimal, standard: i32) -> Result<String, FormatFail> {
    let n = d.normalize();
    let plain = n.to_string();
    if matches!(standard, 1 | 2 | 9) {
        return Ok(plain);
    }
    if standard != 0 {
        return Err(format!("Format of a Decimal with standard format {standard} (not pinned)").into());
    }
    let (sign, digits) = plain.strip_prefix('-').map_or(("", plain.as_str()), |r| ("-", r));
    let (int, frac) = digits.split_once('.').map_or((digits, None), |(i, f)| (i, Some(f)));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    Ok(match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    })
}

fn format_guid(g: &[u8; 16]) -> String {
    let h: String = g.iter().map(|b| format!("{b:02X}")).collect();
    format!(
        "{{{}-{}-{}-{}-{}}}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// Day number 1 = 0001-01-01. Returns (year, month, day).
pub fn civil_from_day(day: i32) -> (i32, u32, u32) {
    // Days since 0000-03-01 style algorithm (Howard Hinnant), shifted so day 1 = 0001-01-01.
    let z = i64::from(day) - 1 + 306; // 0001-01-01 is 306 days after 0000-03-01
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = (yoe + era * 400) as i32 + i32::from(m <= 2);
    (y, m, d)
}

/// Inverse of [`civil_from_day`].
pub fn day_from_civil(y: i32, m: u32, d: u32) -> i32 {
    let y = i64::from(y) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let mp = (i64::from(m) + 9) % 12;
    let doy = (153 * mp + 2) / 5 + i64::from(d) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    (era * 146_097 + doe - 306 + 1) as i32
}

fn hour12(h: i64) -> (i64, &'static str) {
    (if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" })
}

/// `hh:mm AM/PM` for a time of day in milliseconds, the hour padded with a zero
/// (types/TestFormatEvaluateTextContracts.al:Format_DateTime_StandardFormats).
fn format_time_12h(ms_of_day: i64) -> String {
    let (h, m) = (ms_of_day / 3_600_000, ms_of_day / 60_000 % 60);
    let (h12, ap) = hour12(h);
    format!("{h12:02}:{m:02} {ap}")
}

/// The milliseconds of a time of day as `.fff`, nothing when there are none. A count that ends in a
/// zero (`.120` or `.12`) is not measured (docs/probes-needed.md), but prints padded like `.007`.
fn millis(ms: i64) -> String {
    if ms == 0 { String::new() } else { format!(".{ms:03}") }
}

/// A Time, from its milliseconds since midnight (types/TestFormatEvaluateTextContracts.al:
/// Format_Time_StandardFormats). Format 0 and 1 are a 12-hour clock whose hour is padded with a
/// space; format 1 adds the milliseconds (a time without any is not measured: refused); format 2 is
/// the AL literal; format 9 the round-trip form.
fn format_time(ms_of_day: i64, standard: i32) -> Result<String, FormatFail> {
    let (h, m, s, f) = (
        ms_of_day / 3_600_000,
        ms_of_day / 60_000 % 60,
        ms_of_day / 1000 % 60,
        ms_of_day % 1000,
    );
    let (h12, ap) = hour12(h);
    Ok(match standard {
        0 => format!("{h12:>2}:{m:02}:{s:02} {ap}"),
        1 if f != 0 => format!("{h12:>2}:{m:02}:{s:02}{} {ap}", millis(f)),
        2 => format!("{h:02}{m:02}{s:02}{}T", millis(f)),
        9 => format!("{h:02}:{m:02}:{s:02}{}", millis(f)),
        n => return Err(format!("Format of a Time with standard format {n} (not pinned)").into()),
    })
}

/// A DateTime, from its milliseconds since 0001-01-01. Formats 0 and 2 render to the minute
/// (`09/20/26 01:10 PM`, pagesave/TestPageSaveDataLayer.al:199-206); format 1 adds seconds and
/// milliseconds (refused without milliseconds); format 9 is the round-trip form
/// (`2026-09-20T11:10:12.851Z`, no fraction when there are no milliseconds:
/// types/TestFormatEvaluateTextContracts.al:Format_DateTime_StandardFormats). The session time zone
/// is taken to be UTC.
fn format_datetime(ms: i64, standard: i32) -> Result<String, FormatFail> {
    let day = ms.div_euclid(86_400_000);
    let in_day = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_day(day as i32 + 1);
    let (h, mi, s, f) = (
        in_day / 3_600_000,
        in_day / 60_000 % 60,
        in_day / 1000 % 60,
        in_day % 1000,
    );
    let date = format!("{m:02}/{d:02}/{:02}", y.rem_euclid(100));
    Ok(match standard {
        0 | 2 => format!("{date} {}", format_time_12h(in_day)),
        1 if f != 0 => {
            let (h12, ap) = hour12(h);
            format!("{date} {h12:02}:{mi:02}:{s:02}{} {ap}", millis(f))
        }
        9 => format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}{}Z", millis(f)),
        n => return Err(format!("Format of a DateTime with standard format {n} (not pinned)").into()),
    })
}

/// A Duration. Formats 0 and 1 spell the units (`1 day 1 hour 1 minute 1 second 1 millisecond`, a
/// zero unit left out, a zero duration blank, every unit negative for a negative one:
/// handlers/TestPageFieldFormats_Tests.al:599-602, handlers/TestPageRuntimeContracts.al:162);
/// format 9 is `P1DT1H1M1.001S` (types/TestFormatEvaluateTextContracts.al:Format_Duration_StandardFormats)
/// with `.0` for whole seconds (json/TestJsonRuntimeContracts.al:214, `P0DT0H0M5.0S`).
fn format_duration(ms: i64, standard: i32) -> Result<String, FormatFail> {
    match standard {
        0 | 1 => {
            if ms == 0 {
                return Ok(String::new());
            }
            let sign = if ms < 0 { "-" } else { "" };
            let mut left = ms.abs();
            let mut parts = Vec::new();
            for (unit, size) in [
                ("day", 86_400_000),
                ("hour", 3_600_000),
                ("minute", 60_000),
                ("second", 1000),
                ("millisecond", 1),
            ] {
                let n = left / size;
                left %= size;
                if n > 0 {
                    parts.push(format!("{sign}{n} {unit}{}", if n == 1 { "" } else { "s" }));
                }
            }
            Ok(parts.join(" "))
        }
        9 => {
            if ms < 0 {
                return Err("Format 9 of a negative Duration (not pinned)".to_string().into());
            }
            let (d, h, m, s, f) = (
                ms / 86_400_000,
                ms / 3_600_000 % 24,
                ms / 60_000 % 60,
                ms / 1000 % 60,
                ms % 1000,
            );
            let frac = if f == 0 { ".0".to_string() } else { format!(".{f:03}") };
            Ok(format!("P{d}DT{h}H{m}M{s}{frac}S"))
        }
        n => Err(format!("Format of a Duration with standard format {n} (not pinned)").into()),
    }
}

const MONTHS: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];

/// The standard formats of a Date, en-US (types/TestFormatEvaluateTextContracts.al:Format_Date_StandardFormats):
/// 0 and 1 `mm/dd/yy`, 2 `mmddyyD`, 3 `yy/mm/dd`, 4 `March 5, 2024`, 5 `mmddyy`, 6 `yymmdd`,
/// 7 `Mar 5, 2024`, 9 `yyyy-mm-dd`. Number 8 does not exist
/// (:Format_Date_Format8_DoesNotExist). A closing date is marked with `C` in the formats the corpus
/// shows it in (0, 1 and 2); the others are not pinned for one.
fn format_date(day: i32, closing: bool, standard: i32) -> Result<String, FormatFail> {
    let (y, m, d) = civil_from_day(day);
    let yy = y.rem_euclid(100);
    let c = if closing { "C" } else { "" };
    if closing && !matches!(standard, 0 | 1 | 2 | 9) {
        return Err(format!("Format of a closing date with standard format {standard} (not pinned)").into());
    }
    let month = MONTHS[m as usize - 1];
    Ok(match standard {
        0 | 1 => format!("{c}{m:02}/{d:02}/{yy:02}"),
        2 => format!("{m:02}{d:02}{yy:02}{c}D"),
        3 => format!("{yy:02}/{m:02}/{d:02}"),
        4 => format!("{month} {d}, {y}"),
        5 => format!("{m:02}{d:02}{yy:02}"),
        6 => format!("{yy:02}{m:02}{d:02}"),
        7 => format!("{} {d}, {y}", &month[..3]),
        8 => {
            return Err(FormatFail::Error {
                message: "Standard format number 8 does not exist for the type 'Date'.".into(),
                code: "InvalidStandardFormat".into(),
            });
        }
        9 => format!("{y:04}-{m:02}-{d:02}"),
        n => return Err(format!("Format of a Date with standard format {n} (not pinned)").into()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Format` for a value the tests expect to be pinned.
    fn format(v: &Value, length: i32, standard: i32, ctx: &dyn FormatCtx) -> String {
        try_format(v, length, standard, ctx).unwrap_or_else(|e| panic!("{e}"))
    }
    use rust_decimal::Decimal;
    use std::rc::Rc;
    use std::str::FromStr;

    struct Ctx;
    impl FormatCtx for Ctx {
        fn enum_name(&self, _id: u32, ordinal: i32) -> String {
            ["Open", "In Progress"][ordinal as usize].to_string()
        }
    }

    fn f(v: Value) -> String {
        format(&v, 0, 0, &Ctx)
    }
    fn f2(v: Value) -> String {
        format(&v, 0, 2, &Ctx)
    }
    fn dec(s: &str) -> Value {
        Value::Decimal(Decimal::from_str(s).unwrap())
    }

    #[test]
    fn integers() {
        // text/TestFormat.al: Format(42) = '42', Format(-7) = '-7', Format(0) = '0'.
        assert_eq!(f(Value::Integer(42)), "42");
        assert_eq!(f(Value::Integer(-7)), "-7");
        assert_eq!(f(Value::Integer(0)), "0");
        assert_eq!(f2(Value::BigInteger(-567)), "-567");
    }

    #[test]
    fn length_right_aligns() {
        // text/TestFormat.al: Format(42, 5) = '   42'.
        assert_eq!(format(&Value::Integer(42), 5, 0, &Ctx), "   42");
    }

    #[test]
    fn booleans() {
        // text/TestFormat.al: Format(true) = 'Yes', Format(false) = 'No' (the doc's
        // True/False is wrong for BC); <Standard Format,2> gives 1/0.
        assert_eq!(f(Value::Boolean(true)), "Yes");
        assert_eq!(f(Value::Boolean(false)), "No");
        assert_eq!(f2(Value::Boolean(true)), "1");
        assert_eq!(f2(Value::Boolean(false)), "0");
        // Doc: standard format 9 is true/false.
        assert_eq!(format(&Value::Boolean(true), 0, 9, &Ctx), "true");
    }

    #[test]
    fn text_and_code_are_themselves() {
        assert_eq!(f(Value::Text("ABC".into())), "ABC");
        assert_eq!(f2(Value::Code("X1".into())), "X1");
    }

    #[test]
    fn undefined_dates_and_times_are_empty() {
        // types/TestDate*.al etc.: Format(0D) = Format(0T) = Format(0DT) = ''.
        assert_eq!(f(Value::Date(0, false)), "");
        assert_eq!(f(Value::Time(0)), "");
        assert_eq!(f(Value::DateTime(0)), "");
        assert_eq!(f2(Value::Date(0, false)), "");
    }

    #[test]
    fn options_and_enums() {
        // record tests: Format(option 1 of " ,Draft") = 'Draft'; quoting stripped.
        let members: Rc<[String]> = Rc::from(vec!["".to_string(), "Draft".to_string()]);
        assert_eq!(f(Value::Option(1, members.clone())), "Draft");
        assert_eq!(
            f2(Value::Option(1, members)),
            "1",
            "doc: option standard format 2 is the number"
        );
        assert_eq!(f(Value::Enum(7, 1)), "In Progress");
        assert_eq!(f2(Value::Enum(7, 1)), "1");
    }

    #[test]
    fn guids_are_upper_case_in_braces() {
        // Corpus: '{63CA2FA4-4F03-4F2B-A480-172FEF340D3F}'.
        let g = [
            0x63, 0xca, 0x2f, 0xa4, 0x4f, 0x03, 0x4f, 0x2b, 0xa4, 0x80, 0x17, 0x2f, 0xef, 0x34, 0x0d, 0x3f,
        ];
        assert_eq!(f(Value::Guid(g)), "{63CA2FA4-4F03-4F2B-A480-172FEF340D3F}");
        assert_eq!(f2(Value::Guid(g)), "{63CA2FA4-4F03-4F2B-A480-172FEF340D3F}");
    }

    #[test]
    fn calendar_round_trips() {
        for (y, m, d) in [(1, 1, 1), (2021, 4, 5), (2000, 2, 29), (1900, 3, 1), (9999, 12, 31)] {
            assert_eq!(civil_from_day(day_from_civil(y, m, d)), (y, m, d));
        }
        assert_eq!(day_from_civil(1, 1, 1), 1);
    }

    #[test]
    fn dates_per_doc_table() {
        let day = day_from_civil(2021, 4, 5);
        assert_eq!(f2(Value::Date(day, false)), "040521D");
        assert_eq!(format(&Value::Date(day, false), 0, 9, &Ctx), "2021-04-05");
    }

    #[test]
    fn date_times_in_the_two_standard_formats_the_corpus_measured() {
        // pagesave/TestPageSaveDataLayer.al:199-206 (measured on BC): Format(_, 0, 2) of a DateTime
        // renders to the minute ('09/20/26 01:10 PM'); Format(_, 0, 9) is the round-trip form
        // ('2026-09-20T11:10:12.851Z'). The session time zone is taken to be UTC here.
        let day = i64::from(day_from_civil(2026, 9, 20)) - 1;
        let ms = day * 86_400_000 + (13 * 3600 + 10 * 60 + 12) * 1000 + 851;
        assert_eq!(f2(Value::DateTime(ms + 1)), "09/20/26 01:10 PM");
        assert_eq!(format(&Value::DateTime(ms + 1), 0, 9, &Ctx), "2026-09-20T13:10:12.851Z");
        let midnight = day * 86_400_000;
        assert_eq!(f2(Value::DateTime(midnight + 1)), "09/20/26 12:00 AM");
        // Two instants in the same minute format alike: Assert compares DateTimes at minute resolution.
        assert_eq!(f2(Value::DateTime(ms + 1)), f2(Value::DateTime(ms - 5_000 + 1)));
        assert_ne!(f2(Value::DateTime(ms + 1)), f2(Value::DateTime(ms + 120_000 + 1)));
    }

    #[test]
    fn times_are_distinct_per_minute_in_format_2() {
        // Assert.AreEqual/AreNotEqual compare Times through Format(_, 0, 2) (Assert.al Equal);
        // types/TestFormatEvaluateTextContracts.al:Format_Time_StandardFormats: format 2 is the AL literal.
        assert_eq!(f2(Value::Time(13 * 3_600_000 + 10 * 60_000 + 1)), "131000T");
        assert_eq!(f2(Value::Time(1)), "000000T");
    }

    #[test]
    fn the_standard_formats_the_probe_pins() {
        // types/TestFormatEvaluateTextContracts.al:Format_Decimal_StandardFormats, Format_Date_StandardFormats,
        // Format_Time_StandardFormats, Format_DateTime_StandardFormats, Format_Duration_StandardFormats.
        let fmt = |v: Value, n: i32| format(&v, 0, n, &Ctx);
        assert_eq!(fmt(dec("1234567.891"), 0), "1,234,567.891");
        for n in [1, 2, 9] {
            assert_eq!(fmt(dec("1234567.891"), n), "1234567.891");
        }
        let day = Value::Date(day_from_civil(2024, 3, 5), false);
        let want = [
            "03/05/24",
            "03/05/24",
            "030524D",
            "24/03/05",
            "March 5, 2024",
            "030524",
            "240305",
            "Mar 5, 2024",
        ];
        for (n, w) in want.iter().enumerate() {
            assert_eq!(fmt(day.clone(), n as i32), *w, "date format {n}");
        }
        assert_eq!(fmt(day.clone(), 9), "2024-03-05");
        let Err(FormatFail::Error { message, code }) = format_value(&day, 0, 8, &Ctx) else {
            panic!("format 8 must raise");
        };
        assert_eq!(message, "Standard format number 8 does not exist for the type 'Date'.");
        assert_eq!(code, "InvalidStandardFormat");
        let time = Value::Time(52_205_124);
        assert_eq!(fmt(time.clone(), 0), " 2:30:05 PM");
        assert_eq!(fmt(time.clone(), 1), " 2:30:05.123 PM");
        assert_eq!(fmt(time.clone(), 2), "143005.123T");
        assert_eq!(fmt(Value::Time(86_399_001), 2), "235959T");
        assert_eq!(fmt(time, 9), "14:30:05.123");
        assert_eq!(fmt(Value::Time(1), 0), "12:00:00 AM");
        let dt = Value::DateTime((i64::from(day_from_civil(2024, 3, 5)) - 1) * 86_400_000 + 52_205_123 + 1);
        assert_eq!(fmt(dt.clone(), 0), "03/05/24 02:30 PM");
        assert_eq!(fmt(dt.clone(), 1), "03/05/24 02:30:05.123 PM");
        assert_eq!(fmt(dt.clone(), 9), "2024-03-05T14:30:05.123Z");
        let midnight = Value::DateTime((i64::from(day_from_civil(2024, 3, 5)) - 1) * 86_400_000 + 1);
        assert_eq!(fmt(midnight, 9), "2024-03-05T00:00:00Z");
        let dur = Value::Duration(90_061_001);
        assert_eq!(fmt(dur.clone(), 0), "1 day 1 hour 1 minute 1 second 1 millisecond");
        assert_eq!(fmt(dur, 9), "P1DT1H1M1.001S");
        // json/TestJsonRuntimeContracts.al:214.
        assert_eq!(fmt(Value::Duration(5000), 9), "P0DT0H0M5.0S");
        // handlers/TestPageRuntimeContracts.al:162.
        assert_eq!(
            fmt(Value::Duration(-90_061_001), 0),
            "-1 day -1 hour -1 minute -1 second -1 millisecond"
        );
    }

    #[test]
    fn a_date_formula_prints_without_brackets() {
        // types/TestDateRoundFormatStringContracts.al:DateFormula_Evaluate_ThenFormat_PrintsTheNormalizedFormula.
        assert_eq!(f(Value::DateFormula("<1D>".into())), "1D");
        assert_eq!(f(Value::DateFormula("<+1M>".into())), "+1M");
        assert_eq!(f(Value::DateFormula("1D+1M".into())), "1D+1M");
    }

    #[test]
    fn decimals_in_code_constant_format() {
        // Doc: format 2 is <Sign><Integer><Decimals> with '.' as the separator.
        assert_eq!(f2(dec("-76543.21")), "-76543.21");
        assert_eq!(f2(dec("5")), "5");
        assert_eq!(f2(dec("1.50")), "1.5");
    }
    // Mutation-testing additions (docs/status.md debt).

    #[test]
    fn a_length_that_is_not_positive_or_not_longer_changes_nothing() {
        assert_eq!(pad("42".into(), 0), "42");
        assert_eq!(pad("42".into(), -1), "42");
        assert_eq!(pad("42".into(), -100), "42");
        assert_eq!(pad("42".into(), 2), "42");
        assert_eq!(pad("42".into(), 1), "42");
        assert_eq!(pad("42".into(), 3), " 42");
    }

    #[test]
    fn thousands_are_grouped_by_threes_from_the_right() {
        for (d, want) in [
            ("1", "1"),
            ("12", "12"),
            ("123", "123"),
            ("1234", "1,234"),
            ("12345", "12,345"),
            ("123456", "123,456"),
            ("1234567.89", "1,234,567.89"),
            ("-1234", "-1,234"),
            ("-999", "-999"),
        ] {
            assert_eq!(f(dec(d)), want, "{d}");
        }
        // Formats 2 and 9 never group.
        assert_eq!(f2(dec("1234567.5")), "1234567.5");
    }

    #[test]
    fn a_datetime_shows_its_milliseconds_exactly() {
        // The value is one more than the milliseconds since the start of 0001-01-01.
        let at = |ms: i64| format(&Value::DateTime(ms + 1), 0, 9, &Ctx);
        assert!(at(0).ends_with("00:00:00Z"), "{}", at(0));
        assert!(at(7).ends_with("00:00:00.007Z"), "{}", at(7));
        assert!(at(86_399_999).ends_with("23:59:59.999Z"), "{}", at(86_399_999));
        assert!(at(86_400_000).ends_with("00:00:00Z"), "{}", at(86_400_000));
    }

    #[test]
    fn a_library_value_without_a_pinned_format_is_refused() {
        let v = Value::Library("Foo".into(), Rc::default());
        assert!(try_format(&v, 0, 0, &Ctx).is_err());
        // A RecordId that holds nothing is empty, not an error.
        let r = Value::Library("RecordId".into(), Rc::default());
        assert_eq!(try_format(&r, 0, 0, &Ctx).unwrap(), "");
    }

    #[test]
    fn a_time_is_one_more_than_its_milliseconds_since_midnight() {
        // 12:00:00.000 is Time(43_200_001); the millisecond before it is still 11:59 AM.
        assert_eq!(f2(Value::Time(43_200_000)), "115959.999T");
        assert_eq!(f2(Value::Time(43_200_001)), "120000T");
        assert_eq!(f2(Value::Time(1)), "000000T");
        assert_eq!(f2(Value::Time(86_400_000)), "235959.999T");
    }
}
