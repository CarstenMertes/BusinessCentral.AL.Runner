//! `Round` and `Power`. Only the forms a corpus test pins are computed; the rest is refused.

use crate::native::{Ctx, NativeFn, Signal};
use crate::value::Value;
use rust_decimal::{Decimal, RoundingStrategy};

pub const REGISTRY: &[(&str, NativeFn)] = &[("System.Round#1", round), ("System.Power#1", power)];

type Out = Result<Value, Signal>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn dec(v: &Value) -> Option<Decimal> {
    match v {
        Value::Decimal(d) => Some(*d),
        Value::Integer(i) => Some(Decimal::from(*i)),
        Value::BigInteger(i) => Some(Decimal::from(*i)),
        Value::Byte(b) => Some(Decimal::from(*b)),
        _ => None,
    }
}

/// `Round(Number, Precision, Direction)`: the nearest multiple of `Precision`, halves away from
/// zero (session/TestSystemComplete.al:88-113, types/TestArithmeticContracts.al:126-130);
/// `'='` is the default direction (TestArithmeticContracts.al:108-115), `'>'` rounds away from zero and
/// `'<'` towards it, on negative numbers too (TestArithmeticContracts.al:82-97,
/// types/TestDateRoundFormatStringContracts.al:Round_DefaultPrecision_Directions_AndPrecisions). The
/// default precision is 0.01 (same test). A precision that is not positive is refused.
fn round(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    let Some(number) = dec(&a[0]) else {
        return unsupported(format!("Round of {:?}", a[0]));
    };
    let precision = match a.get(1) {
        None => Decimal::new(1, 2),
        Some(p) => match dec(p) {
            Some(p) => p,
            None => return unsupported(format!("Round with a Precision of {p:?}")),
        },
    };
    if precision <= Decimal::ZERO {
        return unsupported("Round with a Precision that is not positive (not pinned)");
    }
    let direction = match a.get(2) {
        None => "=".to_string(),
        Some(v) => crate::native::text_of(v).unwrap_or_default(),
    };
    let strategy = match direction.as_str() {
        "=" => RoundingStrategy::MidpointAwayFromZero,
        ">" => RoundingStrategy::AwayFromZero,
        "<" => RoundingStrategy::ToZero,
        other => return unsupported(format!("Round with direction `{other}` (not pinned)")),
    };
    let Some(steps) = number.checked_div(precision) else {
        return unsupported("Round outside the Decimal range");
    };
    let whole = steps.round_dp_with_strategy(0, strategy);
    match whole.checked_mul(precision) {
        Some(r) => Ok(Value::Decimal(r.normalize_to(precision))),
        None => unsupported("Round outside the Decimal range"),
    }
}

trait NormalizeTo {
    fn normalize_to(self, precision: Decimal) -> Decimal;
}

impl NormalizeTo for Decimal {
    /// Drops trailing zeros beyond the precision's own scale (`2.50` stays `2.50` for 0.01, `3` for 1).
    fn normalize_to(mut self, precision: Decimal) -> Decimal {
        let keep = precision.normalize().scale();
        if self.scale() > keep {
            self.rescale(keep);
        }
        self
    }
}

