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
    /// `*` / `?` wildcards on text; `@` makes it case-insensitive.
    Like(String, bool),
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
            FilterExpr::Like(p, ci) => crate::native::text_of(v).is_some_and(|t| like(&t, p, *ci)),
            FilterExpr::Or(xs) => xs.iter().any(|x| x.matches(v)),
            FilterExpr::And(xs) => xs.iter().all(|x| x.matches(v)),
        }
    }
}

fn like(text: &str, pattern: &str, ci: bool) -> bool {
    let (t, p): (Vec<char>, Vec<char>) = if ci {
        (
            text.to_lowercase().chars().collect(),
            pattern.to_lowercase().chars().collect(),
        )
    } else {
        (text.chars().collect(), pattern.chars().collect())
    };
    fn go(t: &[char], p: &[char]) -> bool {
        match p.first() {
            None => t.is_empty(),
            Some('*') => (0..=t.len()).any(|i| go(&t[i..], &p[1..])),
            Some('?') => !t.is_empty() && go(&t[1..], &p[1..]),
            Some(c) => t.first() == Some(c) && go(&t[1..], &p[1..]),
        }
    }
    go(&t, &p)
}

/// Parses a `SetFilter` expression for a field of type `ty`. `%1`.. take the
/// typed `args`. Forms the corpus does not exercise yet return an error.
pub fn parse_filter(text: &str, args: &[crate::value::Value], ty: &crate::value::RtType) -> Result<FilterExpr, String> {
    let toks = tokenize(text, args)?;
    let mut p = Parser { toks, i: 0, ty };
    let e = p.or()?;
    if p.i != p.toks.len() {
        return Err(format!("unexpected `{:?}` in filter `{text}`", p.toks[p.i]));
    }
    Ok(e)
}

#[derive(Debug, Clone)]
enum Tok {
    /// Unquoted text (may hold wildcards).
    Raw(String),
    /// Quoted literal: no operators or wildcards inside.
    Quoted(String),
    /// A `%n` placeholder's typed value.
    Typed(Value),
    Op(&'static str),
}

fn tokenize(text: &str, args: &[Value]) -> Result<Vec<Tok>, String> {
    let c: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let mut raw = String::new();
    let flush = |raw: &mut String, out: &mut Vec<Tok>| {
        let t = raw.trim();
        if !t.is_empty() {
            out.push(Tok::Raw(t.to_string()));
        }
        raw.clear();
    };
    let mut i = 0;
    while i < c.len() {
        let rest: String = c[i..].iter().take(2).collect();
        let op = ["..", "<>", "<=", ">="]
            .iter()
            .find(|o| rest.starts_with(**o))
            .copied()
            .or_else(|| match c[i] {
                '|' => Some("|"),
                '&' => Some("&"),
                '<' => Some("<"),
                '>' => Some(">"),
                '=' => Some("="),
                '(' => Some("("),
                ')' => Some(")"),
                '@' => Some("@"),
                _ => None,
            });
        if let Some(op) = op {
            flush(&mut raw, &mut out);
            out.push(Tok::Op(op));
            i += op.len();
        } else if c[i] == '\'' {
            flush(&mut raw, &mut out);
            let mut lit = String::new();
            i += 1;
            loop {
                match c.get(i) {
                    None => return Err("unterminated quote in filter".into()),
                    Some('\'') if c.get(i + 1) == Some(&'\'') => {
                        lit.push('\'');
                        i += 2;
                    }
                    Some('\'') => {
                        i += 1;
                        break;
                    }
                    Some(ch) => {
                        lit.push(*ch);
                        i += 1;
                    }
                }
            }
            out.push(Tok::Quoted(lit));
        } else if c[i] == '%' && c.get(i + 1).is_some_and(|d| d.is_ascii_digit()) {
            flush(&mut raw, &mut out);
            let n = c[i + 1].to_digit(10).expect("digit") as usize;
            out.push(Tok::Typed(
                args.get(n.wrapping_sub(1))
                    .cloned()
                    .ok_or("filter placeholder without an argument")?,
            ));
            i += 2;
        } else {
            raw.push(c[i]);
            i += 1;
        }
    }
    flush(&mut raw, &mut out);
    Ok(out)
}

struct Parser<'a> {
    toks: Vec<Tok>,
    i: usize,
    ty: &'a crate::value::RtType,
}

