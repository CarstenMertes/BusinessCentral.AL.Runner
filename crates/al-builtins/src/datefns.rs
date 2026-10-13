//! Date, Time and DateTime builtins: `DMY2Date`, `CreateDateTime`, `Date2DMY`, `CalcDate`, ...
//!
//! Representations follow `Value`: a Date is a day number (1 = 0001-01-01) and a closing flag, a
//! Time is milliseconds since midnight plus one, a DateTime is milliseconds since 0001-01-01 UTC
//! plus one (0 is the undefined value of each). The session time zone is taken to be UTC, as in
//! `format.rs` (docs/probes-needed.md).
//!
//! Every function accepts the forms a corpus test pins and refuses the rest with
//! `Signal::Unsupported`, never a guess.

use crate::format::{civil_from_day, day_from_civil};
use crate::native::{Ctx, NativeFn, Signal};
use crate::value::Value;

pub const REGISTRY: &[(&str, NativeFn)] = &[
    ("System.DMY2Date#1", dmy2date),
    ("System.CreateDateTime#1", create_datetime),
    ("System.DT2Date#1", dt2date),
    ("System.DT2Time#1", dt2time),
    ("System.DaTi2Variant#1", create_datetime),
    ("System.Date2DMY#1", date2dmy),
    ("System.NormalDate#1", normal_date),
    ("System.ClosingDate#1", closing_date),
    ("System.CalcDate#1", calcdate_formula),
    ("System.CalcDate#2", calcdate_text),
    ("Date.DayOfWeek#1", day_of_week),
    ("Date.WeekNo#1", week_no),
    ("Time.Hour#1", |_, r, _| time_part(r, 3_600_000, 24)),
    ("Time.Minute#1", |_, r, _| time_part(r, 60_000, 60)),
    ("Time.Second#1", |_, r, _| time_part(r, 1000, 60)),
    ("Time.Millisecond#1", |_, r, _| time_part(r, 1, 1000)),
];

type Out = Result<Value, Signal>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

const MS_PER_DAY: i64 = 86_400_000;
const LAST_DAY: i32 = 3_652_059; // 9999-12-31

fn days_in_month(y: i32, m: u32) -> u32 {
    match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        _ if (y % 4 == 0 && y % 100 != 0) || y % 400 == 0 => 29,
        _ => 28,
    }
}

fn int_of(v: Option<&Value>) -> Option<i32> {
    match v? {
        Value::Integer(n) => Some(*n),
        _ => None,
    }
}

/// A defined, non-closing date's day number.
fn plain_date(v: &Value, what: &str) -> Result<i32, Signal> {
    match v {
        Value::Date(n, false) if *n > 0 => Ok(*n),
        other => unsupported(format!(
            "{what} of {other:?}: only a defined, non-closing Date is pinned"
        )),
    }
}

/// `DMY2Date(Day, Month, Year)` with all three parts, which is every form the corpus calls
/// (fieldref/TestFieldRefValue.al:122, record/TestDateVirtualTableFlowField.al:65,83).
/// A part outside the calendar raises in BC with a text no corpus test pins: refused.
fn dmy2date(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    let (Some(d), Some(m), Some(y)) = (int_of(a.first()), int_of(a.get(1)), int_of(a.get(2))) else {
        return unsupported("DMY2Date without all of Day, Month and Year (not pinned by the corpus)");
    };
    if !(1..=9999).contains(&y) || !(1..=12).contains(&m) || d < 1 || d as u32 > days_in_month(y, m as u32) {
        return unsupported(format!(
            "DMY2Date({d}, {m}, {y}) is not a calendar date (error text not pinned)"
        ));
    }
    Ok(Value::Date(day_from_civil(y, m as u32, d as u32), false))
}

