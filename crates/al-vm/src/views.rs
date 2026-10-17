//! Filter and view text: `GetFilter`, `GetFilters`, `GetView`, `SetView`.
//!
//! The corpus pins how a filter reads back for the simple shapes
//! (`'10..20'`, `'30'`, `'A'`: testfilter/TestTestFilter.al:227-350) and the view
//! grammar through the strings it feeds to `SetView`
//! (`'SORTING(Entry No.) WHERE(Entry No.=FILTER(1..3))'`:
//! recordref/TestRecordRefFilter.al:23). Anything else is refused.

use crate::Session;
use al_builtins::describe::Describe;
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

/// Characters whose presence in a text value makes `GetFilter` quote the whole value (record/
/// TestSetFilterSpecialTextContracts.al, BC 27.5), and `..`.
fn needs_quotes(s: &str) -> bool {
    s.contains(['|', '&', '<', '=', '(', ')']) || s.contains("..")
}

/// Characters in a value `GetFilter` cannot show yet: nothing pins them.
const FILTER_UNPINNED: &[char] = &['>', '*', '?', '"'];

impl Session {
    /// A filter expression over a field of type `ty`, enum members by name included.
    pub(crate) fn parse_filter_text(&self, text: &str, ty: &al_builtins::value::RtType) -> Result<FilterExpr, String> {
        let engine = self.engine.clone();
        let enums = |id: u32| crate::views::enum_filter_members(&engine.program.objects[id as usize]);
        al_builtins::filter::parse_filter_with(text, &[], ty, &enums)
    }

    /// `filter_value`; with `iso` a Date reads `yyyy-mm-dd`, as a view writes it (record/lt-record-read/RcAMisc.al:
    /// GetView_Field_Types `date`).
    fn filter_value_with(&self, v: &Value, iso: bool) -> Result<String, Signal> {
        // A blank date shows as two single quotes, like a blank text (record/lt-record-read/RcAMisc.al:
        // SetRecFilter_Composite_And_Text_Keys `dateBlank`).
        if matches!(v, Value::Date(0, false)) {
            return Ok("''".into());
        }
        // A view writes a Boolean as `true` or `false` (:GetView_Field_Types `bool`).
        if let (true, Value::Boolean(b)) = (iso, v) {
            return Ok(b.to_string());
        }
        if let (true, Value::Date(d, false)) = (iso, v) {
            if *d != 0 {
                let (y, m, day) = al_builtins::format::civil_from_day(*d);
                return Ok(format!("{y:04}-{m:02}-{day:02}"));
            }
        }
        if let Value::Text(s) | Value::Code(s) = v {
            // A blank text shows as two single quotes (corpus record/PWFilterGroupOr.al: `A: ''`).
            if s.is_empty() {
                return Ok("''".into());
            }
            if needs_quotes(s) {
                if s.contains(FILTER_UNPINNED) || s.contains('\'') || s.starts_with('@') {
                    return unsupported("a filter value that needs quoting and holds more filter characters");
                }
                return Ok(format!("'{s}'"));
            }
            if s.contains(FILTER_UNPINNED) || s.starts_with('@') {
                return unsupported("a filter value that needs quoting");
            }
        }
        let text = self.message_value(v)?;
        // A value that is only blank space is shown in single quotes (corpus
        // record/TestRecordRuntimeTextsAndViews.al:Record_GetView_And_GetFilters_Texts, the blank
        // option member: `Status: <>' '`).
        // A value with a leading space too (filters/W4Filters.al:BlankOnlyFilterValues_MatchOnlyBlankRows:
        // `SetRange(Txt, ' a')` shows `Txt: ' a'`).
        if !text.is_empty() && (text.trim().is_empty() || text.starts_with(' ')) {
            return Ok(format!("'{text}'"));
        }
        Ok(text)
    }

    pub(crate) fn filter_text(&self, e: &FilterExpr) -> Result<String, Signal> {
        self.filter_text_with(e, false)
    }

