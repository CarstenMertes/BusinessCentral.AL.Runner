//! `Format(Value, Length, FormatString)`: format strings with `<...>` codes.
//!
//! The format-string language is large. Only the codes a corpus test uses are understood:
//! `<Standard Format,N>` (types/TestFormatBooleanContracts.al:36,44), `<Integer>`
//! (json/TestJsonXmlDeepContracts.al:71) and the date and time codes `<Year4>`, `<Month,2>`,
//! `<Day,2>`, `<Hours24,2>`, `<Minutes,2>`, `<Seconds,2>` (handlers/TestPageTemporalSetValue_Page.al:47-65,
//! which only compare the result with the same call on another value). Their digits and padding
//! are the documented ones (docs/probes-needed.md). Any other code is refused.

use crate::format::{FormatCtx, civil_from_day, try_format};
use crate::value::Value;

/// `Err` names the part of the format string that is not understood.
pub fn format_with_string(v: &Value, length: i32, template: &str, ctx: &dyn FormatCtx) -> Result<String, String> {
    let mut out = String::new();
    let mut rest = template;
    while let Some(open) = rest.find('<') {
        out.push_str(&rest[..open]);
        let after = &rest[open + 1..];
        let Some(close) = after.find('>') else {
            return Err(format!("format string `{template}`: `<` without `>`"));
        };
        out.push_str(&code(v, &after[..close], ctx).map_err(|e| format!("format string `{template}`: {e}"))?);
        rest = &after[close + 1..];
    }
    out.push_str(rest);
    Ok(crate::format::pad(out, length))
}

fn code(v: &Value, code: &str, ctx: &dyn FormatCtx) -> Result<String, String> {
    if let Some(n) = code.strip_prefix("Standard Format,") {
        return match n.trim().parse::<i32>() {
            Ok(n) => try_format(v, 0, n, ctx),
            Err(_) => Err(format!("code `<{code}>`")),
        };
    }
    if code == "Integer" {
        return match v {
            Value::Integer(i) => Ok(i.to_string()),
            Value::BigInteger(i) => Ok(i.to_string()),
            other => Err(format!("code `<Integer>` on {other:?}")),
        };
    }
    let date = match v {
        Value::Date(n, false) if *n > 0 => Some(civil_from_day(*n)),
        Value::DateTime(t) if *t > 0 => Some(civil_from_day((t - 1).div_euclid(86_400_000) as i32 + 1)),
        _ => None,
    };
    let clock = match v {
        Value::Time(t) if *t > 0 => Some(t - 1),
        Value::DateTime(t) if *t > 0 => Some((t - 1).rem_euclid(86_400_000)),
        _ => None,
    };
    let part = match (code, date, clock) {
        ("Year4", Some((y, _, _)), _) => format!("{y:04}"),
        ("Month,2", Some((_, m, _)), _) => format!("{m:02}"),
        ("Day,2", Some((_, _, d)), _) => format!("{d:02}"),
        ("Hours24,2", _, Some(ms)) => format!("{:02}", ms / 3_600_000),
        ("Minutes,2", _, Some(ms)) => format!("{:02}", ms / 60_000 % 60),
        ("Seconds,2", _, Some(ms)) => format!("{:02}", ms / 1000 % 60),
        _ => return Err(format!("code `<{code}>` on {v:?}")),
    };
    Ok(part)
}

#[cfg(test)]
mod tests {
    use crate::format::day_from_civil;
    use crate::native::Signal;
    use crate::native::tests::{call, t};
    use crate::value::Value;

    fn fmt(v: Value, template: &str) -> Result<Value, Signal> {
        call("System.Format#2", vec![v, Value::Integer(0), t(template)])
    }
    fn text(r: Result<Value, Signal>) -> String {
        match r {
            Ok(Value::Text(s)) => s,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn standard_format_code() {
        // types/TestFormatBooleanContracts.al:36,44.
        assert_eq!(text(fmt(Value::Boolean(true), "<Standard Format,2>")), "1");
        assert_eq!(text(fmt(Value::Boolean(false), "<Standard Format,2>")), "0");
    }

    #[test]
    fn integer_code() {
        // json/TestJsonXmlDeepContracts.al:71.
        assert_eq!(text(fmt(Value::Integer(10), "<Integer>")), "10");
    }

    #[test]
    fn date_and_time_codes() {
        // handlers/TestPageTemporalSetValue_Page.al:47-65.
        let day = day_from_civil(2024, 3, 7);
        assert_eq!(
            text(fmt(Value::Date(day, false), "<Year4>-<Month,2>-<Day,2>")),
            "2024-03-07"
        );
        let dt = Value::DateTime((i64::from(day) - 1) * 86_400_000 + (14 * 3600 + 30 * 60 + 5) * 1000 + 1);
        assert_eq!(
            text(fmt(dt, "<Year4>-<Month,2>-<Day,2> <Hours24,2>:<Minutes,2>:<Seconds,2>")),
            "2024-03-07 14:30:05"
        );
        let time = Value::Time((14 * 3600 + 30 * 60) * 1000 + 1);
        assert_eq!(text(fmt(time, "<Hours24,2>:<Minutes,2>:<Seconds,2>")), "14:30:00");
    }

    #[test]
    fn other_codes_and_values_are_refused() {
        let day = Value::Date(day_from_civil(2024, 3, 7), false);
        for (v, f) in [
            (day.clone(), "<Day>"),
            (day.clone(), "<Hours24,2>"),
            (day.clone(), "<Year4"),
            (Value::Date(0, false), "<Year4>"),
            (Value::Decimal(1.into()), "<Integer>"),
            (Value::Integer(1), "<Standard Format,x>"),
        ] {
            assert!(matches!(fmt(v, f), Err(Signal::Unsupported(_))), "{f}");
        }
    }

    #[test]
    fn length_pads_the_result() {
        let r = call(
            "System.Format#2",
            vec![Value::Integer(10), Value::Integer(5), t("<Integer>")],
        );
        assert_eq!(text(r), "   10");
    }
}
