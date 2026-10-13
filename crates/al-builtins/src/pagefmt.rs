//! The text a page control shows for a value (ADR-012), en-US. This is not
//! `Format()`: a control spells Decimals with its own places and thousands
//! separators, Booleans as Yes/No, dates in the short forms, and so on.
//! Every rule cites the corpus test that measured it on real BC.

use crate::value::Value;
use rust_decimal::{Decimal, RoundingStrategy};

/// Which numbers a control blanks (`BlankNumbers`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Blank {
    #[default]
    DontBlank,
    Neg,
    NegAndZero,
    Zero,
    ZeroAndPos,
    Pos,
}

impl Blank {
    pub fn parse(s: &str) -> Option<Blank> {
        Some(match s.trim().to_ascii_lowercase().as_str() {
            "dontblank" => Blank::DontBlank,
            "blankneg" => Blank::Neg,
            "blanknegandzero" => Blank::NegAndZero,
            "blankzero" => Blank::Zero,
            "blankzeroandpos" => Blank::ZeroAndPos,
            "blankpos" => Blank::Pos,
            _ => return None,
        })
    }
}

/// What the page and the source field say about how a value is shown.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ControlFormat {
    /// `DecimalPlaces = min : max` (or the precision of an auto-format expression).
    pub decimal_places: Option<(u32, u32)>,
    pub blank_zero: bool,
    pub blank: Blank,
    /// `OptionCaption` of the control, else of the source field.
    pub option_captions: Option<Vec<String>>,
}

/// `2 : 2`, `0 : 5` or `3` as (min, max) places.
pub fn parse_decimal_places(p: &str) -> Option<(u32, u32)> {
    let mut it = p.split(':').map(|x| x.trim().parse::<u32>());
    let min = it.next()?.ok()?;
    let max = match it.next() {
        Some(m) => m.ok()?,
        None => min,
    };
    Some((min, max))
}

/// The precision of an auto-format expression: `<Precision,4:4><Standard Format,0>` is 4 : 4
/// (corpus autoformat/TestAutoFormat.al, AutoFormat_CustomExpressionControl_HonoursTheFormatString).
pub fn parse_precision(expr: &str) -> Option<(u32, u32)> {
    let rest = expr.split("<Precision,").nth(1)?;
    parse_decimal_places(rest.split('>').next()?)
}

/// `1234567.891` with (2, 2) as `1,234,567.89`: half away from zero, thousands separators.
pub fn decimal_text(d: &Decimal, places: (u32, u32)) -> String {
    let (min, max) = places;
    let rounded = d.round_dp_with_strategy(max, RoundingStrategy::MidpointAwayFromZero);
    let mut s = rounded.abs().to_string();
    if !s.contains('.') && min > 0 {
        s.push('.');
    }
    if let Some((_, frac)) = s.split_once('.') {
        let have = frac.len() as u32;
        // Trailing zeros beyond `min` places are dropped, missing ones up to `min` are added.
        let mut frac = frac.to_string();
        while (frac.len() as u32) > min && frac.ends_with('0') {
            frac.pop();
        }
        for _ in have.min(min)..min {
            if (frac.len() as u32) < min {
                frac.push('0');
            }
        }
        let int = s.split('.').next().unwrap_or("0").to_string();
        s = if frac.is_empty() { int } else { format!("{int}.{frac}") };
    }
    let (int, frac) = s.split_once('.').map_or((s.as_str(), None), |(i, f)| (i, Some(f)));
    let mut grouped = String::new();
    for (i, c) in int.chars().enumerate() {
        if i > 0 && (int.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(c);
    }
    let sign = if rounded.is_sign_negative() && !rounded.is_zero() {
        "-"
    } else {
        ""
    };
    match frac {
        Some(f) => format!("{sign}{grouped}.{f}"),
        None => format!("{sign}{grouped}"),
    }
}

fn blanked(f: &ControlFormat, sign: std::cmp::Ordering) -> bool {
    use std::cmp::Ordering::*;
    (f.blank_zero && sign == Equal)
        || match f.blank {
            Blank::DontBlank => false,
            Blank::Neg => sign == Less,
            Blank::NegAndZero => sign != Greater,
            Blank::Zero => sign == Equal,
            Blank::ZeroAndPos => sign != Less,
            Blank::Pos => sign == Greater,
        }
}

fn hour12(h: i64) -> (i64, &'static str) {
    (if h % 12 == 0 { 12 } else { h % 12 }, if h < 12 { "AM" } else { "PM" })
}

/// `12:34:56 PM` for a Time value (milliseconds since midnight plus one).
fn time_text(t: i64) -> String {
    let ms = t - 1;
    let (h, m, s) = (ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60);
    let (h12, ap) = hour12(h);
    format!("{h12}:{m:02}:{s:02} {ap}")
}

fn duration_text(ms: i64) -> String {
    if ms == 0 {
        return String::new();
    }
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
            parts.push(format!("{n} {unit}{}", if n == 1 { "" } else { "s" }));
        }
    }
    let text = parts.join(" ");
    if ms < 0 { format!("-{text}") } else { text }
}

