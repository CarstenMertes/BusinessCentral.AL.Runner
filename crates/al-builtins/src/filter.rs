//! Record filters as data (shared by the VM and the store).

use crate::ops::compare;
use crate::value::Value;
use std::cmp::Ordering;

#[derive(Debug, Clone)]
pub enum FilterExpr {
    /// `SetRange(F, A, B)` and `SetRange(F, A)` (as A..A); open ends are None.
    Range(Option<Value>, Option<Value>),
    Eq(Value),
    Ne(Value),
    Lt(Value),
    Le(Value),
    Gt(Value),
    Ge(Value),
    Or(Vec<FilterExpr>),
    And(Vec<FilterExpr>),
}

impl FilterExpr {
    /// Whether a field value passes. Values that cannot be compared do not pass.
    pub fn matches(&self, v: &Value) -> bool {
        let cmp = |x: &Value| compare(v, x).ok();
        match self {
            FilterExpr::Range(lo, hi) => {
                lo.as_ref()
                    .is_none_or(|l| matches!(cmp(l), Some(Ordering::Equal | Ordering::Greater)))
                    && hi
                        .as_ref()
                        .is_none_or(|h| matches!(cmp(h), Some(Ordering::Equal | Ordering::Less)))
            }
            FilterExpr::Eq(x) => cmp(x) == Some(Ordering::Equal),
            FilterExpr::Ne(x) => matches!(cmp(x), Some(Ordering::Less | Ordering::Greater)),
            FilterExpr::Lt(x) => cmp(x) == Some(Ordering::Less),
            FilterExpr::Le(x) => matches!(cmp(x), Some(Ordering::Less | Ordering::Equal)),
            FilterExpr::Gt(x) => cmp(x) == Some(Ordering::Greater),
            FilterExpr::Ge(x) => matches!(cmp(x), Some(Ordering::Greater | Ordering::Equal)),
            FilterExpr::Or(xs) => xs.iter().any(|x| x.matches(v)),
            FilterExpr::And(xs) => xs.iter().all(|x| x.matches(v)),
        }
    }
}

/// A filter on the field at position `field` of the table's field list.
#[derive(Debug, Clone)]
pub struct FieldFilter {
    pub field: usize,
    pub expr: FilterExpr,
}

/// Total order on key tuples (field-wise `compare`; incomparable counts as equal).
pub fn key_cmp(a: &[Value], b: &[Value]) -> Ordering {
    for (x, y) in a.iter().zip(b) {
        match compare(x, y).unwrap_or(Ordering::Equal) {
            Ordering::Equal => continue,
            o => return o,
        }
    }
    a.len().cmp(&b.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_and_comparisons() {
        let r = FilterExpr::Range(Some(Value::Integer(2)), Some(Value::Integer(4)));
        assert!(!r.matches(&Value::Integer(1)));
        assert!(r.matches(&Value::Integer(2)));
        assert!(r.matches(&Value::Integer(4)));
        assert!(!r.matches(&Value::Integer(5)));
        assert!(FilterExpr::Range(None, Some(Value::Integer(3))).matches(&Value::Integer(-9)));
        assert!(
            FilterExpr::Or(vec![
                FilterExpr::Eq(Value::Integer(1)),
                FilterExpr::Gt(Value::Integer(8))
            ])
            .matches(&Value::Integer(9))
        );
        assert!(!FilterExpr::Ne(Value::Text("a".into())).matches(&Value::Text("a".into())));
    }

    #[test]
    fn key_order() {
        assert_eq!(
            key_cmp(
                &[Value::Integer(1), Value::Text("b".into())],
                &[Value::Integer(1), Value::Text("a".into())]
            ),
            Ordering::Greater
        );
        assert_eq!(
            key_cmp(&[Value::Code("A".into())], &[Value::Code("A".into())]),
            Ordering::Equal
        );
    }
}
