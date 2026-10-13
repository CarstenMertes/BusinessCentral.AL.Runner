//! Filter and view text: `GetFilter`, `GetFilters`, `GetView`, `SetView`.
//!
//! The corpus pins how a filter reads back for the simple shapes
//! (`'10..20'`, `'30'`, `'A'`: testfilter/TestTestFilter.al:227-350) and the view
//! grammar through the strings it feeds to `SetView`
//! (`'SORTING(Entry No.) WHERE(Entry No.=FILTER(1..3))'`:
//! recordref/TestRecordRefFilter.al:23). Anything else is refused.

use crate::Session;
use al_builtins::filter::{FieldFilter, FilterExpr};
use al_builtins::native::Signal;
use al_builtins::value::{RecordState, Value};
use al_data::TableDef;
use std::cell::RefCell;
use std::rc::Rc;

type Rec = Rc<RefCell<RecordState>>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

/// Characters that would need quoting in a filter value.
const FILTER_SPECIAL: &[char] = &['|', '&', '<', '>', '=', '*', '?', '@', '\'', '"', '(', ')'];

impl Session {
    /// A filter expression over a field of type `ty`, enum members by name included.
    pub(crate) fn parse_filter_text(&self, text: &str, ty: &al_builtins::value::RtType) -> Result<FilterExpr, String> {
        let engine = self.engine.clone();
        let enums = |id: u32| engine.program.objects[id as usize].enum_values.clone();
        al_builtins::filter::parse_filter_with(text, &[], ty, &enums)
    }

    fn filter_value(&self, v: &Value) -> Result<String, Signal> {
        if let Value::Text(s) | Value::Code(s) = v {
            if s.contains(FILTER_SPECIAL) || s.contains("..") {
                return unsupported("a filter value that needs quoting");
            }
        }
        self.message_value(v)
    }

    fn filter_text(&self, e: &FilterExpr) -> Result<String, Signal> {
        Ok(match e {
            FilterExpr::Range(Some(a), Some(b)) => {
                if al_builtins::ops::compare(a, b).ok() == Some(std::cmp::Ordering::Equal) {
                    self.filter_value(a)?
                } else {
                    format!("{}..{}", self.filter_value(a)?, self.filter_value(b)?)
                }
            }
            FilterExpr::Range(Some(a), None) => format!("{}..", self.filter_value(a)?),
            FilterExpr::Range(None, Some(b)) => format!("..{}", self.filter_value(b)?),
            FilterExpr::Range(None, None) => return unsupported("an unbounded range filter"),
            FilterExpr::Eq(v) => self.filter_value(v)?,
            FilterExpr::Ne(v) => format!("<>{}", self.filter_value(v)?),
            FilterExpr::Lt(v) => format!("<{}", self.filter_value(v)?),
            FilterExpr::Le(v) => format!("<={}", self.filter_value(v)?),
            FilterExpr::Gt(v) => format!(">{}", self.filter_value(v)?),
            FilterExpr::Ge(v) => format!(">={}", self.filter_value(v)?),
            FilterExpr::Like(p, ci) => format!("{}{p}", if *ci { "@" } else { "" }),
            FilterExpr::Or(xs) => xs
                .iter()
                .map(|x| self.filter_text(x))
                .collect::<Result<Vec<_>, _>>()?
                .join("|"),
            FilterExpr::And(xs) => {
                if xs.iter().any(|x| matches!(x, FilterExpr::Or(_))) {
                    return unsupported("an AND of OR filters");
                }
                xs.iter()
                    .map(|x| self.filter_text(x))
                    .collect::<Result<Vec<_>, _>>()?
                    .join("&")
            }
        })
    }

    /// The filter on field `pos` as `GetFilter` reads it back ('' when there is none).
    pub(crate) fn field_filter_text(&self, rec: &Rec, pos: usize) -> Result<String, Signal> {
        let r = rec.borrow();
        let parts = r
            .filters
            .iter()
            .filter(|f| f.field == pos && f.group == r.filter_group)
            .map(|f| self.filter_text(&f.expr))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(parts.join(""))
    }

    /// The filters of the current filter group, in field order.
    fn filters_in_field_order(rec: &Rec) -> Vec<FieldFilter> {
        let r = rec.borrow();
        let mut v: Vec<FieldFilter> = r
            .filters
            .iter()
            .filter(|f| f.group == r.filter_group)
            .cloned()
            .collect();
        v.sort_by_key(|f| f.field);
        v
    }

    /// Whether the current filter group has a filter (`HasFilter`).
    pub(crate) fn has_filter(rec: &Rec) -> bool {
        !Self::filters_in_field_order(rec).is_empty()
    }

    /// `GetFilters`: `Caption: filter` pairs joined by `, ` (docs/probes-needed.md).
    pub(crate) fn get_filters(&self, rec: &Rec, t: &TableDef) -> Result<String, Signal> {
        let mut parts = Vec::new();
        for f in Self::filters_in_field_order(rec) {
            parts.push(format!(
                "{}: {}",
                self.field_caption(t, f.field),
                self.filter_text(&f.expr)?
            ));
        }
        Ok(parts.join(", "))
    }