fn guid_text(g: &[u8; 16]) -> String {
    let h: String = g.iter().map(|b| format!("{b:02x}")).collect();
    format!(
        "{{{}-{}-{}-{}-{}}}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// The text a control shows for `v`. `enum_caption` gives the caption of an enum value.
pub fn control_text(v: &Value, f: &ControlFormat, enum_caption: &dyn Fn(u32, i32) -> String) -> String {
    use std::cmp::Ordering;
    match v {
        Value::Null => String::new(),
        Value::Boolean(b) => if *b { "Yes" } else { "No" }.into(),
        Value::Integer(i) => {
            if blanked(f, i.cmp(&0)) {
                String::new()
            } else {
                i.to_string()
            }
        }
        Value::BigInteger(i) => {
            if blanked(f, i.cmp(&0)) {
                String::new()
            } else {
                i.to_string()
            }
        }
        Value::Byte(b) => b.to_string(),
        Value::Char(c) => char::from_u32(u32::from(*c)).map(String::from).unwrap_or_default(),
        Value::Decimal(d) => {
            let sign = d.cmp(&Decimal::ZERO);
            if blanked(f, sign) {
                String::new()
            } else {
                decimal_text(d, f.decimal_places.unwrap_or((2, 2)))
            }
        }
        Value::Text(t) | Value::Code(t) => t.clone(),
        Value::Option(o, members) => match &f.option_captions {
            Some(c) if (*o as usize) < c.len() => c[*o as usize].clone(),
            _ => members.get(*o as usize).cloned().unwrap_or_else(|| o.to_string()),
        },
        Value::Enum(id, o) => enum_caption(*id, *o),
        Value::Date(0, _) | Value::Time(0) | Value::DateTime(0) => String::new(),
        Value::Date(day, _) => {
            let (y, m, d) = crate::format::civil_from_day(*day);
            format!("{m}/{d}/{y}")
        }
        Value::Time(t) => time_text(*t),
        Value::DateTime(ms) => {
            let ms = ms - 1;
            let day = (ms.div_euclid(86_400_000)) as i32 + 1;
            let (y, mo, d) = crate::format::civil_from_day(day);
            let tod = ms.rem_euclid(86_400_000);
            let (h12, ap) = hour12(tod / 3_600_000);
            format!("{mo}/{d}/{y} {h12}:{:02} {ap}", tod / 60_000 % 60)
        }
        Value::Duration(ms) => duration_text(*ms),
        Value::Guid(g) => guid_text(g),
        Value::DateFormula(s) => s.clone(),
        other => {
            let _ = Ordering::Equal;
            format!("{other:?}")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::rc::Rc;
    use std::str::FromStr;

    fn d(s: &str) -> Value {
        Value::Decimal(Decimal::from_str(s).unwrap())
    }
    fn t(v: &Value, f: &ControlFormat) -> String {
        control_text(v, f, &|_, o| format!("enum{o}"))
    }
    fn places(min: u32, max: u32) -> ControlFormat {
        ControlFormat {
            decimal_places: Some((min, max)),
            ..Default::default()
        }
    }

    // handlers/TestPageFieldFormats_Tests.al Value_Decimal_TwoPlaces / NoDecimalPlaces
    #[test]
    fn decimals_default_to_two_places_with_thousands_and_half_away_from_zero_rounding() {
        let f = ControlFormat::default();
        for (v, want) in [
            ("0", "0.00"),
            ("999.5", "999.50"),
            ("1000", "1,000.00"),
            ("1234567.89", "1,234,567.89"),
            ("-1234567.89", "-1,234,567.89"),
            ("0.005", "0.01"),
            ("0.015", "0.02"),
            ("99999999999999.99", "99,999,999,999,999.99"),
            ("3.14159", "3.14"),
            ("1234567.891", "1,234,567.89"),
        ] {
            assert_eq!(t(&d(v), &f), want, "{v}");
        }
    }

    // Value_Decimal_OnePlace / FivePlaces / ZeroToFivePlaces
    #[test]
    fn decimal_places_set_minimum_and_maximum() {
        assert_eq!(t(&d("3.25"), &places(1, 1)), "3.3");
        assert_eq!(t(&d("1234.56"), &places(1, 1)), "1,234.6");
        assert_eq!(t(&d("0"), &places(1, 1)), "0.0");
        assert_eq!(t(&d("1.5"), &places(5, 5)), "1.50000");
        assert_eq!(t(&d("1234.12345"), &places(5, 5)), "1,234.12345");
        for (v, want) in [
            ("1000", "1,000"),
            ("1234.5", "1,234.5"),
            ("1.123456", "1.12346"),
            ("0", "0"),
        ] {
            assert_eq!(t(&d(v), &places(0, 5)), want, "{v}");
        }
    }

    // Value_Decimal_BlankZero / BlankZeroAndPositive / BlankNegative, Value_Integer_BlankZero
    #[test]
    fn blank_rules() {
        let bz = ControlFormat {
            blank_zero: true,
            ..Default::default()
        };
        assert_eq!(t(&d("0"), &bz), "");
        assert_eq!(t(&d("5"), &bz), "5.00");
        assert_eq!(t(&Value::Integer(0), &bz), "");
        assert_eq!(t(&Value::Integer(7), &bz), "7");
        let zp = ControlFormat {
            blank: Blank::ZeroAndPos,
            ..Default::default()
        };
        assert_eq!(
            (t(&d("-5"), &zp), t(&d("0"), &zp), t(&d("5"), &zp)),
            ("-5.00".into(), "".into(), "".into())
        );
        let neg = ControlFormat {
            blank: Blank::Neg,
            ..Default::default()
        };
        assert_eq!(
            (t(&d("-5"), &neg), t(&d("0"), &neg), t(&d("5"), &neg)),
            ("".into(), "0.00".into(), "5.00".into())
        );
    }

    // Value_Integer / BigInteger: plain digits, no thousands separator.
    #[test]
    fn integers_have_no_separator() {
        let f = ControlFormat::default();
        assert_eq!(t(&Value::Integer(1234567), &f), "1234567");
        assert_eq!(t(&Value::Integer(-1234567), &f), "-1234567");
        assert_eq!(t(&Value::BigInteger(9_000_000_000), &f), "9000000000");
    }

    // Value_Boolean, Value_Option (captions), Value_Guid
    #[test]
    fn booleans_options_guids() {
        let f = ControlFormat::default();
        assert_eq!(t(&Value::Boolean(true), &f), "Yes");
        assert_eq!(t(&Value::Boolean(false), &f), "No");
        let members: Rc<[String]> = Rc::from(vec!["Alpha".to_string(), "Beta".to_string()]);
        let cap = ControlFormat {
            option_captions: Some(vec!["Eins".into(), "Zwei".into()]),
            ..Default::default()
        };
        assert_eq!(t(&Value::Option(1, members.clone()), &cap), "Zwei");
        assert_eq!(t(&Value::Option(1, members), &f), "Beta");
        let g = [
            0x11, 0x11, 0x11, 0x11, 0x22, 0x22, 0x33, 0x33, 0x44, 0x44, 0x55, 0x55, 0x55, 0x55, 0x55, 0x55,
        ];
        assert_eq!(t(&Value::Guid(g), &f), "{11111111-2222-3333-4444-555555555555}");
        assert_eq!(t(&Value::Guid([0; 16]), &f), "{00000000-0000-0000-0000-000000000000}");
    }

    // Value_Date / Value_Time / Value_DateTime: the short forms, blank when undefined.
    #[test]
    fn dates_and_times_use_the_short_forms() {
        let f = ControlFormat::default();
        let day = crate::format::day_from_civil(2024, 3, 2);
        assert_eq!(t(&Value::Date(day, false), &f), "3/2/2024");
        assert_eq!(t(&Value::Date(0, false), &f), "");
        let tm = |h: i64, m: i64, s: i64| Value::Time(((h * 60 + m) * 60 + s) * 1000 + 1);
        assert_eq!(t(&tm(12, 34, 56), &f), "12:34:56 PM");
        assert_eq!(t(&tm(23, 59, 59), &f), "11:59:59 PM");
        assert_eq!(t(&tm(0, 0, 1), &f), "12:00:01 AM");
        assert_eq!(t(&Value::Time(0), &f), "");
        let dt = |y, m, d, h: i64, mi: i64| {
            Value::DateTime(
                i64::from(crate::format::day_from_civil(y, m, d) - 1) * 86_400_000 + (h * 60 + mi) * 60_000 + 1,
            )
        };
        assert_eq!(t(&dt(2024, 3, 2, 12, 34), &f), "3/2/2024 12:34 PM");
        assert_eq!(t(&dt(2024, 3, 2, 0, 0), &f), "3/2/2024 12:00 AM");
        assert_eq!(t(&dt(2024, 12, 31, 23, 59), &f), "12/31/2024 11:59 PM");
        assert_eq!(t(&Value::DateTime(0), &f), "");
    }

    // Value_Duration
    #[test]
    fn durations_are_spelled_in_words() {
        let f = ControlFormat::default();
        for (ms, want) in [
            (0, ""),
            (5000, "5 seconds"),
            (3_723_000, "1 hour 2 minutes 3 seconds"),
            (90_000_000, "1 day 1 hour"),
            (86_400_000, "1 day"),
            (1, "1 millisecond"),
        ] {
            assert_eq!(t(&Value::Duration(ms), &f), want, "{ms}");
        }
    }

    #[test]
    fn precision_and_decimal_places_parse() {
        assert_eq!(parse_decimal_places("2 : 2"), Some((2, 2)));
        assert_eq!(parse_decimal_places("0:5"), Some((0, 5)));
        assert_eq!(parse_decimal_places("3"), Some((3, 3)));
        assert_eq!(parse_precision("<Precision,4:4><Standard Format,0>"), Some((4, 4)));
        assert_eq!(parse_precision("EUR"), None);
    }
}