/// `CreateDateTime(Date, Time)`. A defined date with `0T` is midnight
/// (handlers/TestPageTemporalSetValue_Tests.al:85). An undefined date is not pinned.
fn create_datetime(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    let day = plain_date(&a[0], "CreateDateTime")?;
    let Value::Time(t) = a[1] else {
        return unsupported(format!("CreateDateTime with {:?}", a[1]));
    };
    let time_of_day = (t - 1).max(0);
    Ok(Value::DateTime((i64::from(day) - 1) * MS_PER_DAY + time_of_day + 1))
}

fn defined_datetime(v: &Value, what: &str) -> Result<i64, Signal> {
    match v {
        Value::DateTime(n) if *n > 0 => Ok(*n - 1),
        other => unsupported(format!("{what} of {other:?}: only a defined DateTime is pinned")),
    }
}

/// handlers/TestPageTemporalSetValue_Tests.al:118.
fn dt2date(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    let ms = defined_datetime(&a[0], "DT2Date")?;
    Ok(Value::Date(ms.div_euclid(MS_PER_DAY) as i32 + 1, false))
}

/// handlers/TestPageTemporalSetValue_Tests.al:117.
fn dt2time(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    let ms = defined_datetime(&a[0], "DT2Time")?;
    Ok(Value::Time(ms.rem_euclid(MS_PER_DAY) + 1))
}

/// `Date2DMY(Date, 1|2|3)`: day, month, year (session/TestCalcDateNotificationContracts.al:21,22,32).
fn date2dmy(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    let day = plain_date(&a[0], "Date2DMY")?;
    let (y, m, d) = civil_from_day(day);
    match int_of(a.get(1)) {
        Some(1) => Ok(Value::Integer(d as i32)),
        Some(2) => Ok(Value::Integer(m as i32)),
        Some(3) => Ok(Value::Integer(y)),
        other => unsupported(format!("Date2DMY with Value {other:?}: only 1, 2 and 3 are pinned")),
    }
}

/// record/TestDateVirtualTable.al:188-190: a closing date gives its plain day, a plain date is unchanged.
fn normal_date(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    match &a[0] {
        Value::Date(n, _) if *n > 0 => Ok(Value::Date(*n, false)),
        other => unsupported(format!("NormalDate of {other:?}")),
    }
}

/// record/TestFilterContracts.al:56,58.
fn closing_date(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    match &a[0] {
        Value::Date(n, _) if *n > 0 => Ok(Value::Date(*n, true)),
        other => unsupported(format!("ClosingDate of {other:?}")),
    }
}

/// 1 = Monday (session/TestSystemExtended.al:278). Day 1 (0001-01-01) is a Monday.
fn weekday(day: i32) -> i32 {
    (day - 1).rem_euclid(7) + 1
}

fn receiver_date(r: Option<&mut Value>, what: &str) -> Result<i32, Signal> {
    match r {
        Some(v) => plain_date(v, what),
        None => unsupported(format!("{what} without a receiver")),
    }
}

fn day_of_week(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    Ok(Value::Integer(weekday(receiver_date(r, "Date.DayOfWeek")?)))
}

/// The ISO 8601 week: the week that holds the year's first Thursday is week 1. The corpus pins
/// the range and the first week of a year (session/TestSystemExtended.al:285-300); the rule
/// itself is the documented one (docs/probes-needed.md).
fn week_no(_: &mut dyn Ctx, r: Option<&mut Value>, _: &mut [Value]) -> Out {
    let day = receiver_date(r, "Date.WeekNo")?;
    let thursday = day + (4 - weekday(day));
    let (y, _, _) = civil_from_day(thursday);
    Ok(Value::Integer((thursday - day_from_civil(y, 1, 1)) / 7 + 1))
}

/// `Time.Hour/Minute/Second/Millisecond` (session/TestMiscTypes.al:22-42). The undefined time is not pinned.
fn time_part(r: Option<&mut Value>, unit_ms: i64, modulo: i64) -> Out {
    match r {
        Some(Value::Time(t)) if *t > 0 => Ok(Value::Integer((((*t - 1) / unit_ms) % modulo) as i32)),
        other => unsupported(format!("a Time part of {other:?}: only a defined Time is pinned")),
    }
}