/// `Power(Number, Power)` with a whole exponent: `Power(2, 3)`, `(5, 0)`, `(1, 100)`, `(0, 0) = 1`,
/// `(2, -1) = 0.5`, `(2, 10)` (session/TestSystemComplete.al:124-144,
/// types/TestArithmeticContracts.al:46-73, types/TestDateRoundFormatStringContracts.al:Power_NonWholeAndNegativeExponents:
/// `(-8, 3) = -512`). Any other exponent is computed in floating point and read back as a Decimal
/// of 15 significant digits, as .NET converts a double (`Power(2, 0.5)` is `1.4142135623731`, same
/// test); a result that is not a finite number is refused, as is zero to a negative power.
fn power(_: &mut dyn Ctx, _: Option<&mut Value>, a: &mut [Value]) -> Out {
    let (Some(base), Some(exp)) = (dec(&a[0]), dec(&a[1])) else {
        return unsupported(format!("Power of {:?} and {:?}", a[0], a[1]));
    };
    if exp.fract() != Decimal::ZERO || exp.abs() > Decimal::from(10_000) {
        use rust_decimal::prelude::ToPrimitive;
        let (Some(b), Some(e)) = (base.to_f64(), exp.to_f64()) else {
            return unsupported("Power outside the double range");
        };
        let r = b.powf(e);
        if !r.is_finite() {
            return unsupported("Power with a result that is not a finite number (error not pinned)");
        }
        let Some(d) = Decimal::from_f64_retain(r)
            .and_then(|d| d.round_sf_with_strategy(15, RoundingStrategy::MidpointNearestEven))
        else {
            return unsupported("Power outside the Decimal range");
        };
        return Ok(Value::Decimal(d.normalize()));
    }
    let n = i64::try_from(exp.trunc().mantissa() / 10i128.pow(exp.scale())).unwrap_or(i64::MAX);
    let mut result = Decimal::ONE;
    for _ in 0..n.unsigned_abs() {
        result = match result.checked_mul(base) {
            Some(r) => r,
            None => return unsupported("Power outside the Decimal range"),
        };
    }
    if n < 0 {
        result = match Decimal::ONE.checked_div(result) {
            Some(r) => r,
            None => return unsupported("Power with a zero base and a negative exponent (error not pinned)"),
        };
    }
    Ok(Value::Decimal(result.normalize()))
}

#[cfg(test)]
mod tests {
    use crate::native::Signal;
    use crate::native::tests::{call, t};
    use crate::value::Value;
    use rust_decimal::Decimal;
    use std::str::FromStr;

    fn d(s: &str) -> Value {
        Value::Decimal(Decimal::from_str(s).unwrap())
    }
    fn is_dec(r: Result<Value, Signal>, want: &str) {
        match r {
            Ok(Value::Decimal(got)) => assert_eq!(got, Decimal::from_str(want).unwrap(), "got {got}"),
            other => panic!("{other:?}"),
        }
    }
    fn refused(r: Result<Value, Signal>) {
        assert!(matches!(r, Err(Signal::Unsupported(_))), "{r:?} should be a refusal");
    }

    #[test]
    fn round_half_away_from_zero() {
        // session/TestSystemComplete.al:88,100,112; types/TestArithmeticContracts.al:126-130.
        is_dec(call("System.Round#1", vec![d("2.5"), Value::Integer(1)]), "3");
        is_dec(call("System.Round#1", vec![d("-2.5"), Value::Integer(1)]), "-3");
        is_dec(call("System.Round#1", vec![d("3.14159"), d("0.01")]), "3.14");
        // record/TestSalesQuoteVatRounding.al:45-47.
        is_dec(call("System.Round#1", vec![d("8.437"), d("0.01")]), "8.44");
        is_dec(call("System.Round#1", vec![d("7.337"), d("0.01")]), "7.34");
        is_dec(call("System.Round#1", vec![d("14.3454"), d("0.01")]), "14.35");
    }

    #[test]
    fn round_directions() {
        // types/TestArithmeticContracts.al:82-115.
        is_dec(call("System.Round#1", vec![d("2.5"), Value::Integer(1), t(">")]), "3");
        is_dec(call("System.Round#1", vec![d("2.5"), Value::Integer(1), t("<")]), "2");
        is_dec(call("System.Round#1", vec![d("2.6"), Value::Integer(1), t("=")]), "3");
        is_dec(call("System.Round#1", vec![d("2.4"), Value::Integer(1), t("=")]), "2");
        is_dec(call("System.Round#1", vec![d("2.5"), Value::Integer(1), t("=")]), "3");
    }

    #[test]
    fn round_refuses_what_the_corpus_does_not_pin() {
        refused(call("System.Round#1", vec![d("2.5"), Value::Integer(0)]));
        refused(call("System.Round#1", vec![d("2.5"), Value::Integer(1), t("x")]));
    }

