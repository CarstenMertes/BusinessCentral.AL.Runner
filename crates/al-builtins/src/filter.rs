//! Record filters as data (shared by the VM and the store).

use crate::describe::Describe;
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
    parse_filter_with(text, args, ty, &|_| Vec::new())
}

/// Like [`parse_filter`], with the (ordinal, name) members of an enum object
/// so that filters on enum fields can name members.
/// Prefix of the parse error for a word a Boolean filter does not accept; the rest is the word.
pub const INVALID_BOOLEAN: &str = "invalid Boolean filter value: ";

pub fn parse_filter_with(
    text: &str,
    args: &[crate::value::Value],
    ty: &crate::value::RtType,
    enums: &dyn Fn(u32) -> Vec<(i32, String)>,
) -> Result<FilterExpr, String> {
    let toks = tokenize(text, args)?;
    let mut p = Parser { toks, i: 0, ty, enums };
    let e = p.or()?;
    if p.i != p.toks.len() {
        return Err(format!("unexpected {} in filter `{text}`", p.toks[p.i].describe()));
    }
    Ok(e)
}

#[derive(Debug, Clone)]
enum Tok {
    /// Unquoted text (may hold wildcards).
    Raw(String),
    /// Quoted expression: no filter operators inside (`@` and wildcards stay active).
    Quoted(String),
    /// A `%n` placeholder's typed value.
    Typed(Value),
    Op(&'static str),
}

impl Describe for Tok {
    fn describe(&self) -> String {
        match self {
            Tok::Raw(s) | Tok::Quoted(s) => format!("`{s}`"),
            Tok::Typed(v) => v.describe(),
            Tok::Op(o) => format!("`{o}`"),
        }
    }
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
                    // A placeholder inside the quotes is replaced like any other (the filter text '''%1''|''%2''' in
                    // Gen. Jnl.-Show CT Entries filters a blank currency as ''|''). A text or an Integer is put in as it
                    // reads; any other type is not pinned (registry entry filter-placeholder-in-quotes).
                    Some('%') if c.get(i + 1).is_some_and(|d| d.is_ascii_digit()) => {
                        let n = c[i + 1].to_digit(10).expect("digit") as usize;
                        let arg = args
                            .get(n.wrapping_sub(1))
                            .ok_or("filter placeholder without an argument")?;
                        match arg {
                            Value::Text(t) | Value::Code(t) => lit.push_str(t),
                            Value::Integer(v) => lit.push_str(&v.to_string()),
                            other => {
                                return Err(format!(
                                    "placeholder %{n} inside quotes holds {}: only a text or an Integer is put in (not pinned)",
                                    other.describe()
                                ));
                            }
                        }
                        crate::assumptions::assume(crate::assumptions::Assumption::FilterPlaceholderInQuotes);
                        i += 2;
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
            let arg = args
                .get(n.wrapping_sub(1))
                .cloned()
                .ok_or("filter placeholder without an argument")?;
            // A text substituted for a placeholder is TEXT, not a filter, except that `*` and `?` stay
            // wildcards and a leading `@` ignores case. `&`, `|`, `(`, `)`, `<`, `=` and `..` in it make
            // BC quote the whole value (record/TestSetFilterSpecialTextContracts.al, BC 27.5); `>` is
            // not pinned, nor a text with both a wildcard and one of those.
            if let Value::Text(t) | Value::Code(t) = &arg {
                let (at, body) = match t.strip_prefix('@') {
                    Some(rest) => (true, rest),
                    None => (false, t.as_str()),
                };
                let special = body.contains(['&', '|', '(', ')', '<', '=']) || body.contains("..");
                let wildcard = body.contains(['*', '?']);
                if body.contains('>') || (special && wildcard) || body.starts_with('@') {
                    let ch = body.chars().find(|c| ">*?@".contains(*c)).unwrap_or('>');
                    return Err(format!(
                        "placeholder %{n} holds a text with the filter character `{ch}`: how BC quotes it is not pinned"
                    ));
                }
                if at {
                    out.push(Tok::Op("@"));
                }
                if special {
                    out.push(Tok::Quoted(body.to_string()));
                } else if !body.is_empty() {
                    out.push(Tok::Raw(body.to_string()));
                } else {
                    out.push(Tok::Typed(arg.clone()));
                }
                i += 2;
                continue;
            }
            out.push(Tok::Typed(arg));
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
    enums: &'a dyn Fn(u32) -> Vec<(i32, String)>,
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
            Tok::Typed(v) => return crate::ops::convert(v, self.ty).map_err(|e| e.describe()),
            Tok::Raw(s) | Tok::Quoted(s) => s,
            Tok::Op(o) => return Err(format!("operator `{o}` where a value was expected")),
        };
        Ok(match self.ty {
            T::Text(_) => Value::Text(text.into()),
            T::Code(_) => Value::Code(text.trim().to_uppercase().into()),
            T::Integer => Value::Integer(text.parse().map_err(|_| format!("`{text}` is not an Integer"))?),
            T::BigInteger => Value::BigInteger(text.parse().map_err(|_| format!("`{text}` is not a BigInteger"))?),
            T::Decimal => Value::Decimal(text.parse().map_err(|_| format!("`{text}` is not a Decimal"))?),
            // Format(false) = 'No' is used as a filter value in the corpus. The words BC accepts,
            // in any case, are Yes, True, 1 and No, False, 0
            // (record/TestFilterTextBooleanFindContracts.al:103, SetFilter_BooleanField_AcceptedWords).
            T::Boolean if ["yes", "true", "1"].contains(&text.to_lowercase().as_str()) => Value::Boolean(true),
            T::Boolean if ["no", "false", "0"].contains(&text.to_lowercase().as_str()) => Value::Boolean(false),
            T::Boolean => return Err(format!("{INVALID_BOOLEAN}{text}")),
            // The matcher trims both sides and skips zero-length member names (corpus
            // types/TestEnumBlankOptionFilterTests.Codeunit.al:122,188).
            T::Option(m) => match m
                .iter()
                .position(|x| !x.is_empty() && x.trim().eq_ignore_ascii_case(text.trim()))
            {
                Some(i) => Value::Option(i as i32, m.clone().into()),
                None => Value::Option(
                    text.parse().map_err(|_| format!("`{text}` is not an option member"))?,
                    m.clone().into(),
                ),
            },
            T::Enum(id) => {
                let named = (self.enums)(*id)
                    .into_iter()
                    .find(|(_, n)| !n.is_empty() && n.trim().eq_ignore_ascii_case(text.trim()))
                    .map(|(v, _)| v);
                match named.or_else(|| text.trim().parse().ok()) {
                    Some(v) => Value::Enum(*id, v),
                    None => return Err(format!("`{text}` is not a member of the enum")),
                }
            }
            other => {
                return Err(format!(
                    "text filter value for {other}: how BC reads the text (an empty `''`, a formula such as `1M`, a region's date format) is not pinned (docs/probes-needed.md)",
                    other = other.describe()
                ));
            }
        })
    }