// ---- CalcDate ------------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Unit {
    Day,
    Month,
    Year,
    Quarter,
}

/// One term of a date formula.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Term {
    /// `+nD`, `-nM`, `nY`: move by `n` units.
    Step(Unit, i32),
    /// `CM`, `CQ`, `CY`: the end of the current period; `-CM`, `-CY`: its start.
    Edge { unit: Unit, end: bool },
}

/// Parses `<term...>` where a term is `[+|-][digits](D|M|Y)` or `[+|-]C(M|Y)` / `CQ`.
/// These are the forms the corpus pins (types/TestDate.al:28,38; session/TestDateTimeContracts.al:19-47;
/// session/TestCalcDateNotificationContracts.al:20-80; types/TestDateFormulaContracts.al:44-125).
/// Everything else in the formula language (weeks, quarters as steps, weekdays, `CD`, `CW`,
/// `-CQ`, no brackets) is refused.
fn parse_formula(text: &str) -> Result<Vec<Term>, Signal> {
    let refuse = || unsupported(format!("date formula `{text}`: this form is not pinned by the corpus"));
    let Some(body) = text.strip_prefix('<').and_then(|t| t.strip_suffix('>')) else {
        return refuse();
    };
    let b = body.as_bytes();
    let mut terms = Vec::new();
    let mut i = 0;
    while i < b.len() {
        // After the first term, every term carries its sign (`<1M-1D>`); `<1M1D>` is not pinned.
        if i > 0 && !matches!(b[i], b'+' | b'-') {
            return refuse();
        }
        let negative = match b[i] {
            b'-' => {
                i += 1;
                true
            }
            b'+' => {
                i += 1;
                false
            }
            _ => false,
        };
        if b.get(i) == Some(&b'C') {
            let unit = match b.get(i + 1) {
                Some(b'M') => Unit::Month,
                Some(b'Y') => Unit::Year,
                Some(b'Q') if !negative => Unit::Quarter,
                _ => return refuse(),
            };
            terms.push(Term::Edge { unit, end: !negative });
            i += 2;
            continue;
        }
        let start = i;
        while b.get(i).is_some_and(u8::is_ascii_digit) {
            i += 1;
        }
        if start == i {
            return refuse();
        }
        let Ok(n) = body[start..i].parse::<i32>() else {
            return refuse();
        };
        let unit = match b.get(i) {
            Some(b'D') => Unit::Day,
            Some(b'M') => Unit::Month,
            Some(b'Y') => Unit::Year,
            _ => return refuse(),
        };
        i += 1;
        terms.push(Term::Step(unit, if negative { -n } else { n }));
    }
    if terms.is_empty() {
        return refuse();
    }
    Ok(terms)
}

/// Whether `text` is a date formula `Evaluate` can store (see [`parse_formula`]).
pub fn check_formula(text: &str) -> Result<(), Signal> {
    parse_formula(text).map(|_| ())
}