    #[test]
    fn power_with_a_whole_exponent() {
        let p = |a: i32, b: i32| call("System.Power#1", vec![Value::Integer(a), Value::Integer(b)]);
        is_dec(p(2, 3), "8");
        is_dec(p(5, 0), "1");
        is_dec(p(1, 100), "1");
        is_dec(p(0, 0), "1");
        is_dec(p(2, -1), "0.5");
        is_dec(p(2, 10), "1024");
        is_dec(call("System.Power#1", vec![d("1.5"), Value::Integer(2)]), "2.25");
    }

    #[test]
    fn power_refuses_what_the_corpus_does_not_pin() {
        refused(call("System.Power#1", vec![Value::Integer(0), Value::Integer(-1)]));
        refused(call("System.Power#1", vec![Value::Integer(10), Value::Integer(100)]));
    }
    // Mutation-testing additions (docs/status.md debt).

    #[test]
    fn round_takes_every_whole_number_type() {
        is_dec(
            call("System.Round#1", vec![Value::BigInteger(25), Value::Integer(10)]),
            "30",
        );
        is_dec(call("System.Round#1", vec![Value::Byte(7), Value::Integer(5)]), "5");
        is_dec(
            call("System.Round#1", vec![Value::Integer(7), Value::BigInteger(5)]),
            "5",
        );
        is_dec(call("System.Round#1", vec![Value::Integer(7), Value::Byte(5)]), "5");
    }

    #[test]
    fn a_direction_on_zero_is_not_a_negative_number() {
        // The refusal is for numbers below zero only; zero rounds like any other number.
        is_dec(call("System.Round#1", vec![d("0"), Value::Integer(1), t(">")]), "0");
        is_dec(call("System.Round#1", vec![d("0"), Value::Integer(1), t("<")]), "0");
    }

    #[test]
    fn the_result_keeps_the_scale_of_the_precision_and_no_more() {
        let scale = |r: Result<Value, Signal>| match r {
            Ok(Value::Decimal(d)) => d.scale(),
            other => panic!("{other:?}"),
        };
        // 2.50 for a precision of 0.01, 3 for 1, and a precision written 0.50 counts as 0.5.
        assert_eq!(scale(call("System.Round#1", vec![d("2.5"), d("0.01")])), 2);
        assert_eq!(scale(call("System.Round#1", vec![d("2.5"), Value::Integer(1)])), 0);
        assert_eq!(scale(call("System.Round#1", vec![d("1.4"), d("0.50")])), 1);
        is_dec(call("System.Round#1", vec![d("1.4"), d("0.50")]), "1.5");
    }

    #[test]
    fn power_exponent_limit_and_the_scale_of_a_written_exponent() {
        let p = |a: &str, b: Value| call("System.Power#1", vec![d(a), b]);
        // Up to 10000 the product is exact; past it the result comes from a double, and one that
        // is not a finite number is refused.
        is_dec(p("1", Value::Integer(10_000)), "1");
        is_dec(p("1", Value::Integer(10_001)), "1");
        refused(p("2", Value::Integer(10_001)));
    }

    // FOUND BY MUTATION TESTING (not fixed here: this task changes no runtime behaviour). `power`
    // reads the whole exponent as `exp.trunc().mantissa() / 10^exp.scale()`. `trunc()` already
    // drops the scale, so an exponent written with one (3.0, or 6 / 2 computed as 3.0) is divided
    // by ten once too often: Power(2, 3.0) answers 1, not 8. The fix is
    // `exp.trunc().to_i64()`. Remove the #[ignore] with the fix.
    #[test]
    #[ignore = "bug: Power with an exponent that carries a scale (3.0) answers 1"]
    fn power_exponent_written_with_a_scale_is_still_a_whole_number() {
        let p = |a: &str, b: Value| call("System.Power#1", vec![d(a), b]);
        is_dec(p("2", d("3.0")), "8");
        is_dec(p("2", d("-2.00")), "0.25");
    }
}