    /// `filter_text`; with `iso` the dates are written as a view writes them.
    pub(crate) fn filter_text_with(&self, e: &FilterExpr, iso: bool) -> Result<String, Signal> {
        Ok(match e {
            FilterExpr::Range(Some(a), Some(b)) => {
                if al_builtins::ops::compare(a, b).ok() == Some(std::cmp::Ordering::Equal) {
                    self.filter_value_with(a, iso)?
                } else {
                    format!(
                        "{}..{}",
                        self.filter_value_with(a, iso)?,
                        self.filter_value_with(b, iso)?
                    )
                }
            }
            FilterExpr::Range(Some(a), None) => format!("{}..", self.filter_value_with(a, iso)?),
            FilterExpr::Range(None, Some(b)) => format!("..{}", self.filter_value_with(b, iso)?),
            FilterExpr::Range(None, None) => return unsupported("an unbounded range filter"),
            FilterExpr::Eq(v) => self.filter_value_with(v, iso)?,
            FilterExpr::Ne(v) => format!("<>{}", self.filter_value_with(v, iso)?),
            FilterExpr::Lt(v) => format!("<{}", self.filter_value_with(v, iso)?),
            FilterExpr::Le(v) => format!("<={}", self.filter_value_with(v, iso)?),
            FilterExpr::Gt(v) => format!(">{}", self.filter_value_with(v, iso)?),
            FilterExpr::Ge(v) => format!(">={}", self.filter_value_with(v, iso)?),
            FilterExpr::Like(p, ci) if needs_quotes(p) && !p.contains(['*', '?', '\'', '>']) => {
                format!("'{}{p}'", if *ci { "@" } else { "" })
            }
            FilterExpr::Like(p, ci) => format!("{}{p}", if *ci { "@" } else { "" }),
            FilterExpr::Or(xs) => xs
                .iter()
                .map(|x| self.filter_text_with(x, iso))
                .collect::<Result<Vec<_>, _>>()?
                .join("|"),
            FilterExpr::And(xs) => {
                if xs.iter().any(|x| matches!(x, FilterExpr::Or(_))) {
                    return unsupported("an AND of OR filters");
                }
                xs.iter()
                    .map(|x| self.filter_text_with(x, iso))
                    .collect::<Result<Vec<_>, _>>()?
                    .join("&")
            }
        })
    }

    /// `e` with the Option values of field `pos` carrying the field's OptionCaption entries: `GetFilter` and `GetFilters`
    /// show the caption (record/TestProbeRecordRefContracts.al:Filters_GetFiltersText, `option_filter` is `Second`).
    fn with_option_captions(&self, t: &TableDef, pos: usize, e: &FilterExpr) -> FilterExpr {
        let Ok(Some((false, members))) = self.members_of_field(t, pos) else {
            return e.clone();
        };
        let captions: std::sync::Arc<[String]> = members.iter().map(|m| m.2.clone()).collect::<Vec<_>>().into();
        let map = |v: &Value| match v {
            Value::Option(o, _) => Value::Option(*o, captions.clone()),
            other => other.clone(),
        };
        match e {
            FilterExpr::Range(a, b) => FilterExpr::Range(a.as_ref().map(map), b.as_ref().map(map)),
            FilterExpr::Eq(v) => FilterExpr::Eq(map(v)),
            FilterExpr::Ne(v) => FilterExpr::Ne(map(v)),
            FilterExpr::Lt(v) => FilterExpr::Lt(map(v)),
            FilterExpr::Le(v) => FilterExpr::Le(map(v)),
            FilterExpr::Gt(v) => FilterExpr::Gt(map(v)),
            FilterExpr::Ge(v) => FilterExpr::Ge(map(v)),
            FilterExpr::Or(xs) => FilterExpr::Or(xs.iter().map(|x| self.with_option_captions(t, pos, x)).collect()),
            FilterExpr::And(xs) => FilterExpr::And(xs.iter().map(|x| self.with_option_captions(t, pos, x)).collect()),
            other => other.clone(),
        }
    }