fn apply(terms: &[Term], from: i32) -> Result<i32, Signal> {
    let mut day = from;
    for term in terms {
        let (y, m, d) = civil_from_day(day);
        day = match *term {
            Term::Step(Unit::Day, n) => day.checked_add(n).unwrap_or(0),
            Term::Step(Unit::Month, n) => {
                let total = i64::from(y) * 12 + i64::from(m) - 1 + i64::from(n);
                let (ny, nm) = (total.div_euclid(12), total.rem_euclid(12) as u32 + 1);
                if !(1..=9999).contains(&ny) {
                    return unsupported("CalcDate result outside 0001-9999 (error text not pinned)");
                }
                if d > days_in_month(ny as i32, nm) {
                    // Only the Feb 29 clamp of `<1Y>` is pinned (session/TestDateTimeContracts.al:38).
                    return unsupported("CalcDate month step that clamps the day (not pinned by the corpus)");
                }
                day_from_civil(ny as i32, nm, d)
            }
            Term::Step(Unit::Year, n) => {
                let ny = i64::from(y) + i64::from(n);
                if !(1..=9999).contains(&ny) {
                    return unsupported("CalcDate result outside 0001-9999 (error text not pinned)");
                }
                day_from_civil(ny as i32, m, d.min(days_in_month(ny as i32, m)))
            }
            Term::Step(Unit::Quarter, _) => return unsupported("CalcDate quarter step"),
            Term::Edge { unit: Unit::Month, end } => day_from_civil(y, m, if end { days_in_month(y, m) } else { 1 }),
            Term::Edge { unit: Unit::Year, end } => {
                if end {
                    day_from_civil(y, 12, 31)
                } else {
                    day_from_civil(y, 1, 1)
                }
            }
            Term::Edge {
                unit: Unit::Quarter, ..
            } => {
                let last = (m - 1) / 3 * 3 + 3;
                day_from_civil(y, last, days_in_month(y, last))
            }
            Term::Edge { unit: Unit::Day, .. } => return unsupported("CalcDate CD"),
        };
        if !(1..=LAST_DAY).contains(&day) {
            return unsupported("CalcDate result outside 0001-9999 (error text not pinned)");
        }
    }
    Ok(day)
}

fn calcdate(formula: &str, base: Option<&Value>) -> Out {
    let terms = parse_formula(formula)?;
    let Some(base) = base else {
        return unsupported("CalcDate without a date uses the work date, which is not available");
    };
    let from = plain_date(base, "CalcDate")?;
    Ok(Value::Date(apply(&terms, from)?, false))
}

fn calcdate_text(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    match crate::native::text_of(&a[0]) {
        Some(f) => calcdate(&f, a.get(1)),
        None => unsupported("CalcDate with a formula that is not text"),
    }
}

fn calcdate_formula(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    match &a[0] {
        Value::DateFormula(f) => calcdate(f, a.get(1)),
        other => unsupported(format!("CalcDate with {other:?}")),
    }
}

#[cfg(test)]
mod tests {
    use crate::format::day_from_civil;
    use crate::native::tests::{TestCtx, call, t};
    use crate::native::{Signal, lookup};
    use crate::value::Value;