    /// A single value, which may be a wildcard pattern on text fields.
    fn single(&mut self, t: Tok, case_insensitive: bool) -> Result<FilterExpr, String> {
        use crate::value::RtType as T;
        // `@` and wildcards stay active inside a quoted expression; only the filter operators
        // are literal there (record/TestFilterQuotedLiterals.al).
        if let (Tok::Quoted(q), T::Text(_) | T::Code(_)) = (&t, self.ty) {
            let (body, ci) = match q.strip_prefix('@') {
                Some(b) => (b, true),
                None => (q.as_str(), case_insensitive),
            };
            if body.contains(['*', '?']) || ci {
                // A Code filter shows upper case with `@` too (types/TestProbeScalarContracts.al:SetFilter_CodeFieldUpperCases).
                let body = if matches!(self.ty, T::Code(_)) {
                    body.to_uppercase()
                } else {
                    body.to_string()
                };
                return Ok(FilterExpr::Like(body, ci));
            }
        }
        if let (Tok::Raw(s), T::Text(_) | T::Code(_)) = (&t, self.ty) {
            let s = if matches!(self.ty, T::Code(_)) {
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
                        Value::Text(String::new().into())
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
    /// The filter group it was set in (`FilterGroup(n)`); every group applies at once.
    pub group: i32,
}

/// The filter group `FilterGroup(-1)` writes to: its filters on different fields are alternatives
/// (a row passes when it matches any of them), while the filters of every other group all have to
/// match (corpus record/PWFilterGroupOr.al).
pub const OR_GROUP: i32 = -1;

/// Whether `row` passes `filters`: every filter outside the OR group matches, and, when the OR
/// group holds filters, at least one of them does.
pub fn row_passes(row: &[Value], filters: &[FieldFilter]) -> bool {
    let (mut or_seen, mut or_matched) = (false, false);
    for f in filters {
        let matched = f.expr.matches(&row[f.field]);
        if f.group == OR_GROUP {
            or_seen = true;
            or_matched |= matched;
        } else if !matched {
            return false;
        }
    }
    !or_seen || or_matched
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
    fn the_or_group_is_alternatives_and_the_other_groups_all_apply() {
        let ff = |field, v: i32, group| FieldFilter {
            field,
            expr: FilterExpr::Eq(Value::Integer(v)),
            group,
        };
        let row = [Value::Integer(1), Value::Integer(2), Value::Integer(3)];
        // Either of two fields matches: the row passes.
        assert!(row_passes(&row, &[ff(0, 9, OR_GROUP), ff(1, 2, OR_GROUP)]));
        assert!(!row_passes(&row, &[ff(0, 9, OR_GROUP), ff(1, 9, OR_GROUP)]));
        // A filter of group 0 still has to match next to the OR group.
        assert!(!row_passes(&row, &[ff(2, 9, 0), ff(1, 2, OR_GROUP)]));
        assert!(row_passes(&row, &[ff(2, 3, 0), ff(1, 2, OR_GROUP)]));
        assert!(row_passes(&row, &[]));
    }

    use crate::value::RtType;

    fn pf(text: &str, args: &[Value], ty: RtType) -> FilterExpr {
        parse_filter(text, args, &ty).unwrap_or_else(|e| panic!("{text}: {e}"))
    }
    fn ints(f: &FilterExpr, vals: &[i32]) -> Vec<bool> {
        vals.iter().map(|v| f.matches(&Value::Integer(*v))).collect()
    }
    fn texts(f: &FilterExpr, vals: &[&str]) -> Vec<bool> {
        vals.iter()
            .map(|v| f.matches(&Value::Code(v.to_string().into())))
            .collect()
    }

    fn opt(text: &str, members: &[&str]) -> Result<FilterExpr, String> {
        let m: Vec<String> = members.iter().map(|x| x.to_string()).collect();
        parse_filter(text, &[], &RtType::Option(m))
    }

    // types/TestEnumBlankOptionFilterTests.Codeunit.al: option member names match with both
    // sides trimmed and case ignored, zero-length member names are skipped, and only then
    // is the text read as an ordinal.
    #[test]
    fn option_filters_match_member_names_trimmed_and_skip_zero_length_names() {
        let m = ["", "Draft", "Active"];
        let ord = |f: &FilterExpr, o: i32| f.matches(&Value::Option(o, Vec::<String>::new().into()));
        let f = opt(" draft ", &m).unwrap();
        assert!(ord(&f, 1) && !ord(&f, 2));
        // A space-named member (`OptionMembers = " ",Draft`) is not zero length: '' trims to it.
        let f = opt("''", &[" ", "Draft"]).unwrap();
        assert!(ord(&f, 0) && !ord(&f, 1));
        // A zero-length member name is skipped, so '' names no member and is no ordinal.
        assert!(opt("''", &m).is_err());
        // The ordinal fallback.
        let f = opt("2", &m).unwrap();
        assert!(ord(&f, 2) && !ord(&f, 1));
        assert!(opt("Nope", &m).is_err());
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
        assert!(f.matches(&Value::Text(String::new().into())) && !f.matches(&Value::Text("x".into())));
        let f = pf("<>''", &[], RtType::Text(None));
        assert!(!f.matches(&Value::Text(String::new().into())) && f.matches(&Value::Text("x".into())));
        let f = pf("'Smith & Sons'", &[], RtType::Text(None));
        assert!(f.matches(&Value::Text("Smith & Sons".into())));
        let f = pf("'(Travel|Meals)'", &[], RtType::Text(None));
        assert!(f.matches(&Value::Text("(Travel|Meals)".into())));
    }

    fn enum_members(_: u32) -> Vec<(i32, String)> {
        vec![
            (0, " ".into()),
            (1, "Initial Entry".into()),
            (2, "Payment Discount (VAT Excl.)".into()),
            (3, "Application".into()),
        ]
    }

    fn enum_hits(text: &str) -> Vec<i32> {
        let f = parse_filter_with(text, &[], &RtType::Enum(7), &enum_members).unwrap_or_else(|e| panic!("{text}: {e}"));
        (0..4).filter(|v| f.matches(&Value::Enum(7, *v))).collect()
    }

    // corpus record/TestCalcFormulaQuotedFilterHeader.Table.al:21-52: member names with
    // spaces, parentheses and a blank member, once the quotes are single quotes.
    #[test]
    fn enum_filters_take_member_names_and_ordinals() {
        assert_eq!(enum_hits("'Initial Entry'"), [1]);
        assert_eq!(enum_hits("'Payment Discount (VAT Excl.)'"), [2]);
        assert_eq!(enum_hits("Application | 'Initial Entry'"), [1, 3]);
        assert_eq!(enum_hits("<> ' '"), [1, 2, 3]);
        assert_eq!(enum_hits("' '"), [0]);
        assert_eq!(enum_hits("application"), [3]);
        assert_eq!(enum_hits("2"), [2]);
        assert!(parse_filter_with("Nope", &[], &RtType::Enum(7), &enum_members).is_err());
    }

    // corpus types/TestEnumBlankOptionFilterTests.Codeunit.al:122,160,188: the option matcher
    // trims both sides, skips zero-length member names, and falls back to an ordinal.
    #[test]
    fn option_filters_trim_both_sides_and_skip_zero_length_member_names() {
        let opt = |members: &[&str]| RtType::Option(members.iter().map(|m| m.to_string()).collect());
        let hits = |ty: &RtType, text: &str| -> Result<Vec<i32>, String> {
            let f = parse_filter(text, &[], ty)?;
            let names: Vec<String> = match ty {
                RtType::Option(m) => m.clone(),
                _ => unreachable!(),
            };
            Ok((0..names.len() as i32)
                .filter(|i| f.matches(&Value::Option(*i, names.clone().into())))
                .collect())
        };
        // A member named with one space is matched by the empty string.
        let space = opt(&[" ", "Draft", "Active"]);
        assert_eq!(hits(&space, "''").unwrap(), [0]);
        assert_eq!(hits(&space, "' Draft '").unwrap(), [1]);
        assert_eq!(hits(&space, "active").unwrap(), [2]);
        // A member with no name at all is skipped, so the empty string names nothing.
        let empty = opt(&["", "Draft", "Active"]);
        assert!(hits(&empty, "''").is_err());
        assert_eq!(hits(&empty, "2").unwrap(), [2]);
        assert!(hits(&empty, "Purple").is_err());
    }

    #[test]
    fn option_filter_matches_member_names_trimmed_case_blind_and_skips_zero_length_members() {
        // corpus types/TestEnumBlankOptionFilterTests.Codeunit.al:1-12, :187-205
        let members: Vec<String> = vec!["".into(), "Draft".into(), " ".into()];
        let ty = RtType::Option(members);
        let ord = |f: &FilterExpr| {
            (0..3)
                .map(|i| f.matches(&Value::Option(i, Default::default())))
                .collect::<Vec<_>>()
        };
        assert_eq!(ord(&pf(" draft ", &[], ty.clone())), [false, true, false]);
        assert_eq!(ord(&pf("'DRAFT'", &[], ty.clone())), [false, true, false]);
        // The empty string names the blank member " " (trimmed), never the zero-length one.
        assert_eq!(ord(&pf("''", &[], ty.clone())), [false, false, true]);
        let zero_length_only = RtType::Option(vec!["".into(), "Draft".into()]);
        assert!(
            parse_filter("''", &[], &zero_length_only).is_err(),
            "'' names no member here"
        );
    }

    #[test]
    fn at_and_wildcards_stay_active_inside_a_quoted_expression() {
        // record/TestFilterQuotedLiterals.al: `'@*Smith & Sons*'` ("quote so &, (, ), =, | are literal,
        // and keep @ and * active"): a case-insensitive "contains" filter on a value with operators in it.
        let f = pf("'@*Smith & Sons*'", &[], RtType::Text(None));
        assert!(f.matches(&Value::Text("Mr smith & sons LLC".into())));
        assert!(!f.matches(&Value::Text("Acme Co".into())));
        let f = pf("'@*(2025)*'", &[], RtType::Text(None));
        assert!(f.matches(&Value::Text("Expenses (2025) - Diana".into())));
        assert!(!f.matches(&Value::Text("Expenses 2025".into())));
        // Without @ the quoted wildcard still matches case-sensitively.
        let f = pf("'*A|B*'", &[], RtType::Text(None));
        assert!(f.matches(&Value::Text("xA|By".into())) && !f.matches(&Value::Text("xa|by".into())));
        // A quoted expression without wildcards is a plain literal.
        let f = pf("'Smith & Sons'", &[], RtType::Text(None));
        assert!(f.matches(&Value::Text("Smith & Sons".into())) && !f.matches(&Value::Text("smith & sons".into())));
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