impl Parser<'_> {
    fn peek_op(&self) -> Option<&'static str> {
        match self.toks.get(self.i) {
            Some(Tok::Op(o)) => Some(o),
            _ => None,
        }
    }

    fn or(&mut self) -> Result<FilterExpr, String> {
        let mut xs = vec![self.and()?];
        while self.peek_op() == Some("|") {
            self.i += 1;
            xs.push(self.and()?);
        }
        Ok(if xs.len() == 1 {
            xs.pop().expect("one")
        } else {
            FilterExpr::Or(xs)
        })
    }

    fn and(&mut self) -> Result<FilterExpr, String> {
        let mut xs = vec![self.term()?];
        while self.peek_op() == Some("&") {
            self.i += 1;
            xs.push(self.term()?);
        }
        Ok(if xs.len() == 1 {
            xs.pop().expect("one")
        } else {
            FilterExpr::And(xs)
        })
    }

    fn value(&mut self) -> Option<Tok> {
        match self.toks.get(self.i) {
            Some(Tok::Op(_)) | None => None,
            Some(t) => {
                self.i += 1;
                Some(t.clone())
            }
        }
    }

    fn typed(&self, t: Tok) -> Result<Value, String> {
        use crate::value::RtType as T;
        let text = match t {
            Tok::Typed(v) => return crate::ops::convert(v, self.ty).map_err(|e| format!("{e:?}")),
            Tok::Raw(s) | Tok::Quoted(s) => s,
            Tok::Op(o) => return Err(format!("operator `{o}` where a value was expected")),
        };
        Ok(match self.ty {
            T::Text(_) => Value::Text(text),
            T::Code(_) => Value::Code(text.trim().to_uppercase()),
            T::Integer => Value::Integer(text.parse().map_err(|_| format!("`{text}` is not an Integer"))?),
            T::BigInteger => Value::BigInteger(text.parse().map_err(|_| format!("`{text}` is not a BigInteger"))?),
            T::Decimal => Value::Decimal(text.parse().map_err(|_| format!("`{text}` is not a Decimal"))?),
            // Format(false) = 'No' is used as a filter value in the corpus.
            T::Boolean if ["yes", "true"].contains(&text.to_lowercase().as_str()) => Value::Boolean(true),
            T::Boolean if ["no", "false"].contains(&text.to_lowercase().as_str()) => Value::Boolean(false),
            T::Option(m) => match m.iter().position(|x| x.eq_ignore_ascii_case(&text)) {
                Some(i) => Value::Option(i as i32, m.clone().into()),
                None => Value::Option(
                    text.parse().map_err(|_| format!("`{text}` is not an option member"))?,
                    m.clone().into(),
                ),
            },
            other => {
                return Err(format!(
                    "text filter value for {other:?} (region-dependent or not pinned yet)"
                ));
            }
        })
    }

    /// A single value, which may be a wildcard pattern on text fields.
    fn single(&mut self, t: Tok, case_insensitive: bool) -> Result<FilterExpr, String> {
        use crate::value::RtType as T;
        if let (Tok::Raw(s), T::Text(_) | T::Code(_)) = (&t, self.ty) {
            let s = if matches!(self.ty, T::Code(_)) && !case_insensitive {
                s.to_uppercase()
            } else {
                s.clone()
            };
            if s.contains(['*', '?']) || case_insensitive {
                return Ok(FilterExpr::Like(s, case_insensitive));
            }
        }
        Ok(FilterExpr::Eq(self.typed(t)?))
    }

    fn term(&mut self) -> Result<FilterExpr, String> {
        match self.peek_op() {
            Some("(") => {
                self.i += 1;
                let e = self.or()?;
                if self.peek_op() != Some(")") {
                    return Err("missing `)` in filter".into());
                }
                self.i += 1;
                Ok(e)
            }
            Some("@") => {
                self.i += 1;
                let v = self.value().ok_or("value expected after @")?;
                self.single(v, true)
            }
            Some(op @ ("<>" | "<=" | ">=" | "<" | ">" | "=")) => {
                self.i += 1;
                let v = match self.value() {
                    Some(v) => self.typed(v)?,
                    None if matches!(self.ty, crate::value::RtType::Text(_) | crate::value::RtType::Code(_)) => {
                        Value::Text(String::new())
                    }
                    None => return Err(format!("value expected after `{op}`")),
                };
                Ok(match op {
                    "<>" => FilterExpr::Ne(v),
                    "<=" => FilterExpr::Le(v),
                    ">=" => FilterExpr::Ge(v),
                    "<" => FilterExpr::Lt(v),
                    ">" => FilterExpr::Gt(v),
                    _ => FilterExpr::Eq(v),
                })
            }
            Some("..") => {
                self.i += 1;
                let hi = self.value().ok_or("value expected after ..")?;
                Ok(FilterExpr::Range(None, Some(self.typed(hi)?)))
            }
            _ => {
                let Some(lo) = self.value() else {
                    // An empty filter value (`''` was a quoted empty literal and is a value).
                    return Err("empty filter term".into());
                };
                if self.peek_op() == Some("..") {
                    self.i += 1;
                    let lo = self.typed(lo)?;
                    let hi = match self.value() {
                        Some(h) => Some(self.typed(h)?),
                        None => None,
                    };
                    return Ok(FilterExpr::Range(Some(lo), hi));
                }
                self.single(lo, false)
            }
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

    use crate::value::RtType;

    fn pf(text: &str, args: &[Value], ty: RtType) -> FilterExpr {
        parse_filter(text, args, &ty).unwrap_or_else(|e| panic!("{text}: {e}"))
    }
    fn ints(f: &FilterExpr, vals: &[i32]) -> Vec<bool> {
        vals.iter().map(|v| f.matches(&Value::Integer(*v))).collect()
    }
    fn texts(f: &FilterExpr, vals: &[&str]) -> Vec<bool> {
        vals.iter().map(|v| f.matches(&Value::Code(v.to_string()))).collect()
    }

    #[test]
    fn filter_ranges_and_comparisons() {
        assert_eq!(
            ints(&pf("20..30", &[], RtType::Integer), &[19, 20, 30, 31]),
            [false, true, true, false]
        );
        assert_eq!(ints(&pf("..5", &[], RtType::Integer), &[-1, 5, 6]), [true, true, false]);
        assert_eq!(ints(&pf("5..", &[], RtType::Integer), &[4, 5, 99]), [false, true, true]);
        assert_eq!(ints(&pf(">=1", &[], RtType::Integer), &[0, 1]), [false, true]);
        assert_eq!(ints(&pf(">0", &[], RtType::Integer), &[0, 1]), [false, true]);
        assert_eq!(ints(&pf("<>3", &[], RtType::Integer), &[2, 3]), [true, false]);
        assert_eq!(ints(&pf("1|3", &[], RtType::Integer), &[1, 2, 3]), [true, false, true]);
    }

    #[test]
    fn filter_placeholders_take_typed_values() {
        assert_eq!(
            ints(&pf("<%1", &[Value::Integer(10)], RtType::Integer), &[9, 10]),
            [true, false]
        );
        assert_eq!(
            ints(
                &pf("%1|%2", &[Value::Integer(4), Value::Integer(7)], RtType::Integer),
                &[4, 5, 7]
            ),
            [true, false, true]
        );
        let f = pf(
            "%1..%2",
            &[Value::Date(10, false), Value::Date(20, false)],
            RtType::Date,
        );
        assert!(f.matches(&Value::Date(15, false)) && !f.matches(&Value::Date(21, false)));
    }

    #[test]
    fn filter_wildcards_on_code_follow_code_normalisation() {
        assert_eq!(
            texts(&pf("AP*", &[], RtType::Code(Some(20))), &["APPLE", "XAP"]),
            [true, false]
        );
        // Values convert to the field type: on a Code field 'ap*' means 'AP*'.
        assert_eq!(texts(&pf("ap*", &[], RtType::Code(Some(20))), &["APPLE"]), [true]);
        assert_eq!(
            texts(&pf("A", &[], RtType::Code(Some(20))), &["A", "AB"]),
            [true, false]
        );
    }

    #[test]
    fn filter_empty_and_quoted_literals() {
        let f = pf("''", &[], RtType::Text(None));
        assert!(f.matches(&Value::Text(String::new())) && !f.matches(&Value::Text("x".into())));
        let f = pf("<>''", &[], RtType::Text(None));
        assert!(!f.matches(&Value::Text(String::new())) && f.matches(&Value::Text("x".into())));
        let f = pf("'Smith & Sons'", &[], RtType::Text(None));
        assert!(f.matches(&Value::Text("Smith & Sons".into())));
        let f = pf("'(Travel|Meals)'", &[], RtType::Text(None));
        assert!(f.matches(&Value::Text("(Travel|Meals)".into())));
    }

    #[test]
    fn filter_forms_not_pinned_yet_are_errors() {
        assert!(
            parse_filter("01-15-24", &[], &RtType::Date).is_err(),
            "text dates depend on the region"
        );
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