    /// `GetView(UseNames)`: `SORTING(keyfields) [ORDER(Descending)] [WHERE(f=FILTER(x),...)]`.
    pub(crate) fn get_view(&self, rec: &Rec, t: &TableDef, use_names: bool) -> Result<String, Signal> {
        let name = |pos: usize| {
            if use_names {
                t.fields[pos].name.clone()
            } else {
                format!("Field{}", t.fields[pos].id)
            }
        };
        let (key, ascending) = {
            let r = rec.borrow();
            if r.field_descending {
                return Err(Signal::Unsupported(
                    "GetView after a per-field SetAscending (not pinned)".into(),
                ));
            }
            (r.key, r.ascending)
        };
        let mut out = format!(
            "SORTING({})",
            t.keys[key].iter().map(|&p| name(p)).collect::<Vec<_>>().join(",")
        );
        if !ascending {
            out.push_str(" ORDER(Descending)");
        }
        let filters = Self::filters_in_field_order(rec);
        if !filters.is_empty() {
            let mut items = Vec::new();
            for f in &filters {
                items.push(format!("{}=FILTER({})", name(f.field), self.filter_text(&f.expr)?));
            }
            out.push_str(&format!(" WHERE({})", items.join(",")));
        }
        Ok(out)
    }

    /// `SetView`: replaces the key, the order and all filters by those of the view.
    pub(crate) fn set_view(&mut self, rec: &Rec, t: &TableDef, view: &str) -> Result<(), Signal> {
        let mut key = None;
        let mut ascending = true;
        let mut filters: Vec<FieldFilter> = Vec::new();
        let group = rec.borrow().filter_group;
        let mut rest = view.trim();
        while !rest.is_empty() {
            let open = rest
                .find('(')
                .ok_or_else(|| Signal::Unsupported(format!("view `{view}`")))?;
            let word = rest[..open].trim().to_ascii_uppercase();
            let (inner, after) =
                balanced(&rest[open..]).ok_or_else(|| Signal::Unsupported(format!("view `{view}`")))?;
            match word.as_str() {
                "VERSION" => {}
                "SORTING" => {
                    let wanted = inner
                        .split(',')
                        .map(|n| field_by_name(t, n.trim()))
                        .collect::<Result<Vec<_>, _>>()?;
                    key = Some(
                        t.keys
                            .iter()
                            .position(|k| k.len() >= wanted.len() && k[..wanted.len()] == wanted[..])
                            .ok_or_else(|| Signal::Unsupported("SetView with SORTING without a matching key".into()))?,
                    );
                }
                "ORDER" => match inner.trim().to_ascii_lowercase().as_str() {
                    "ascending" => ascending = true,
                    "descending" => ascending = false,
                    other => return unsupported(format!("view ORDER({other})")),
                },
                "WHERE" => {
                    for item in split_top(inner, ',') {
                        let (field, cond) = item
                            .split_once('=')
                            .ok_or_else(|| Signal::Unsupported(format!("view condition `{item}`")))?;
                        let pos = field_by_name(t, field.trim())?;
                        let cond = cond.trim();
                        let open = cond
                            .find('(')
                            .ok_or_else(|| Signal::Unsupported(format!("view condition `{item}`")))?;
                        let kind = cond[..open].trim().to_ascii_uppercase();
                        let (value, tail) = balanced(&cond[open..])
                            .ok_or_else(|| Signal::Unsupported(format!("view condition `{item}`")))?;
                        if !tail.trim().is_empty() || !matches!(kind.as_str(), "FILTER" | "CONST") {
                            return unsupported(format!("view condition `{item}`"));
                        }
                        let ty = t.fields[pos].ty.clone();
                        let expr = self
                            .parse_filter_text(value, &ty)
                            .map_err(|e| Signal::Unsupported(format!("SetView: {e}")))?;
                        filters.retain(|f| f.field != pos);
                        filters.push(FieldFilter {
                            field: pos,
                            expr,
                            group,
                        });
                    }
                }
                other => return unsupported(format!("view part {other}")),
            }
            rest = after.trim();
        }
        let mut r = rec.borrow_mut();
        // The view replaces the filters of the current group; other groups stay.
        r.filters.retain(|f| f.group != group);
        r.filters.extend(filters);
        r.key = key.unwrap_or(0);
        r.ascending = ascending;
        Ok(())
    }
}

/// `(a(b)c)rest` → (`a(b)c`, `rest`).
fn balanced(s: &str) -> Option<(&str, &str)> {
    let mut depth = 0usize;
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => {
                depth -= 1;
                if depth == 0 {
                    return Some((&s[1..i], &s[i + 1..]));
                }
            }
            _ => {}
        }
    }
    None
}

fn split_top(s: &str, sep: char) -> Vec<&str> {
    let mut out = Vec::new();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in s.char_indices() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            c if c == sep && depth == 0 => {
                out.push(&s[start..i]);
                start = i + c.len_utf8();
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

/// A view names a field by its name or, in the numeric form, as `Field<n>`.
fn field_by_name(t: &TableDef, name: &str) -> Result<usize, Signal> {
    let name = name.trim_matches('"');
    if let Some(p) = t.fields.iter().position(|f| f.name.eq_ignore_ascii_case(name)) {
        return Ok(p);
    }
    name.strip_prefix("Field")
        .and_then(|n| n.parse::<u32>().ok())
        .and_then(|id| t.fields.iter().position(|f| f.id == id))
        .map_or_else(|| unsupported(format!("view field `{name}`")), Ok)
}