    /// The filter on field `pos` as `GetFilter` reads it back ('' when there is none).
    pub(crate) fn field_filter_text(&self, rec: &Rec, pos: usize) -> Result<String, Signal> {
        let table = self.table(rec)?;
        let r = rec.borrow();
        let parts = r
            .filters
            .iter()
            .filter(|f| f.field == pos && f.group == r.filter_group)
            .map(|f| self.filter_text(&self.with_option_captions(&table, pos, &f.expr)))
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

    /// `FilterGroup([Group])` of a record or a RecordRef: the group it sets, or the current one without an
    /// argument (corpus record/TestRecordRuntimeTextsAndViews.al:Record_FilterGroup_ReturnsTheGroupJustSet_...).
    pub(crate) fn filter_group(rec: &Rec, args: &[Value]) -> Result<Value, Signal> {
        let mut r = rec.borrow_mut();
        if let Some(v) = args.first() {
            r.filter_group = match v {
                // The OR group: filters on different fields are alternatives (corpus
                // record/PWFilterGroupOr.al). Other negative groups are not pinned.
                Value::Integer(n) if *n < 0 && *n != al_builtins::filter::OR_GROUP => {
                    return unsupported(format!("FilterGroup({n}) (only -1, the OR group, is pinned)"));
                }
                Value::Integer(n) => *n,
                other => return unsupported(format!("FilterGroup({})", other.describe())),
            };
        }
        Ok(Value::Integer(r.filter_group))
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
                self.filter_text(&self.with_option_captions(t, f.field, &f.expr))?
            ));
        }
        Ok(parts.join(", "))
    }

    /// `GetView(UseNames)`: `SORTING(keyfields) [ORDER(Descending)] [WHERE(f=FILTER(x),...)]`.
    /// Record and RecordRef views start with `VERSION(1) ` (corpus
    /// record/TestRecordRuntimeTextsAndViews.al:Record_GetView_And_GetFilters_Texts).
    pub(crate) fn get_view(&self, rec: &Rec, t: &TableDef, use_names: bool) -> Result<String, Signal> {
        Ok(format!(
            "VERSION(1) {}",
            self.get_view_with(rec, t, use_names, false, true)?
        ))
    }

    /// `get_view`; with `option_ordinals` an Option or Enum filter value is written as its
    /// ordinal (a FilterPageBuilder's `GetView(Name, false)`, corpus
    /// filterpage/TestFilterPageBuilder.al:FilterPageBuilder_GetView_CaptionsFlag_RendersAnOptionFilterDifferently).
    pub(crate) fn get_view_with(
        &self,
        rec: &Rec,
        t: &TableDef,
        use_names: bool,
        option_ordinals: bool,
        record_form: bool,
    ) -> Result<String, Signal> {
        let name = |pos: usize| {
            if use_names {
                // The caption stands for the name (record/TestProbeRecordRefContracts.al:GetTable_CopiesBufferFiltersKeyDirection:
                // `Identifier=FILTER(1..5)` for field Id with Caption 'Identifier').
                self.field_caption(t, pos)
            } else {
                format!("Field{}", t.fields[pos].id)
            }
        };
        let (key, ascending) = {
            let r = rec.borrow();
            (r.key, r.ascending && !r.field_descending)
        };
        let mut out = format!(
            "SORTING({})",
            t.keys[key].iter().map(|&p| name(p)).collect::<Vec<_>>().join(",")
        );
        if !ascending {
            out.push_str(if use_names || !record_form {
                " ORDER(Descending)"
            } else {
                " ORDER(1)"
            });
        }
        let filters = Self::filters_in_field_order(rec);
        if !filters.is_empty() {
            let mut items = Vec::new();
            for f in &filters {
                let text = if option_ordinals {
                    self.filter_text(&ordinals(&f.expr))?
                } else {
                    self.filter_text_with(&f.expr, true)?
                };
                // The numeric form writes the kind as a number: FILTER is 1, CONST is 0.
                items.push(if use_names || !record_form {
                    format!("{}=FILTER({text})", name(f.field))
                } else {
                    format!("{}=1({text})", name(f.field))
                });
            }
            out.push_str(&format!(" WHERE({})", items.join(",")));
        }
        Ok(out)
    }

    /// `SetView`: replaces the key, the order and all filters by those of the view.
    pub(crate) fn set_view(&mut self, rec: &Rec, t: &TableDef, view: &str) -> Result<(), Signal> {
        self.set_view_in(rec, t, view, None, false, false)
    }

    /// `set_view`, writing the filters of the view to filter group `group` (default: the current
    /// one). With `keep_unnamed`, a view that names no SORTING keeps the record's key and one that
    /// names no ORDER keeps its direction (a part page's SourceTableView after the part control's
    /// SubPageView, corpus handlers/TestPartOwnSortVsSubPageView.al:1-45). With `order_needs_key`,
    /// an ORDER without a SORTING is ignored (a part's SubPageView, corpus
    /// handlers/TestPartSubPageViewSorting.al:SubPageViewOrderOnly_KeepsPrimaryKeyOrder).
    pub(crate) fn set_view_in(
        &mut self,
        rec: &Rec,
        t: &TableDef,
        view: &str,
        group: Option<i32>,
        keep_unnamed: bool,
        order_needs_key: bool,
    ) -> Result<(), Signal> {
        let mut key = None;
        let mut adhoc: Option<Vec<usize>> = None;
        let mut ascending = None;
        let mut filters: Vec<FieldFilter> = Vec::new();
        let group = group.unwrap_or(rec.borrow().filter_group);
        let mut rest = view.trim();
        while !rest.is_empty() {
            let open = rest.find('(').ok_or_else(|| invalid_view(view))?;
            let word = rest[..open].trim().to_ascii_uppercase();
            let (inner, after) = balanced(&rest[open..]).ok_or_else(|| invalid_view(view))?;
            match word.as_str() {
                "VERSION" => {}
                "SORTING" => {
                    let wanted = inner
                        .split(',')
                        .map(|n| field_by_name(t, n.trim()))
                        .collect::<Result<Vec<_>, _>>()?;
                    // Fields no key starts with order the rows by themselves, as SetCurrentKey does (a view that
                    // GetView wrote for such a key sets it back). Not pinned for SetView; Microsoft's tests set views
                    // of ad hoc keys (assumption set-view-sorting-without-a-key).
                    match t
                        .keys
                        .iter()
                        .position(|k| k.len() >= wanted.len() && k[..wanted.len()] == wanted[..])
                    {
                        Some(k) => key = Some(k),
                        None => adhoc = Some(wanted),
                    }
                }
                "ORDER" => match inner.trim().to_ascii_lowercase().as_str() {
                    "ascending" => ascending = Some(true),
                    "descending" => ascending = Some(false),
                    "1" => ascending = Some(false),
                    "0" => ascending = Some(true),
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
                        if !tail.trim().is_empty() || !matches!(kind.as_str(), "FILTER" | "CONST" | "0" | "1") {
                            return unsupported(format!("view condition `{item}`"));
                        }
                        let ty = t.fields[pos].ty.clone();
                        // `filter('>10')` is an AL text literal: the filter is what it holds, operators included
                        // (misc/lt-probes-needed-7/PnGFixtures.al:666 `SubPageView = where("Line No." = filter('>10'))`, opened by
                        // PnGPartTriggers.al:DisabledPartControlEnabled). A literal with a quote inside is not stripped.
                        let value = value
                            .trim()
                            .strip_prefix('\'')
                            .and_then(|v| v.strip_suffix('\''))
                            // Only a literal that starts with a comparison operator (`'>10'`, `'>=2'`) is read as a filter; a quoted name such as a
                            // member with parentheses keeps its quotes (record/TestCalcFormulaQuotedFilterTests.Codeunit.al:60308).
                            .filter(|v| !v.contains('\'') && !v.contains('"') && v.starts_with(['<', '>']))
                            .unwrap_or(value);
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
        if order_needs_key && key.is_none() {
            ascending = None;
        }
        if key.is_some() || !keep_unnamed {
            r.key = key.unwrap_or(0);
        }
        // A view that names no ORDER keeps the direction
        // (record/TestRecordViewCopyMarkContracts.al:130, SetView_ReplacesTheFilters_KeepsTheDirection_AndRejectsGarbage).
        if let Some(a) = ascending {
            r.ascending = a;
        }
        drop(r);
        if let Some(wanted) = adhoc {
            // SORTING(...) that no key starts with orders by those fields, then by the primary key, as SetCurrentKey does
            // (record/lt-setview-sorting-nonkey/SetViewSortingNonKey.al:SetView_SortingNonKeyField_Descending, SetView_SortingFieldLists).
            self.ret_used = false;
            let mut args: Vec<Value> = wanted.iter().map(|p| Value::Integer(*p as i32)).collect();
            self.record_op(al_builtins::record_op::RecordOp::SetCurrentKey, rec, &mut args)?;
        }
        Ok(())
    }
}

/// BC's error for a view that does not parse: a carriage return and a backslash join the
/// sentence to the view text (record/TestRecordViewCopyMarkContracts.al:130,
/// SetView_ReplacesTheFilters_KeepsTheDirection_AndRejectsGarbage).
fn invalid_view(view: &str) -> Signal {
    Signal::Error(Box::new(al_builtins::native::AlError {
        message: format!("Invalid expression of type: table view.{}{view}", crate::records::BR),
        code: "DB:Ok".into(),
        call_stack: String::new(),
        ..Default::default()
    }))
}

/// `e` with every Option and Enum value replaced by its ordinal.
fn ordinals(e: &FilterExpr) -> FilterExpr {
    let v = |x: &Value| match x {
        Value::Option(o, _) | Value::Enum(_, o) => Value::Integer(*o),
        other => other.clone(),
    };
    match e {
        FilterExpr::Range(a, b) => FilterExpr::Range(a.as_ref().map(v), b.as_ref().map(v)),
        FilterExpr::Eq(x) => FilterExpr::Eq(v(x)),
        FilterExpr::Ne(x) => FilterExpr::Ne(v(x)),
        FilterExpr::Lt(x) => FilterExpr::Lt(v(x)),
        FilterExpr::Le(x) => FilterExpr::Le(v(x)),
        FilterExpr::Gt(x) => FilterExpr::Gt(v(x)),
        FilterExpr::Ge(x) => FilterExpr::Ge(v(x)),
        FilterExpr::Or(xs) => FilterExpr::Or(xs.iter().map(ordinals).collect()),
        FilterExpr::And(xs) => FilterExpr::And(xs.iter().map(ordinals).collect()),
        FilterExpr::Like(p, ci) => FilterExpr::Like(p.clone(), *ci),
    }
}

/// A view written as an AL property (`DataItemTableView`, `SourceTableView`, `SubPageView`) in the
/// text form `SetView` reads: a double-quoted name inside a condition's value is the AL quoting of an
/// identifier, which the filter grammar writes with single quotes. Corpus
/// record/TestCalcFormulaQuotedFilterTests.Codeunit.al:189-215 (the oracle passes a DataItemTableView
/// `filter(<> "Initial Entry")` and `filter("Payment Discount (VAT Excl.)")`) and
/// handlers/TestPageSourceTableView.al:169-190 (a SourceTableView `const("G/L")`). Field names keep their
/// quotes (`field_by_name` takes either).
pub(crate) fn property_view(raw: &str) -> String {
    property_view_with(raw, true)
}

/// [`property_view`] for a part control's `SubPageView`: a quoted filter stays as written, so a view like `filter('>10')` is
/// refused.
pub(crate) fn sub_page_view_text(raw: &str) -> String {
    property_view_with(raw, false)
}

fn property_view_with(raw: &str, unquote_filter: bool) -> String {
    let mut parts = Vec::new();
    let mut rest = raw.trim();
    while !rest.is_empty() {
        let (Some(open), Some((inner, after))) = (rest.find('('), rest.find('(').and_then(|o| balanced(&rest[o..])))
        else {
            // Not a view this rewrite understands: left for `set_view` to reject.
            return raw.to_string();
        };
        let word = &rest[..open];
        if word.trim().eq_ignore_ascii_case("where") {
            let items: Vec<String> = split_top(inner, ',')
                .into_iter()
                .map(|item| match item.split_once('=') {
                    Some((field, cond)) => match (cond.find('('), cond.find('(').and_then(|o| balanced(&cond[o..]))) {
                        (Some(o), Some((value, tail))) => {
                            // The AL string of `filter('>10')` is the filter text `>10`; in a `const` it stays a quoted
                            // literal (handlers/TestPageSourceTableView.al, misc/lt-probes-needed-7/PnGFixtures.al:666, 1036).
                            let value = value.trim();
                            let is_filter = unquote_filter && cond[..o].trim().eq_ignore_ascii_case("filter");
                            let whole = value.len() >= 2
                                && value.starts_with('\'')
                                && value.ends_with('\'')
                                && !value[1..value.len() - 1].replace("''", "").contains('\'');
                            if is_filter && whole {
                                format!(
                                    "{field}={}({}){tail}",
                                    &cond[..o],
                                    value[1..value.len() - 1].replace("''", "'")
                                )
                            } else {
                                format!("{field}={}({}){tail}", &cond[..o], rewrite_al_quotes(value))
                            }
                        }
                        _ => item.to_string(),
                    },
                    None => item.to_string(),
                })
                .collect();
            parts.push(format!("{word}({})", items.join(",")));
        } else {
            parts.push(format!("{word}({inner})"));
        }
        rest = after.trim();
    }
    parts.join(" ")
}

/// AL's `"name"` as the filter literal `'name'`; a single-quoted literal is kept as it is.
fn rewrite_al_quotes(raw: &str) -> String {
    let mut out = String::new();
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '"' => {
                out.push('\'');
                for n in chars.by_ref() {
                    match n {
                        '"' => break,
                        '\'' => out.push_str("''"),
                        n => out.push(n),
                    }
                }
                out.push('\'');
            }
            '\'' => {
                out.push('\'');
                while let Some(n) = chars.next() {
                    out.push(n);
                    if n == '\'' {
                        if chars.peek() == Some(&'\'') {
                            out.push(chars.next().expect("peeked"));
                        } else {
                            break;
                        }
                    }
                }
            }
            c => out.push(c),
        }
    }
    out
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

/// The members of an enum for a text filter: the captions first, then the names (see `filter::parse_filter_with`).
pub(crate) fn enum_filter_members(o: &al_ir::Object) -> Vec<(i32, String)> {
    let mut out: Vec<(i32, String)> = o.enum_captions.iter().filter(|(_, c)| !c.is_empty()).cloned().collect();
    out.extend(o.enum_values.iter().cloned());
    out
}

#[cfg(test)]
mod tests {
    use super::property_view;

    #[test]
    fn a_property_view_reads_al_quoted_names_in_values_as_filter_literals() {
        // corpus record/TestCalcFormulaQuotedFilterNegatedView.Report.al:18 and
        // TestCalcFormulaQuotedFilterParenView.Report.al:17 (the oracle passes
        // record/TestCalcFormulaQuotedFilterTests.Codeunit.al:189-215).
        assert_eq!(
            property_view("sorting(\"Entry No.\") where(\"Entry Type\" = filter(<> \"Initial Entry\"))"),
            "sorting(\"Entry No.\") where(\"Entry Type\" = filter(<> 'Initial Entry'))"
        );
        assert_eq!(
            property_view("where(\"Document Type\" = const(\"Credit Memo\"), Kind = const(Item))"),
            "where(\"Document Type\" = const('Credit Memo'), Kind = const(Item))"
        );
        assert_eq!(
            property_view("sorting(\"No.\") order(descending)"),
            "sorting(\"No.\") order(descending)"
        );
    }
}
