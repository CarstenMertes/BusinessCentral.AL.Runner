//! `Format(Value [, Length] [, FormatNumber])`. Every rule cites the corpus
//! test or the doc table ("Formatting values, dates, and time") that pins it;
//! where the two disagree the corpus wins (it runs on real BC).

use crate::value::Value;

/// What formatting needs from the running program.
pub trait FormatCtx {
    /// Member name of enum `id` at `ordinal`.
    fn enum_name(&self, id: u32, ordinal: i32) -> String;
}

/// `Format(v, length, standard_format)`. Panics on a value or format no corpus test pins
/// yet; builtins use [`try_format`] so that becomes a refusal instead.
pub fn format(v: &Value, length: i32, standard: i32, ctx: &dyn FormatCtx) -> String {
    try_format(v, length, standard, ctx).unwrap_or_else(|e| unimplemented!("{e}"))
}

/// `Err` names the value whose format is not pinned yet.
pub fn try_format(v: &Value, length: i32, standard: i32, ctx: &dyn FormatCtx) -> Result<String, String> {
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
        Value::Decimal(d) => format_decimal(d, standard),
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
        Value::Date(day, closing) => format_date(*day, *closing, standard),
        Value::Time(t) if standard == 2 => format_time_12h(t - 1),
        Value::DateTime(t) if standard == 2 || standard == 9 => format_datetime(t - 1, standard),
        Value::DateFormula(f) => f.clone(),
        // A RecordId reads `Table: key1,key2` (corpus record/TestRecordRecordId.al:42-56 only needs
        // the key values in it; the layout is docs/probes-needed.md).
        Value::Library(name, l) if name == "RecordId" => match &*l.borrow() {
            crate::value::LibraryValue::RecordId { name, key, .. } => {
                let key = key
                    .iter()
                    .map(|k| try_format(k, 0, 0, ctx))
                    .collect::<Result<Vec<_>, _>>()?;
                format!("{name}: {}", key.join(","))
            }
            _ => String::new(),
        },
        // Duration and the other reference types get their formats with
        // the corpus tests that pin them.
        other => {
            return Err(format!(
                "Format of {other:?} (standard format {standard}) is not pinned yet"
            ));
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

/// Format 0/1: `<Sign><Integer Thousand><Decimals>` (en-US doc table, region
/// unverified: docs/probes-needed.md); formats 2 and 9: `<Sign><Integer><Decimals>`
/// with `.`.
fn format_decimal(d: &rust_decimal::Decimal, standard: i32) -> String {
    let n = d.normalize();
    let plain = n.to_string();
    if standard == 2 || standard == 9 {
        return plain;
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
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
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

/// `hh:mm AM/PM` for a time of day in milliseconds. Assert compares Times with `Format(_, 0, 2)`;
/// the text itself is not pinned (docs/probes-needed.md).
fn format_time_12h(ms_of_day: i64) -> String {
    let (h, m) = (ms_of_day / 3_600_000, ms_of_day / 60_000 % 60);
    format!(
        "{:02}:{m:02} {}",
        if h % 12 == 0 { 12 } else { h % 12 },
        if h < 12 { "AM" } else { "PM" }
    )
}

/// Format 2 renders to the minute (`09/20/26 01:10 PM`) and format 9 is the round-trip form
/// (`2026-09-20T11:10:12.851Z`), both measured on BC in pagesave/TestPageSaveDataLayer.al:199-206.
/// The session time zone is taken to be UTC.
fn format_datetime(ms: i64, standard: i32) -> String {
    let day = ms.div_euclid(86_400_000);
    let in_day = ms.rem_euclid(86_400_000);
    let (y, m, d) = civil_from_day(day as i32 + 1);
    if standard == 9 {
        let (h, mi, s, f) = (
            in_day / 3_600_000,
            in_day / 60_000 % 60,
            in_day / 1000 % 60,
            in_day % 1000,
        );
        format!("{y:04}-{m:02}-{d:02}T{h:02}:{mi:02}:{s:02}.{f:03}Z")
    } else {
        format!("{m:02}/{d:02}/{:02} {}", y.rem_euclid(100), format_time_12h(in_day))
    }
}

/// en-US doc table (region unverified: docs/probes-needed.md): format 0/1
/// `<Closing><Month,2>/<Day,2>/<Year>`, format 2 `<Month,2><Day,2><Year><Closing>D`,
/// format 9 `yyyy-mm-dd`.
fn format_date(day: i32, closing: bool, standard: i32) -> String {
    let (y, m, d) = civil_from_day(day);
    let yy = y.rem_euclid(100);
    let c = if closing { "C" } else { "" };
    match standard {
        2 => format!("{m:02}{d:02}{yy:02}{c}D"),
        9 => format!("{y:04}-{m:02}-{d:02}"),
        _ => format!("{c}{m:02}/{d:02}/{yy:02}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
        // the exact text is not pinned (docs/probes-needed.md).
        assert_eq!(f2(Value::Time(13 * 3_600_000 + 10 * 60_000 + 1)), "01:10 PM");
        assert_eq!(f2(Value::Time(1)), "12:00 AM");
    }

    #[test]
    fn decimals_in_code_constant_format() {
        // Doc: format 2 is <Sign><Integer><Decimals> with '.' as the separator.
        assert_eq!(f2(dec("-76543.21")), "-76543.21");
        assert_eq!(f2(dec("5")), "5");
        assert_eq!(f2(dec("1.50")), "1.5");
    }
}