    fn date(y: i32, m: u32, d: u32) -> Value {
        Value::Date(day_from_civil(y, m, d), false)
    }
    fn is_date(r: Result<Value, Signal>, y: i32, m: u32, d: u32) {
        match r {
            Ok(Value::Date(n, false)) => assert_eq!(n, day_from_civil(y, m, d), "wrong date"),
            other => panic!("{other:?}"),
        }
    }
    fn refused(r: Result<Value, Signal>) {
        assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?} should be a refusal");
    }
    fn int(r: Result<Value, Signal>) -> i32 {
        match r {
            Ok(Value::Integer(i)) => i,
            other => panic!("{other:?}"),
        }
    }
    fn on(id: &str, recv: Value) -> Result<Value, Signal> {
        let f = lookup(id).unwrap_or_else(|| panic!("{id} not registered"));
        let mut recv = recv;
        f(&mut TestCtx::default(), Some(&mut recv), &mut [])
    }
    fn calc(formula: &str, from: Value) -> Result<Value, Signal> {
        call("System.CalcDate#2", vec![t(formula), from])
    }

    #[test]
    fn dmy2date_knows_the_length_of_every_month_and_the_leap_year_rules() {
        let d = |a: i32, b: i32, c: i32| {
            call(
                "System.DMY2Date#1",
                vec![Value::Integer(a), Value::Integer(b), Value::Integer(c)],
            )
        };
        for (month, days) in [(1, 31), (2, 28), (3, 31), (4, 30), (5, 31), (6, 30), (7, 31), (8, 31), (9, 30), (10, 31), (11, 30), (12, 31)] {
            is_date(d(days, month, 2023), 2023, month as u32, days as u32);
            refused(d(days + 1, month, 2023));
        }
        // Leap years: divisible by 4, not by 100 unless by 400.
        is_date(d(29, 2, 2024), 2024, 2, 29);
        is_date(d(29, 2, 2000), 2000, 2, 29);
        refused(d(29, 2, 1900));
        refused(d(29, 2, 2023));
        refused(d(30, 2, 2024));
    }

    #[test]
    fn dmy2date_builds_a_date() {
        // fieldref/TestFieldRefValue.al:122 `DMY2Date(1, 1, 2024)`; record/TestDateVirtualTableFlowField.al:65
        // `DMY2Date(3, 1, 1)` and :83 `DMY2Date(31, 12, 9999)` are the ends of the range.
        let d = |a: i32, b: i32, c: i32| {
            call(
                "System.DMY2Date#1",
                vec![Value::Integer(a), Value::Integer(b), Value::Integer(c)],
            )
        };
        is_date(d(1, 1, 2024), 2024, 1, 1);
        is_date(d(31, 12, 9999), 9999, 12, 31);
        is_date(d(3, 1, 1), 1, 1, 3);
        is_date(d(29, 2, 2024), 2024, 2, 29);
    }

    #[test]
    fn dmy2date_refuses_what_the_corpus_does_not_pin() {
        let d = |args: Vec<i32>| call("System.DMY2Date#1", args.into_iter().map(Value::Integer).collect());
        refused(d(vec![30, 2, 2024]));
        refused(d(vec![1, 13, 2024]));
        refused(d(vec![1, 1, 0]));
        refused(d(vec![1, 1, 10000]));
        refused(d(vec![1]));
        refused(d(vec![1, 1]));
    }

    #[test]
    fn create_datetime_joins_date_and_time() {
        // json/TestJsonTypedGetters.al:351 `CreateDateTime(Today(), 120000T)`;
        // handlers/TestPageTemporalSetValue_Tests.al:85 pairs a date with 0T (midnight).
        let day = i64::from(day_from_civil(2024, 1, 15)) - 1;
        let noon = Value::Time(43_200_000 + 1);
        assert!(matches!(
            call("System.CreateDateTime#1", vec![date(2024, 1, 15), noon]),
            Ok(Value::DateTime(n)) if n == day * 86_400_000 + 43_200_000 + 1
        ));
        assert!(matches!(
            call("System.CreateDateTime#1", vec![date(2024, 1, 15), Value::Time(0)]),
            Ok(Value::DateTime(n)) if n == day * 86_400_000 + 1
        ));
        refused(call(
            "System.CreateDateTime#1",
            vec![Value::Date(0, false), Value::Time(1)],
        ));
        refused(call(
            "System.CreateDateTime#1",
            vec![Value::Date(0, false), Value::Time(0)],
        ));
        refused(call(
            "System.CreateDateTime#1",
            vec![Value::Date(5, true), Value::Time(1)],
        ));
    }

    #[test]
    fn dt2date_and_dt2time_split_a_datetime() {
        // handlers/TestPageTemporalSetValue_Tests.al:117-118.
        let dt = call(
            "System.CreateDateTime#1",
            vec![date(2024, 3, 17), Value::Time(52_200_000 + 1)],
        )
        .unwrap();
        is_date(call("System.DT2Date#1", vec![dt.clone()]), 2024, 3, 17);
        assert!(matches!(
            call("System.DT2Time#1", vec![dt]),
            Ok(Value::Time(52_200_001))
        ));
        refused(call("System.DT2Date#1", vec![Value::DateTime(0)]));
        refused(call("System.DT2Time#1", vec![Value::DateTime(0)]));
    }

    #[test]
    fn dati2variant_is_a_datetime() {
        // session/TestSystemExtended.al:168.
        assert!(matches!(
            call("System.DaTi2Variant#1", vec![date(2024, 1, 1), Value::Time(43_200_001)]),
            Ok(Value::DateTime(_))
        ));
    }

    #[test]
    fn date2dmy_picks_a_part() {
        // session/TestCalcDateNotificationContracts.al:61 `Date2DMY(D, 1)` is the day, :79 `..., 2)` the
        // month; :32 `..., 3)` the year.
        let part = |n: i32| int(call("System.Date2DMY#1", vec![date(2024, 3, 15), Value::Integer(n)]));
        assert_eq!((part(1), part(2), part(3)), (15, 3, 2024));
        refused(call("System.Date2DMY#1", vec![date(2024, 3, 15), Value::Integer(4)]));
        refused(call(
            "System.Date2DMY#1",
            vec![Value::Date(0, false), Value::Integer(1)],
        ));
    }

    #[test]
    fn normal_date_and_closing_date() {
        // record/TestDateVirtualTable.al:188-190 and record/TestFilterContracts.al:56.
        assert!(matches!(
            call("System.ClosingDate#1", vec![date(2024, 12, 31)]),
            Ok(Value::Date(n, true)) if n == day_from_civil(2024, 12, 31)
        ));
        let closing = call("System.ClosingDate#1", vec![date(2026, 3, 31)]).unwrap();
        is_date(call("System.NormalDate#1", vec![closing]), 2026, 3, 31);
        is_date(call("System.NormalDate#1", vec![date(2026, 3, 1)]), 2026, 3, 1);
        refused(call("System.ClosingDate#1", vec![Value::Date(0, false)]));
    }

    #[test]
    fn day_of_week_counts_from_monday() {
        // session/TestSystemExtended.al:278 `20240101D.DayOfWeek()` is 1 (Monday).
        assert_eq!(int(on("Date.DayOfWeek#1", date(2024, 1, 1))), 1);
        assert_eq!(int(on("Date.DayOfWeek#1", date(2024, 1, 7))), 7);
        assert_eq!(int(on("Date.DayOfWeek#1", date(2024, 1, 3))), 3);
        refused(on("Date.DayOfWeek#1", Value::Date(0, false)));
    }

    #[test]
    fn week_no_is_the_iso_week() {
        // session/TestSystemExtended.al:285-300 pins the range 1..53 and the first week of a year.
        assert_eq!(int(on("Date.WeekNo#1", date(2024, 1, 1))), 1);
        assert_eq!(int(on("Date.WeekNo#1", date(2024, 1, 15))), 3);
        assert_eq!(int(on("Date.WeekNo#1", date(2021, 1, 3))), 53);
        assert_eq!(int(on("Date.WeekNo#1", date(2024, 12, 30))), 1);
    }

    #[test]
    fn time_parts() {
        // session/TestMiscTypes.al:22-42.
        let t = Value::Time(((12 * 60 + 30) * 60 + 5) * 1000 + 250 + 1);
        assert_eq!(int(on("Time.Hour#1", t.clone())), 12);
        assert_eq!(int(on("Time.Minute#1", t.clone())), 30);
        assert_eq!(int(on("Time.Second#1", t.clone())), 5);
        assert_eq!(int(on("Time.Millisecond#1", t)), 250);
        refused(on("Time.Hour#1", Value::Time(0)));
    }

    #[test]
    fn calcdate_day_month_year_steps() {
        // types/TestDate.al:28-38 (`<+1D>`, `<-1D>`), session/TestDateTimeContracts.al:29 (`<-1M>`),
        // :38 and :47 (`<1Y>` clamps Feb 29 to Feb 28).
        is_date(calc("<+1D>", date(2024, 1, 1)), 2024, 1, 2);
        is_date(calc("<-1D>", date(2024, 1, 2)), 2024, 1, 1);
        is_date(calc("<1D>", date(2024, 2, 28)), 2024, 2, 29);
        is_date(calc("<+1D>", date(2024, 2, 29)), 2024, 3, 1);
        is_date(calc("<-1M>", date(2024, 3, 1)), 2024, 2, 1);
        is_date(calc("<1M>", date(2024, 1, 1)), 2024, 2, 1);
        is_date(calc("<1Y>", date(2024, 2, 29)), 2025, 2, 28);
        is_date(calc("<1Y>", date(2024, 2, 28)), 2025, 2, 28);
        is_date(calc("<-1M>", date(2024, 1, 15)), 2023, 12, 15);
    }

    #[test]
    fn calcdate_ends_of_periods() {
        // session/TestCalcDateNotificationContracts.al:16-80.
        is_date(calc("<CM>", date(2024, 3, 15)), 2024, 3, 31);
        is_date(calc("<CM>", date(2024, 2, 15)), 2024, 2, 29);
        is_date(calc("<-CM>", date(2024, 3, 15)), 2024, 3, 1);
        is_date(calc("<CY>", date(2024, 3, 15)), 2024, 12, 31);
        is_date(calc("<-CY>", date(2024, 3, 15)), 2024, 1, 1);
        is_date(calc("<CQ>", date(2024, 2, 15)), 2024, 3, 31);
    }

    #[test]
    fn calcdate_terms_apply_left_to_right() {
        // session/TestCalcDateNotificationContracts.al:30 `<1M-1D>` from the first of a month stays in it.
        is_date(calc("<1M-1D>", date(2024, 3, 1)), 2024, 3, 31);
        is_date(calc("<CM+1D>", date(2024, 3, 10)), 2024, 4, 1);
    }

    #[test]
    fn calcdate_refuses_forms_the_corpus_does_not_pin() {
        let from = date(2024, 3, 15);
        for f in [
            "<1W>", "<CW>", "<CD>", "<-CQ>", "<1Q>", "<MON>", "<1W+MON>", "<WD1>", "<2CM>", "<>", "", "1D", "<1X>",
            "<D>", "<+>", "<1D", "<1M1D>",
        ] {
            refused(calc(f, from.clone()));
        }
        // Month arithmetic that would clamp the day, a missing or closing base date, a result off the calendar.
        refused(calc("<1M>", date(2024, 1, 31)));
        refused(calc("<1D>", Value::Date(0, false)));
        refused(calc("<1D>", Value::Date(5, true)));
        refused(calc("<1Y>", date(9999, 6, 1)));
        refused(calc("<-1Y>", date(1, 6, 1)));
        refused(call("System.CalcDate#2", vec![t("<1D>")]));
    }

    #[test]
    fn calcdate_takes_a_date_formula() {
        // types/TestDateFormulaContracts.al:45-125.
        let c = |f: &str, d| call("System.CalcDate#1", vec![Value::DateFormula(f.into()), d]);
        is_date(c("<1D>", date(2024, 1, 1)), 2024, 1, 2);
        is_date(c("<1M>", date(2024, 1, 1)), 2024, 2, 1);
        is_date(c("<-1D>", date(2024, 1, 2)), 2024, 1, 1);
        is_date(c("<1Y>", date(2024, 1, 1)), 2025, 1, 1);
        refused(c("", date(2024, 1, 1)));
    }

    #[test]
    fn evaluate_reads_a_date_formula() {
        // types/TestDateFormulaContracts.al:21,31,43,139: Evaluate then Format, CalcDate.
        let mut args = vec![Value::DateFormula(String::new()), t("<1D>")];
        let f = lookup("System.Evaluate#1").unwrap();
        assert!(matches!(
            f(&mut TestCtx::default(), None, &mut args),
            Ok(Value::Boolean(true))
        ));
        assert!(matches!(&args[0], Value::DateFormula(s) if s == "<1D>"));
        let mut args = vec![Value::DateFormula(String::new()), t("not a formula")];
        refused(f(&mut TestCtx::default(), None, &mut args));
    }
}
