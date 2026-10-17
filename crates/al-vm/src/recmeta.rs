//! Record builtins that read table metadata or guard on session state:
//! `TestField`, `Truncate`, `FieldCaption`, `FieldName`, `FieldNo`,
//! `TableCaption`, `TableName`.

use crate::Session;
use al_builtins::describe::Describe;
use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::record_op::RecordOp;
use al_builtins::value::{RecordState, RtType, Value};
use al_data::{Store, TableDef};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

type Rec = Rc<RefCell<RecordState>>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

/// [`error`] with the text `GetLastErrorText(true)` answers for it.
fn error_redacted(message: String, code: &str, redacted: String) -> Signal {
    Signal::Error(Box::new(AlError {
        message,
        code: code.into(),
        origin: al_builtins::native::ErrorOrigin::Redacted(redacted),
        ..Default::default()
    }))
}

fn error(message: String, code: &str) -> Signal {
    Signal::Error(Box::new(AlError {
        message,
        code: code.into(),
        call_stack: String::new(),
        ..Default::default()
    }))
}

/// The text of an AL string literal property value (`'it''s'` → `it's`), if it is one.
pub(crate) fn string_literal(v: &str) -> Option<String> {
    let v = v.trim();
    let inner = v.strip_prefix('\'')?.strip_suffix('\'')?;
    let mut out = String::new();
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        if c == '\'' {
            // Only a doubled quote may appear inside the literal.
            if chars.next() != Some('\'') {
                return None;
            }
        }
        out.push(c);
    }
    Some(out)
}

/// Whether a `TableRelation` value names `table` (lower case) anywhere: as a quoted
/// identifier or as a bare word. Over-approximates (a field of that name also counts), which
/// only makes callers refuse more.
pub(crate) fn mentions_table(relation: &str, table: &str) -> bool {
    let lower = relation.to_lowercase();
    if lower.contains(&format!("\"{table}\"")) {
        return true;
    }
    lower
        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .any(|w| !w.is_empty() && w == table)
}

/// Whether a value is its type's blank value (what `TestField(Field)` rejects).
pub(crate) fn is_blank(v: &Value) -> Result<bool, Signal> {
    Ok(match v {
        Value::Integer(0) | Value::BigInteger(0) | Value::Byte(0) | Value::Char(0) => true,
        Value::Decimal(d) => d.is_zero(),
        Value::Boolean(b) => !b,
        Value::Text(s) | Value::Code(s) => s.is_empty(),
        Value::DateFormula(s) => s.is_empty(),
        Value::Date(d, _) => *d == 0,
        Value::Time(t) | Value::DateTime(t) | Value::Duration(t) => *t == 0,
        Value::Guid(g) => *g == [0; 16],
        Value::Option(o, _) => *o == 0,
        Value::Enum(_, o) => *o == 0,
        Value::Integer(_) | Value::BigInteger(_) | Value::Byte(_) | Value::Char(_) => false,
        other => {
            return unsupported(format!(
                "TestField of a {} field: when it counts as blank, and the error text, are not pinned (docs/probes-needed.md)",
                other.describe()
            ));
        }
    })
}

fn arg_pos(args: &[Value], i: usize) -> Result<usize, Signal> {
    match args.get(i) {
        Some(Value::Integer(p)) => Ok(*p as usize),
        other => unsupported(format!("field argument {}", other.describe())),
    }
}

impl Session {
    /// `Format(Record)`, `Format(RecordRef)` and `Format(FieldRef)`. A record formats as the
    /// `Format` of its fields joined by a TAB: the normal fields in field-number order, then the
    /// FlowFields in field-number order with the value they hold, system fields left out
    /// (record/TestFormatRecordContracts.al:111-145). A FlowFilter field is left out: nothing
    /// pins it (docs/probes-needed.md).
    pub(crate) fn format_composite_value(&mut self, v: &Value) -> Option<Result<String, Signal>> {
        let (rec, only) = match v {
            Value::Record(r) => (r.clone(), None),
            Value::Library(_, l) => match &*l.borrow() {
                al_builtins::value::LibraryValue::RecordRef(r) => (r.clone(), None),
                al_builtins::value::LibraryValue::FieldRef(r, p)
                | al_builtins::value::LibraryValue::KeyFieldRef(r, p) => (r.clone(), Some(*p)),
                _ => return None,
            },
            _ => return None,
        };
        Some(self.format_record_fields(&rec, only))
    }

    fn format_record_fields(&self, rec: &Rc<RefCell<RecordState>>, only: Option<usize>) -> Result<String, Signal> {
        const SYSTEM_FIELD_BASE: u32 = 2_000_000_000;
        let t = self.table(rec)?;
        let fields = rec.borrow().fields.clone();
        let one = |pos: usize| -> Result<String, Signal> {
            let value = &fields[pos];
            match (&t.fields[pos].ty, value) {
                (al_builtins::value::RtType::Option(_), Value::Option(o, _)) => {
                    let Some((_, members)) = self.members_of_field(&t, pos)? else {
                        return unsupported("Format of an Option field");
                    };
                    Ok(members
                        .iter()
                        .find(|(ord, _, _)| ord == o)
                        .map_or_else(|| o.to_string(), |(_, _, caption)| caption.clone()))
                }
                _ => al_builtins::format::try_format(value, 0, 0, self).map_err(Signal::Unsupported),
            }
        };
        if let Some(pos) = only {
            return one(pos);
        }
        let mut order: Vec<usize> = (0..t.fields.len())
            .filter(|&i| t.fields[i].id < SYSTEM_FIELD_BASE)
            .collect();
        order.sort_by_key(|&i| t.fields[i].id);
        // The stored fields come first, then the FlowFields and the FlowFilter fields together in field number
        // order; a FlowFilter field shows the value the field holds (0 for an Integer) whatever filter was set on it
        // (types/ProbeTypeFormsAndFilters.al:FormatRecord_WithFlowFilter_BeforeAndAfterSetRange and
        // types/W7FormatRecord.al:FormatRecord_FlowFilterField, BC 27.5 and 29.0).
        let (normal, calculated): (Vec<usize>, Vec<usize>) = order.into_iter().partition(|&i| t.fields[i].stored);
        let mut parts = Vec::new();
        for i in normal.into_iter().chain(calculated) {
            parts.push(one(i)?);
        }
        Ok(parts.join("\t"))
    }

    /// `FieldCaption` / `FieldRef.Caption`: refused for a system field whose caption the corpus does not pin.
    pub(crate) fn field_caption_checked(&self, t: &TableDef, pos: usize) -> Result<String, Signal> {
        let has_caption = self.field_property(t, pos, "Caption").is_some();
        if t.fields[pos].id >= 2_000_000_000 && !has_caption {
            return unsupported(format!("the caption of system field {}", t.fields[pos].name));
        }
        // A field with a CaptionClass: `3,<text>` reads as <text> ('3,' alone as the Caption); a numeric area other than 3
        // or an empty area reads as the class text as written (pages/lt-page-report-events/P84Captions.al:
        // Table_NoSubscriber f01..f14, Table_Log_* and Table_Resolve_* with no event call, BC 29). A non-numeric area is
        // the platform's event and not pinned for FieldCaption: refused.
        if let Some(class) = self
            .field_property(t, pos, "CaptionClass")
            .and_then(|v| string_literal(&v))
        {
            return match class.split_once(',') {
                Some(("3", "")) => Ok(self.field_caption(t, pos)),
                Some(("3", rest)) => Ok(rest.to_string()),
                Some((area, _)) if area.is_empty() || area.trim().parse::<u32>().is_ok() => Ok(class),
                _ => unsupported(format!(
                    "FieldCaption of a field whose CaptionClass `{class}` has an area no built-in resolver claims (not pinned)"
                )),
            };
        }
        Ok(self.field_caption(t, pos))
    }

    /// A field's caption: its `Caption` property, else its name.
    pub(crate) fn field_caption(&self, t: &TableDef, pos: usize) -> String {
        self.engine.program.objects[t.object as usize]
            .table
            .as_ref()
            .and_then(|s| s.fields.get(pos))
            .and_then(|f| {
                f.properties
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("Caption"))
                    .and_then(|(_, v)| string_literal(v))
            })
            .unwrap_or_else(|| t.fields[pos].name.clone())
    }

    /// Formats a value for an error message; kinds `Format` cannot render yet are refused.
    pub(crate) fn message_value(&self, v: &Value) -> Result<String, Signal> {
        match v {
            Value::Null
            | Value::Boolean(_)
            | Value::Integer(_)
            | Value::BigInteger(_)
            | Value::Byte(_)
            | Value::Char(_)
            | Value::Decimal(_)
            | Value::Text(_)
            | Value::Code(_)
            | Value::Option(..)
            | Value::Enum(..)
            | Value::Guid(_)
            | Value::Date(..)
            // A DateTime reads as Format gives it, to the minute (record/lt-testfield-typed-values/FieldRefTestFieldTypedTemporalCode.al:
            // P189_FldRef_DateTime_Value, `03/15/24 12:00 PM` for 12:00:00 and 12:00:01).
            | Value::DateTime(_)
            | Value::DateFormula(_) => al_builtins::format::try_format(v, 0, 0, self).map_err(Signal::Unsupported),
            // A RecordId reads as Format gives it (`Table: key`, record/TestProbeRecordRefContracts.al:472-502); that an
            // error message shows it the same way is assumed (docs/probes-needed.md).
            Value::Library(al_builtins::library_kind::LibraryKind::RecordId, _) => {
                al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::RecordIdInMessage);
                al_builtins::format::try_format(v, 0, 0, self).map_err(Signal::Unsupported)
            }
            other => unsupported(format!("a {} value in an error message", other.describe())),
        }
    }

    /// `Code=ABC` style identification of the row's primary key, as in TestField errors
    /// (corpus record/TestRecordTestFieldLookupPageId_Tests.al:54-72 pins the single-field form).
    fn key_text(&self, t: &TableDef, fields: &[Value]) -> Result<String, Signal> {
        let mut parts = Vec::new();
        for &i in &t.keys[0] {
            parts.push(format!(
                "{}={}",
                self.field_caption(t, i),
                self.message_value(&fields[i])?
            ));
        }
        Ok(parts.join(", "))
    }

    pub(crate) fn record_meta_op(
        &mut self,
        method: RecordOp,
        rec: &Rec,
        t: &TableDef,
        args: &mut [Value],
    ) -> Result<Value, Signal> {
        self.connection_check(rec)?;
        match method {
            RecordOp::AddLink
            | RecordOp::DeleteLink
            | RecordOp::DeleteLinks
            | RecordOp::HasLinks
            | RecordOp::CopyLinks => self.record_links(method, rec, t, args),
            RecordOp::TableName => Ok(Value::Text(t.name.clone().into())),
            RecordOp::TableCaption => Ok(Value::Text(self.caption(t).into())),
            RecordOp::FieldName => Ok(Value::Text(t.fields[arg_pos(args, 0)?].name.clone().into())),
            RecordOp::FieldCaption => self
                .field_caption_checked(t, arg_pos(args, 0)?)
                .map(|s| Value::Text(s.into())),
            RecordOp::FieldNo => Ok(Value::Integer(crate::refs::al_field_no(t.fields[arg_pos(args, 0)?].id))),
            // True for an enabled field (corpus record/TestRecordTriggers.al:223-231). What a field
            // with `Enabled = false` answers is not pinned: refused (docs/probes-needed.md).
            RecordOp::FieldActive => {
                let pos = arg_pos(args, 0)?;
                // False for `Enabled = false`, true for the rest, a Pending obsolete field included
                // (record/TestRowVersionFieldNumberFieldActiveContracts.al:110-119).
                Ok(Value::Boolean(self.field_enabled(t, pos)))
            }
            RecordOp::RecordId => Ok(self.record_id(rec, t)),
            RecordOp::IsTemporary => Ok(Value::Boolean(rec.borrow().temporary)),
            // Without an argument, whether the current row is marked; with one, marks or unmarks it.
            // What the call returns then (the previous state?) is not pinned, so a used result is refused.
            RecordOp::Mark => {
                let pk = t.primary_key(&rec.borrow().fields);
                let at = rec.borrow().marks.iter().position(|m| ops::compare_keys(m, &pk));
                match args.first() {
                    None => Ok(Value::Boolean(at.is_some())),
                    // Mark(SetMark) returns the new state
                    // (record/TestRecordViewCopyMarkContracts.al:113, Mark_ReturnsTheNewState).
                    Some(v) => {
                        let want = ops::truthy(v)?;
                        let mut r = rec.borrow_mut();
                        match (want, at) {
                            (true, None) => r.marks.push(pk),
                            (false, Some(i)) => {
                                r.marks.remove(i);
                            }
                            _ => {}
                        }
                        Ok(Value::Boolean(want))
                    }
                }
            }
            RecordOp::MarkedOnly => match args.first() {
                None => Ok(Value::Boolean(rec.borrow().marked_only)),
                Some(v) => {
                    let mut r = rec.borrow_mut();
                    r.marked_only = ops::truthy(v)?;
                    r.position = None;
                    Ok(Value::Null)
                }
            },
            RecordOp::ClearMarks => {
                rec.borrow_mut().marks.clear();
                Ok(Value::Null)
            }
            // The bounds of a range filter on the field, whatever filter group it sits in (corpus record/lt-get-range-filter/
            // GetRangeFilterGroups.al: P168_GetRange_FilterOnlyInGroup2, P168_GetRange_FilterInGroup0ReadFromGroup2). The filters of
            // several groups on the field intersect; an empty intersection and a field with no filter raise the errors that file pins.
            RecordOp::GetRangeMin | RecordOp::GetRangeMax => {
                use al_builtins::filter::FilterExpr as F;
                let pos = arg_pos(args, 0)?;
                let found: Vec<al_builtins::filter::FieldFilter> = rec
                    .borrow()
                    .filters
                    .iter()
                    .filter(|f| f.field == pos)
                    .cloned()
                    .collect();
                let t_cap = self.caption(t);
                let f_cap = self.field_caption(t, pos);
                if found.is_empty() {
                    return Err(Signal::Error(Box::new(AlError {
                        message: format!("Specify a filter for the {f_cap} field in the {t_cap} table."),
                        code: "DB:NoFilter".into(),
                        call_stack: String::new(),
                        ..Default::default()
                    })));
                }
                // The bounds of every filter, as (low, high); a filter that is not a single range is not modelled.
                let mut lo: Option<Value> = None;
                let mut hi: Option<Value> = None;
                for f in &found {
                    let (a, b) = match &f.expr {
                        F::Eq(v) => (Some(v.clone()), Some(v.clone())),
                        F::Range(a, b) => (a.clone(), b.clone()),
                        _ => return unsupported(format!("{method} of a filter that is not a single range")),
                    };
                    if let Some(a) = a {
                        if lo.as_ref().is_none_or(|l| ops::compare(&a, l).is_ok_and(|o| o.is_gt())) {
                            lo = Some(a);
                        }
                    }
                    if let Some(b) = b {
                        if hi.as_ref().is_none_or(|h| ops::compare(&b, h).is_ok_and(|o| o.is_lt())) {
                            hi = Some(b);
                        }
                    }
                }
                // One reversed range gives its bounds as written (record/lt-filters-keys-finds/P68B_RangeMinMax.al: RangeFilters_Integer,
                // `4..2`); only the filters of several groups can be empty together.
                if let (true, Some(l), Some(h)) = (found.len() > 1, &lo, &hi) {
                    if ops::compare(l, h).is_ok_and(|o| o.is_gt()) {
                        return Err(Signal::Error(Box::new(AlError {
                            message: format!(
                                "The filter expression \"<><>\" applied to the field {f_cap} in the {t_cap} table results in empty set."
                            ),
                            code: "DB:NotIntervalFilter".into(),
                            call_stack: String::new(),
                            ..Default::default()
                        })));
                    }
                }
                match (method, lo, hi) {
                    (RecordOp::GetRangeMin, Some(l), _) => Ok(l),
                    (RecordOp::GetRangeMax, _, Some(h)) => Ok(h),
                    _ => unsupported(format!("{method} of a filter that is not a single range")),
                }
            }
            // corpus record/TestRecordExtended.al:15-40: Record.Relation on a field without a TableRelation
            // raises an error whose text is not pinned (FieldRef.Relation answers 0 instead).
            RecordOp::Relation => {
                let pos = arg_pos(args, 0)?;
                let has = self.engine.program.objects[t.object as usize]
                    .table
                    .as_ref()
                    .is_some_and(|s| s.fields[pos].relation);
                if !has {
                    return unsupported("Relation of a field without a TableRelation (error text not pinned)");
                }
                self.field_relation(rec, t, pos).map(Value::Integer)
            }
            // Locking and permissions are not observable in one session: only the answers the corpus pins.
            RecordOp::LockTable => {
                // corpus record/TestRecordLock.al:33-41 expects an error mentioning 'not supported' for Wait = false.
                if matches!(args.first(), Some(Value::Boolean(false))) {
                    return unsupported("LockTable(false) is rejected in the cloud (error text not pinned)");
                }
                Ok(Value::Null)
            }
            RecordOp::Consistent => Ok(Value::Null),
            RecordOp::ReadConsistency => Ok(Value::Boolean(false)),
            RecordOp::RecordLevelLocking => Ok(Value::Boolean(true)),
            RecordOp::ReadPermission | RecordOp::WritePermission => Ok(Value::Boolean(true)),
            RecordOp::CountApprox => self.record_op(RecordOp::Count, rec, &mut []),
            RecordOp::CurrentCompany => Ok(Value::Text(self.current_company_of(rec).into())),
            RecordOp::ReadIsolation | RecordOp::SecurityFiltering => {
                let (enum_name, is_iso) = if method == RecordOp::ReadIsolation {
                    ("IsolationLevel", true)
                } else {
                    ("SecurityFilter", false)
                };
                let members = al_builtins::sysenum::option_members(enum_name).expect("known");
                match args.first() {
                    None => {
                        let r = rec.borrow();
                        let i = if is_iso { r.read_isolation } else { r.security_filtering };
                        Ok(Value::Option(i, members.into()))
                    }
                    Some(Value::Option(i, _)) => {
                        let mut r = rec.borrow_mut();
                        if is_iso {
                            r.read_isolation = *i;
                        } else {
                            r.security_filtering = *i;
                        }
                        Ok(Value::Null)
                    }
                    Some(other) => unsupported(format!("{method}({})", other.describe())),
                }
            }
            RecordOp::GetPosition => {
                let use_names = match args.first() {
                    Some(v) => ops::truthy(v)?,
                    None => true,
                };
                self.get_position(rec, t, use_names).map(|s| Value::Text(s.into()))
            }
            RecordOp::SetPosition => {
                let text = al_builtins::native::text_of(args.first().unwrap_or(&Value::Null)).unwrap_or_default();
                self.set_position(rec, t, &text).map(|()| Value::Null)
            }
            RecordOp::TransferFields => self.transfer_fields(rec, t, args),
            RecordOp::GetBySystemId => self.get_by_system_id(rec, t, args, false),
            RecordOp::ModifyAll => self.modify_all(rec, t, args),
            // Doc (Record.Ascending): with an argument sets the sort direction of the current key,
            // without one reads it (corpus record/TestRecordSort.al:111, TestCopyOntoTemporaryRecord.al).
            RecordOp::Ascending => {
                // After a per-field SetAscending(Field, false) the record reads as descending (record/TestProbeRecordRefContracts.al:
                // GetTable_CopiesBufferFiltersKeyDirection, `rr_ascending` No).
                match args.first() {
                    None => Ok(Value::Boolean({
                        let r = rec.borrow();
                        r.ascending && !r.field_descending
                    })),
                    Some(v) => {
                        let mut r = rec.borrow_mut();
                        r.ascending = ops::truthy(v)?;
                        r.field_descending = false;
                        Ok(Value::Null)
                    }
                }
            }
            // The direction of one key field. A record keeps one direction for its whole key, so
            // only a key of that single field is covered: record/TestRecordSort.al:184-203
            // (SetAscending(Amount, false) makes FindFirst return the largest), :158-181
            // (GetAscending). A field of a longer key, or outside the key, has a direction of its
            // own that is not pinned.
            // The companies are the session company and the rows of the Company table; a record in
            // another company reads and writes that company's rows (record/TestRecordChangeCompanyRows.al).
            RecordOp::ChangeCompany => self.change_company(rec, args),
            RecordOp::SetAscending | RecordOp::GetAscending => {
                let pos = arg_pos(args, 0)?;
                if t.keys[rec.borrow().key] != [pos] {
                    return unsupported(format!(
                        "{method} on a field that is not the only field of the current key (per-field direction not pinned)"
                    ));
                }
                if method == RecordOp::GetAscending {
                    let r = rec.borrow();
                    return Ok(Value::Boolean(r.ascending && !r.field_descending));
                }
                let ascending = match args.get(1) {
                    Some(v) => ops::truthy(v)?,
                    None => return unsupported("SetAscending without a direction"),
                };
                // The direction holds for this record until a page hands its view back and forth
                // (handlers/TestPageViewRoundTripRoutes.al:Action_SetAscending_ReadInTheSameTrigger_IsDescending
                // against ..._ReadByALaterAction); `Ascending` keeps its own, view-borne flag.
                let mut r = rec.borrow_mut();
                if !r.ascending {
                    return unsupported("SetAscending on a record that also has Ascending(false) (not pinned)");
                }
                r.field_descending = !ascending;
                Ok(Value::Null)
            }
            // The key's field names (corpus record/TestCopyOntoTemporaryRecord.al:93 `'No.'`,
            // TestBCMetadataContracts.al:196-208 contains the names).
            RecordOp::CurrentKey => {
                let key = rec.borrow().key;
                Ok(Value::Text(
                    t.keys[key]
                        .iter()
                        // The hidden key on SystemId (recordref/TestRecordRefKeyIndexSetRecFilterContracts.al:101); a field with
                        // a Caption shows it (records/lt-record-meta/RcBMeta*.al:CurrentKey_Default_And_Set `Identifier`).
                        .map(|&p| {
                            if t.fields[p].id == 2_000_000_000 {
                                "System ID".to_string()
                            } else {
                                self.field_caption(t, p)
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                        .into(),
                ))
            }
            RecordOp::HasFilter => Ok(Value::Boolean(Self::has_filter(rec))),
            RecordOp::GetFilters => self.get_filters(rec, t).map(|s| Value::Text(s.into())),
            RecordOp::GetFilter => self
                .field_filter_text(rec, arg_pos(args, 0)?)
                .map(|s| Value::Text(s.into())),
            RecordOp::GetView => {
                let use_names = match args.first() {
                    Some(v) => ops::truthy(v)?,
                    None => true,
                };
                self.get_view(rec, t, use_names).map(|s| Value::Text(s.into()))
            }
            RecordOp::SetView => {
                let view = al_builtins::native::text_of(args.first().unwrap_or(&Value::Null)).unwrap_or_default();
                self.set_view(rec, t, &view).map(|()| Value::Null)
            }
            // Returns the group it sets, or the current one without an argument (corpus
            // record/TestRecordRuntimeTextsAndViews.al:Record_FilterGroup_ReturnsTheGroupJustSet_...).
            RecordOp::FilterGroup => Self::filter_group(rec, args),
            // Doc (Record.SetRecFilter): filters the primary key fields to the current values; the
            // filters on other fields stay (record/TestRecordViewCopyMarkContracts.al:41,
            // SetRecFilter_KeepsFiltersOnOtherFields_AndFiltersOnThePrimaryKey).
            // With another current key it also filters that key's fields, after the primary key's
            // (records/W4SortKeys.al:RecordRef_SetRecFilter_AndRecordSetRecFilter: current key Txt gives
            // `Id: 3, Txt: b`, on a Record and on a RecordRef, BC 27.5 and 29.0).
            RecordOp::SetRecFilter => {
                let group = rec.borrow().filter_group;
                let mut pk: Vec<usize> = t.keys[0].clone();
                let current = rec.borrow().key;
                if let Some(k) = t.keys.get(current) {
                    for &p in k {
                        if !pk.contains(&p) {
                            pk.push(p);
                        }
                    }
                }
                let pk = &pk;
                let mut r = rec.borrow_mut();
                r.filters.retain(|f| !(pk.contains(&f.field) && f.group == group));
                for &p in pk {
                    let v = r.fields[p].clone();
                    r.filters.push(al_builtins::filter::FieldFilter {
                        field: p,
                        expr: al_builtins::filter::FilterExpr::Range(Some(v.clone()), Some(v)),
                        group,
                    });
                }
                Ok(Value::Null)
            }
            // `CopyFilter(FromField, Target.ToField)` (the lowerer passes the target record and the
            // position): the filter of the source field arrives on the target field and replaces
            // what that field had; a source field without a filter clears the target's
            // (record/TestRecordFilter.al:226-239, TestFilterAdvancedContracts.al:216-223). Both
            // fields have to be of one type, and only the group 0 filters are copied: how a filter
            // group or a conversion between types behaves is not pinned.
            RecordOp::CopyFilter => {
                let [Value::Integer(from), Value::Record(dst), Value::Integer(to)] = args else {
                    return unsupported(format!("CopyFilter with {}", args.describe()));
                };
                let (from, to) = (*from as usize, *to as usize);
                let dt = self.table(dst)?;
                // A Code field's filter cannot go to a Text field: the constant must be compatible with the
                // target field (types/ProbeTypeFormsAndFilters.al:361-395, BC 27.5). The other combinations of
                // different types are not pinned.
                if t.fields[from].ty != dt.fields[to].ty {
                    if matches!(
                        (&t.fields[from].ty, &dt.fields[to].ty),
                        (RtType::Code(_), RtType::Text(_))
                    ) {
                        return Err(error(
                            format!(
                                "The type of the constant must be compatible with that of the {to_name} field, and the length of the constant must not exceed the length of the field.\\\\Type: Text <-- Code\\Field: {to_name} <-- {from_name}\\Table: {table}",
                                to_name = dt.fields[to].name,
                                from_name = t.fields[from].name,
                                table = dt.name,
                            ),
                            "NCLCSRTS:TableEqualValue",
                        ));
                    }
                    return unsupported("CopyFilter between fields of different types (not pinned)");
                }
                // The filters of the source's current group go to the target's current group; a field the source
                // does not filter in that group clears the target's (same file, :386-393).
                let (to_group, copied): (i32, Vec<_>) = {
                    let r = rec.borrow();
                    let g = r.filter_group;
                    (
                        dst.borrow().filter_group,
                        r.filters
                            .iter()
                            .filter(|f| f.field == from && f.group == g)
                            .cloned()
                            .collect(),
                    )
                };
                let mut d = dst.borrow_mut();
                d.filters.retain(|f| !(f.field == to && f.group == to_group));
                for mut f in copied {
                    f.field = to;
                    f.group = to_group;
                    d.filters.push(f);
                }
                Ok(Value::Null)
            }
            RecordOp::CopyFilters => {
                let Some(Value::Record(src)) = args.first() else {
                    return unsupported(format!("CopyFilters from {}", args.first().describe()));
                };
                if !Rc::ptr_eq(src, rec) {
                    let filters = src.borrow().filters.clone();
                    rec.borrow_mut().filters = filters;
                }
                Ok(Value::Null)
            }
            // Doc (Record.Copy) and corpus record/TestCopyOntoTemporaryRecord.al:66-110: the buffer, filters,
            // key and direction come over; a temporary target keeps its own rows and stays temporary.
            RecordOp::Copy => {
                let Some(Value::Record(src)) = args.first() else {
                    return unsupported(format!("Copy from {}", args.first().describe()));
                };
                let share = match args.get(1) {
                    Some(v) => ops::truthy(v)?,
                    None => false,
                };
                if Rc::ptr_eq(src, rec) {
                    return Ok(Value::Null);
                }
                let s = src.borrow().clone();
                // Copy takes the fields, filters and key but does not move the target to the source's company
                // (record/TestChangeCompanyFiltersCopyDeleteContracts.al:117-121).
                let mut d = rec.borrow_mut();
                // shareTable needs both records temporary; a plain Copy of a temporary record onto
                // a normal one copies values and filters only
                // (record/TestRecordViewCopyMarkContracts.al:89, Copy_ShareTable_RequiresBothRecordsToBeTemporary).
                if share && !(s.temporary && d.temporary) {
                    return Err(error(
                        "The COPY function can only be used with the shareTable argument set to true if both records are temporary.".into(),
                        "Argument",
                    ));
                }
                d.fields = s.fields;
                d.filters = s.filters;
                d.filter_group = s.filter_group;
                d.key = s.key;
                d.adhoc = s.adhoc;
                d.ascending = s.ascending;
                d.field_descending = s.field_descending;
                d.position = s.position;
                // The marks come over with Copy, not with an assignment (record/lt-record-read/RcAMisc.al:
                // Mark_Separate_Variables_Do_Not_Share `copied`).
                d.marks = s.marks;
                d.marked_only = s.marked_only;
                // SetLoadFields comes over with Copy (record/lt-record-read/RcAMisc.al:SetLoadFields_Copy_And_Assignment).
                d.load.request = s.load.request;
                d.load.buffer = s.load.buffer;
                d.load.partial = s.load.partial;
                if share && s.temporary {
                    d.temp_id = s.temp_id.clone();
                }
                Ok(Value::Null)
            }
            RecordOp::TestField => self.test_field(rec, t, args),
            RecordOp::FieldError => self.field_error(rec, t, args),
            other => unsupported(format!("Record.{other}")),
        }
    }

    /// `TestField(Field [, Value])`. The "must have a value" text is pinned by the corpus
    /// (record/TestRecordTestFieldLookupPageId_Tests.al:54-72); the mismatch text is Assert.al's
    /// documented "new format" (docs/probes-needed.md).
    /// A value as TestField shows it: an Option or Enum member by its caption (a member without one by
    /// its name), anything else as in any error message.
    fn member_caption(&self, t: &TableDef, pos: usize, v: &Value) -> Result<String, Signal> {
        let ordinal = match v {
            Value::Option(o, _) | Value::Enum(_, o) => *o,
            other => return self.message_value(other),
        };
        match self.members_of_field(t, pos)? {
            Some((_, members)) => Ok(members
                .iter()
                .find(|(o, _, _)| *o == ordinal)
                .map_or_else(|| ordinal.to_string(), |(_, _, caption)| caption.clone())),
            None => self.message_value(v),
        }
    }

    pub(crate) fn test_field(&mut self, rec: &Rec, t: &TableDef, args: &[Value]) -> Result<Value, Signal> {
        // `TestField(Field [, Value], ErrorInfo)`: a check that holds raises nothing and leaves the ErrorInfo as it
        // was. A failing one raises the same text and code as the form without an ErrorInfo, whatever Message the
        // ErrorInfo carries; in a collecting scope a collectible ErrorInfo is collected instead (a copy with its own
        // Message, the tested FieldNo and the TableId), and a call that is not collectible keeps FieldNo and TableId
        // 0 on the caller's variable (types/PUTestFieldErrorInfo.al, BC 27.5 and 29.0).
        if let Some((info @ Value::Library(kind, _), plain)) = args.split_last()
            && kind.as_str() == "ErrorInfo"
        {
            return match self.test_field(rec, t, plain) {
                Err(Signal::Error(e)) => {
                    if let Some(copy) = al_builtins::errorinfo::copy_of(info) {
                        let pos = arg_pos(plain, 0)?;
                        let collectible = al_builtins::errorinfo::set_origin_of(
                            &copy,
                            self.engine.program.objects[t.object as usize].id.unwrap_or(0) as i32,
                            t.fields[pos].id as i32,
                        );
                        if collectible && self.collect_error(&copy) {
                            return Ok(Value::Null);
                        }
                    }
                    Err(Signal::Error(e))
                }
                other => other,
            };
        }
        if args.len() > 2 {
            return unsupported("TestField with an ErrorInfo");
        }
        let pos = arg_pos(args, 0)?;
        let (current, row) = {
            let r = rec.borrow();
            (r.fields[pos].clone(), r.fields.clone())
        };
        let caption = self.field_caption(t, pos);
        let table = self.caption(t);
        match args.get(1) {
            None => {
                if is_blank(&current)? {
                    // "<field> must not be <caption of the zero member> in <table>: <key>", code TestField
                    // (record/TestTestFieldOptionEnumContracts.al:TestField_ZeroMember_RaisesMustNotBeTheCaption).
                    if matches!(current, Value::Option(..) | Value::Enum(..)) {
                        let key = self.key_text(t, &row)?;
                        let member = self.member_caption(t, pos, &current)?;
                        return Err(error(
                            format!("{caption} must not be {member} in {table}: {key}"),
                            "TestField",
                        ));
                    }
                    let key = self.key_text(t, &row)?;
                    // The flag hides the key: P86LastError.al:233, :236, P70_02LastErrFlag.al:123, :141.
                    return Err(error_redacted(
                        format!("{caption} must have a value in {table}: {key}. It cannot be zero or empty."),
                        "TestField",
                        format!("{caption} must have a value in {table}: <redacted>. It cannot be zero or empty."),
                    ));
                }
            }
            Some(want) => {
                // Text against a text field and a number against a number field are compared as given, without converting to the
                // field's type: a Code field is not upper-cased or trimmed, a Text longer than the field raises no overflow, a Decimal
                // against an Integer field is not an overflow (record/lt-testfield-fielderror-typed/P96Text.al, P96LongText.al,
                // P96Numbers.al). The message shows the value as given.
                // A one-character text that came through a Variant is a Char: it is compared as the text it is (P96Text.al
                // `ref_empty_cd_blank`).
                let want_owned;
                let want = match want {
                    Value::Char(c) if matches!(current, Value::Text(_) | Value::Code(_)) => {
                        want_owned = Value::Text(char::from_u32(u32::from(*c)).unwrap_or(' ').to_string().into());
                        &want_owned
                    }
                    other => other,
                };
                let text_pair = matches!(
                    (&current, want),
                    (Value::Text(_) | Value::Code(_), Value::Text(_) | Value::Code(_))
                );
                let number_pair = matches!(
                    (&current, want),
                    (
                        Value::Integer(_) | Value::BigInteger(_) | Value::Decimal(_),
                        Value::Integer(_) | Value::BigInteger(_) | Value::Decimal(_)
                    )
                );
                // The expected value as the message writes it, for the typed values the corpus pins (record/lt-testfield-typed-values/):
                // an Enum on an Integer field shows its member (`One`, RecordTestFieldWrongTypeEnumOnInteger.al); a Byte is written as an
                // escaped character from BC 28 (`\u0008`, FieldRefTestFieldTypedNumeric.al:P189_FldRef_Byte_Value; a number before);
                // a Guid on a Text field is never equal and is written with braces from BC 28 (RecordTestFieldWrongTypeGuidOnText.al).
                // A Byte above 9 and a Guid against another field type are not pinned.
                let from28 = self.has(al_version::Gate::TestFieldTypedValueTexts);
                let mut shown: Option<String> = None;
                let mut never_equal = false;
                match (&current, want) {
                    (Value::Integer(_), Value::Enum(id, o)) => {
                        shown = Some(al_builtins::format::FormatCtx::enum_name(self, *id, *o))
                    }
                    (Value::Text(_) | Value::Code(_), Value::Guid(g)) => {
                        let braced = al_builtins::format::format_guid(g);
                        shown = Some(if from28 {
                            braced
                        } else {
                            braced.trim_matches(['{', '}']).to_string()
                        });
                        never_equal = true;
                    }
                    (_, Value::Byte(b)) if from28 && matches!(current, Value::Integer(_)) => {
                        if *b > 9 {
                            return unsupported("TestField against a Byte above 9 (the escaped form is not pinned)");
                        }
                        shown = Some(format!("\\u{:04X}", b));
                    }
                    (Value::Guid(_), Value::Guid(_)) => {}
                    (_, Value::Guid(_)) => {
                        return unsupported("TestField against a Guid on a field that is not Text (not pinned)");
                    }
                    _ => {}
                }
                let want = if text_pair || number_pair || never_equal {
                    want.clone()
                } else {
                    ops::convert(want.clone(), &t.fields[pos].ty)?
                };
                let equal = if never_equal {
                    false
                } else if text_pair {
                    // Exact, case and trailing blanks included; a Code field reads the value as Code does upper-cased
                    // (P96Text.al: `cd_lower` is equal, `cd_trailing_blank` and `txt_case` are not).
                    let (a, b) = (
                        al_builtins::native::text_of(&current),
                        al_builtins::native::text_of(&want),
                    );
                    let code_field = matches!(t.fields[pos].ty, RtType::Code(_));
                    matches!((a, b), (Some(a), Some(b)) if if code_field { a == b.to_uppercase() } else { a == b })
                } else {
                    ops::compare(&current, &want)? == Ordering::Equal
                };
                if !equal {
                    let key = self.key_text(t, &row)?;
                    // record/TestRecordRuntimeTextsAndViews.al:Record_TestField_MismatchTexts: the
                    // wanted and the current value are quoted, and two spaces precede "in".
                    // The flag hides the key and both values: P86LastError.al:229, :233, P70_02LastErrFlag.al:129, :135.
                    return Err(error_redacted(
                        format!(
                            "{caption} must be equal to '{}'  in {table}: {key}. Current value is '{}'.",
                            match shown {
                                Some(text) => text,
                                None => self.member_caption(t, pos, &want)?,
                            },
                            self.member_caption(t, pos, &current)?
                        ),
                        "TestField",
                        format!(
                            "{caption} must be equal to '<redacted>'  in {table}: <redacted>. Current value is '<redacted>'."
                        ),
                    ));
                }
            }
        }
        Ok(Value::Null)
    }

    /// `GetPosition`: the primary key fields, whatever the current key is, as `Caption=CONST(value)`
    /// (`Field<n>=0(value)` with `UseNames = false`); corpus record/TestRecordRuntimeTextsAndViews.al:
    /// RecordId_Format_And_GetPosition, recordref/lt-recordref-fieldref-members/P92Position.al:GetPosition_NamesAndDefault
    /// (the field's caption, else its name). A value with a parenthesis is written in double quotes; a comma and an
    /// apostrophe stay as they are (:GetPosition_CompositeKey, :GetPosition_SpecialCharactersInKey).
    fn get_position(&self, rec: &Rec, t: &TableDef, use_names: bool) -> Result<String, Signal> {
        let r = rec.borrow();
        let mut parts = Vec::new();
        for &p in &t.keys[0] {
            let mut value = self.message_value(&r.fields[p])?;
            // A double quote is written as it is when the value needs no quotes around it (record/lt-record-read/RcAMisc.al:
            // GetPosition_Text_With_Special_Characters `a"b`); inside quotes it is not pinned.
            if value.contains(['(', ')']) {
                if value.contains('"') {
                    return unsupported("a position value with a double quote and a parenthesis");
                }
                value = format!("\"{value}\"");
            }
            if use_names {
                parts.push(format!("{}=CONST({value})", self.field_caption(t, p)));
            } else {
                parts.push(format!("Field{}=0({value})", t.fields[p].id));
            }
        }
        Ok(parts.join(","))
    }

    /// `SetPosition`: puts the key values of a position into the buffer (the row is not read). The text is a list of
    /// `Name=CONST(value)` or `Field<n>=0(value)` items; a value with a parenthesis is in double quotes. A text that
    /// is no such list is an "Invalid expression of type: position" error; a list that is not exactly the primary key
    /// fields in order is "not a valid position specification" (both code DB:Ok); then each value is evaluated into its
    /// field (recordref/lt-recordref-fieldref-members/P92Position.al:SetPosition_Malformed, :SetPosition_FromOtherTable).
    fn set_position(&self, rec: &Rec, t: &TableDef, text: &str) -> Result<(), Signal> {
        let db_ok = |message: String| {
            Signal::Error(Box::new(al_builtins::native::AlError {
                message,
                code: "DB:Ok".into(),
                call_stack: String::new(),
                ..Default::default()
            }))
        };
        let invalid = || db_ok(format!("Invalid expression of type: position.\\{text}"));
        let mut items: Vec<(String, String)> = Vec::new();
        let mut rest = text;
        while !rest.is_empty() {
            let (name, after) = rest.split_once('=').ok_or_else(invalid)?;
            let after = after
                .strip_prefix("CONST(")
                .or_else(|| after.strip_prefix("0("))
                .ok_or_else(invalid)?;
            let (value, after) = if let Some(quoted) = after.strip_prefix('"') {
                let (v, tail) = quoted.split_once('"').ok_or_else(invalid)?;
                (v, tail.strip_prefix(')').ok_or_else(invalid)?)
            } else {
                after.split_once(')').ok_or_else(invalid)?
            };
            items.push((name.to_string(), value.to_string()));
            rest = match after.strip_prefix(',') {
                Some(more) if !more.is_empty() => more,
                Some(_) => return Err(invalid()),
                None if after.is_empty() => "",
                None => return Err(invalid()),
            };
        }
        let key = &t.keys[0];
        let mut matched = items.len() == key.len();
        if matched {
            for ((name, _), &p) in items.iter().zip(key) {
                let by_id = name
                    .strip_prefix("Field")
                    .and_then(|n| n.parse::<u32>().ok())
                    .is_some_and(|id| id == t.fields[p].id);
                let caption = self.field_caption(t, p);
                if !by_id && !name.eq_ignore_ascii_case(&caption) {
                    if name.eq_ignore_ascii_case(&t.fields[p].name) {
                        return unsupported(format!("position with the field name `{name}`, not its caption"));
                    }
                    matched = false;
                }
            }
        }
        if !matched {
            return Err(db_ok(format!(
                "'{text}' is not a valid position specification for the {} table. The position must contain all the primary key fields, in order, and only primary key fields.",
                self.caption(t)
            )));
        }
        let mut values = Vec::new();
        for ((_, value), &pos) in items.iter().zip(key) {
            let ty = &t.fields[pos].ty;
            let v = match ty {
                RtType::Text(_) | RtType::Code(_) => ops::convert(Value::Text(value.as_str().into()), ty)?,
                _ if value.is_empty() => Value::default_for(ty),
                RtType::Integer | RtType::BigInteger | RtType::Decimal => match self.parse_filter_text(value, ty) {
                    Ok(al_builtins::filter::FilterExpr::Eq(v)) => v,
                    _ => {
                        let name = match ty {
                            RtType::Integer => "Integer",
                            RtType::BigInteger => "BigInteger",
                            _ => "Decimal",
                        };
                        return Err(error(
                            format!("The value \"{value}\" can't be evaluated into type {name}."),
                            "Evaluate",
                        ));
                    }
                },
                _ => match self.parse_filter_text(value, ty) {
                    Ok(al_builtins::filter::FilterExpr::Eq(v)) => v,
                    _ => return unsupported(format!("position value `{value}`")),
                },
            };
            values.push((pos, v));
        }
        let mut r = rec.borrow_mut();
        for (pos, v) in values {
            r.fields[pos] = v;
        }
        Ok(())
    }

    /// `TransferFields(From [, InitPrimaryKeyFields [, SkipFieldsNotMatchingType]])`: normal fields
    /// are matched by field number; a primary key field is only taken when asked for; a field of
    /// another type is skipped or refused (corpus record/TestRecordTransferFields.al:14-95).
    fn transfer_fields(&mut self, rec: &Rec, t: &TableDef, args: &[Value]) -> Result<Value, Signal> {
        let Some(Value::Record(src)) = args.first() else {
            return unsupported(format!("TransferFields from {}", args.first().describe()));
        };
        let init_pk = match args.get(1) {
            Some(v) => ops::truthy(v)?,
            None => true,
        };
        let skip_mismatch = match args.get(2) {
            Some(v) => ops::truthy(v)?,
            None => false,
        };
        if Rc::ptr_eq(src, rec) {
            return Ok(Value::Null);
        }
        let (src_table, src_fields) = {
            let s = src.borrow();
            (s.table as usize, s.fields.clone())
        };
        let from = self.engine.tables[src_table]
            .clone()
            .ok_or_else(|| Signal::Unsupported("TransferFields from a table without a schema".into()))?;
        let same_kind = |a: &al_builtins::value::RtType, b: &al_builtins::value::RtType| {
            std::mem::discriminant(a) == std::mem::discriminant(b)
        };
        let mut updates = Vec::new();
        for (dp, df) in t.fields.iter().enumerate() {
            if df.id >= 2_000_000_000 || !df.stored {
                continue;
            }
            if !init_pk && t.keys[0].contains(&dp) {
                continue;
            }
            let Some(sp) = from.fields.iter().position(|f| f.id == df.id && f.stored) else {
                continue;
            };
            // An Enum field and an Option field with the same field number are the same kind of field to
            // TransferFields: Base Application's "Sales Order Entity Buffer" holds `Shipping Advice` as an
            // Option and `Sales Header` as an Enum, and its TransferFields from the header works in BC
            // (corpus record/lt-record-conversion-assumptions/TransferEnumOption.al). The ordinal moves.
            let enum_option = |a: &al_builtins::value::RtType, b: &al_builtins::value::RtType| {
                use al_builtins::value::RtType as T;
                matches!((a, b), (T::Enum(_), T::Option(_)) | (T::Option(_), T::Enum(_)))
            };
            if enum_option(&from.fields[sp].ty, &df.ty) {
                let ordinal = match &src_fields[sp] {
                    Value::Enum(_, o) | Value::Option(o, _) => *o,
                    other => {
                        return unsupported(format!(
                            "TransferFields of {} between an Enum and an Option",
                            other.describe()
                        ));
                    }
                };
                let moved = match &df.ty {
                    al_builtins::value::RtType::Option(m) => Value::Option(ordinal, std::sync::Arc::from(m.as_slice())),
                    // An ordinal the Enum lacks moves too, with no error (corpus record/lt-record-conversion-assumptions/
                    // TransferEnumOption.al: P98_OptionToEnum_MissingOrdinal, P98_EnumToOption_MissingOrdinal).
                    al_builtins::value::RtType::Enum(id) => Value::Enum(*id, ordinal),
                    _ => unreachable!("enum_option"),
                };
                updates.push((dp, moved));
                continue;
            }
            if !same_kind(&from.fields[sp].ty, &df.ty) {
                if skip_mismatch {
                    continue;
                }
                return unsupported(format!(
                    "TransferFields across fields of different types (error text not pinned): field {} of {} and field {} of {}",
                    from.fields[sp].name, from.name, df.name, t.name
                ));
            }
            updates.push((dp, ops::convert(src_fields[sp].clone(), &df.ty)?));
        }
        let mut r = rec.borrow_mut();
        for (dp, v) in updates {
            r.fields[dp] = v;
        }
        Ok(Value::Null)
    }

    /// `GetBySystemId(Guid)`: loads the row holding that SystemId, whatever the filters; a miss is
    /// `false` and leaves the buffer. A miss with the result unused raises; its text is pinned only
    /// for the RecordRef form (`raise_on_miss`), the Record form is refused there.
    /// RecordRef: corpus branch al-runtime-rs-staging (ae3fe88), tests/al-language/types/
    /// PXEnumRecordRef.al:199-301.
    pub(crate) fn get_by_system_id(
        &mut self,
        rec: &Rec,
        t: &TableDef,
        args: &[Value],
        raise_on_miss: bool,
    ) -> Result<Value, Signal> {
        let Some(i) = t.fields.iter().position(|f| f.id == 2_000_000_000) else {
            return unsupported("GetBySystemId on a table without SystemId");
        };
        // A Text argument binds to the Guid parameter as a Text assigns to a Guid
        // (PXEnumRecordRef.al:232 passes the null Guid as a text literal).
        let parsed = match args.first() {
            Some(Value::Guid(g)) => Some(*g),
            Some(Value::Text(s) | Value::Code(s)) => ops::parse_guid(s),
            _ => None,
        };
        let Some(g) = &parsed else {
            return unsupported(format!("GetBySystemId of {}", args.first().describe()));
        };
        let found = if *g == [0; 16] {
            None
        } else {
            let filters = [al_builtins::filter::FieldFilter {
                field: i,
                expr: al_builtins::filter::FilterExpr::Eq(Value::Guid(*g)),
                group: 0,
            }];
            let cx = self.cx.clone();
            self.store_for(rec)
                .next(
                    &cx,
                    t,
                    &al_data::Scan {
                        key: 0,
                        filters: &filters,
                        ascending: true,
                        after: None,
                    },
                )
                .map_err(|e| Signal::Unsupported(format!("{e}")))?
        };
        match found {
            Some(row) => {
                // Through `load`: SetAutoCalcFields fields are calculated when a row is read.
                self.load(rec, row)?;
                Ok(Value::Boolean(true))
            }
            // As for Get (records.rs `fail`): a miss on a table whose seeded rows could not be
            // created is not BC's answer (ADR-013 decision 5).
            None if self.engine.unseeded.contains(&(t.object as usize)) && !rec.borrow().temporary => {
                unsupported(format!(
                    "{} row not found by GetBySystemId: the rows a dependency app's install and Company-Initialize create could not be created (ADR-013 decision 5)",
                    t.name
                ))
            }
            None if self.ret_used => Ok(Value::Boolean(false)),
            // "The PX Row does not exist. Identification fields and values: {x}" / DB:RecordNotFound
            // (PXEnumRecordRef.al:264-267; the probe masks what stands between the braces, here
            // the Guid as Format writes it: docs/probes-needed.md).
            None if raise_on_miss => {
                let id = al_builtins::format::try_format(&Value::Guid(*g), 0, 0, self).map_err(Signal::Unsupported)?;
                Err(Signal::Error(Box::new(AlError {
                    message: format!(
                        "The {} does not exist. Identification fields and values: {id}",
                        self.caption(t)
                    ),
                    code: "DB:RecordNotFound".into(),
                    origin: al_builtins::native::ErrorOrigin::Redacted(format!(
                        "The {} does not exist. Identification fields and values: <redacted>",
                        self.caption(t)
                    )),
                    ..Default::default()
                })))
            }
            None => unsupported("GetBySystemId of a missing row without using the result (error text not pinned)"),
        }
    }

    /// `ModifyAll(Field, Value [, RunTrigger])`: every row passing the filters, as they stand when
    /// the call starts, gets the value through `Modify(RunTrigger)`; the buffer is left alone
    /// (corpus record/TestRecordModify.al:87-170, TestTriggerContracts.al:123-146).
    fn modify_all(&mut self, rec: &Rec, t: &TableDef, args: &[Value]) -> Result<Value, Signal> {
        let pos = arg_pos(args, 0)?;
        if t.fields[pos].id == 2_000_000_000 {
            // Wording after the pinned fragment: record/TestRecordSystemId.al:230-262, docs/probes-needed.md.
            // record/TestSystemFieldStampsContracts.al:153 (SystemId_Duplicate_And_ModifyAll_Texts).
            return Err(error(
                format!(
                    "You cannot change the value of the SystemId field.{}{}Table: {}",
                    crate::records::BR,
                    crate::records::BR,
                    self.caption(t)
                ),
                "Dialog",
            ));
        }
        let value = ops::convert(args.get(1).cloned().unwrap_or(Value::Null), &t.fields[pos].ty)?;
        let run_trigger = match args.get(2) {
            Some(v) => ops::truthy(v)?,
            None => false,
        };
        let mut rows = Vec::new();
        let (key, mut after) = (rec.borrow().key, None);
        while let Some(row) = self.scan(rec, t, true, after.take())? {
            after = Some(al_data::position(t, key, &row));
            rows.push(row);
        }
        // One fresh set of table globals for all rows, not the caller's (ADR-021 decision 6; corpus
        // probe TableGlobals_ModifyAll_WhatTheTriggersAndSubscribersSee).
        let shared = al_builtins::value::TableGlobals::new_shared();
        for mut row in rows {
            row[pos] = value.clone();
            let copy = {
                let r = rec.borrow();
                RecordState {
                    fields: row,
                    filters: Vec::new(),
                    marks: Vec::new(),
                    marked_only: false,
                    position: None,
                    globals: shared.share(),
                    ..r.clone()
                }
            };
            let copy = Rc::new(RefCell::new(copy));
            self.record_op(RecordOp::Modify, &copy, &mut [Value::Boolean(run_trigger)])?;
        }
        Ok(Value::Null)
    }

    /// The `RecordId` of the row in `rec`'s buffer: the table and its primary key values.
    pub(crate) fn record_id(&self, rec: &Rec, t: &TableDef) -> Value {
        let key = t.primary_key(&rec.borrow().fields);
        Value::Library(
            "RecordId".into(),
            Rc::new(RefCell::new(al_builtins::value::LibraryValue::RecordId {
                table: t.object,
                table_no: self.engine.program.objects[t.object as usize].id.unwrap_or(0),
                name: t.name.clone(),
                key,
            })),
        )
    }

    /// `FieldError(Field [, Text])`. With a text: `<caption> <text> in <table> <key>.`; without:
    /// `<caption> must not be <value> in <table> <key>.` The key is `Name='value'` pairs joined by
    /// commas, no colon after the table (corpus record/TestRecordRuntimeTextsAndViews.al:
    /// Record_FieldError_Texts). The code is `NCLCSRTS:TableErrorStr`.
    fn field_error(&mut self, rec: &Rec, t: &TableDef, args: &[Value]) -> Result<Value, Signal> {
        let pos = arg_pos(args, 0)?;
        let row = rec.borrow().fields.clone();
        let mut parts = Vec::new();
        for &i in &t.keys[0] {
            parts.push(format!(
                "{}='{}'",
                self.field_caption(t, i),
                self.message_value(&row[i])?
            ));
        }
        let key = parts.join(",");
        // FieldError(Field, ErrorInfo) raises the same text with the ErrorInfo's message, code NCLCSRTS:TableErrorStr,
        // and leaves the ErrorInfo as it was; a collectible one is collected in a collecting scope as a copy
        // that holds the full text (handlers/PWRefusedBuiltins.al:113-168, BC 27.5).
        let mut info = None;
        // No text, an empty text and an ErrorInfo with an empty message all say what is wrong with the value: a blank Text, Date or
        // DateTime "You must specify <field>", any other value "<field> must not be <value>" (record/lt-testfield-fielderror-typed/
        // P96FieldError.al:FieldError_NoText_Empty_Row; P96LongText.al:LongText_FieldError_Message `fe_empty`).
        let mut what = match args.get(1) {
            Some(v) => match (al_builtins::native::text_of(v), al_builtins::errorinfo::message_of(v)) {
                (Some(text), _) => Some(text.into_string()),
                (None, Some(message)) => {
                    info = Some(v.clone());
                    Some(message)
                }
                _ => return unsupported("FieldError with an argument that is neither text nor an ErrorInfo"),
            },
            None => None,
        }
        .filter(|w| !w.is_empty());
        let given_text = what.is_some();
        let info_given = info.is_some();
        let blank_value =
            is_blank(&row[pos])? && matches!(row[pos], Value::Text(_) | Value::Date(..) | Value::DateTime(_));
        let full = match what.take() {
            Some(w) => format!("{} {w} in {} {key}.", self.field_caption(t, pos), self.caption(t)),
            None if blank_value => format!(
                "You must specify {} in {} {key}.",
                self.field_caption(t, pos),
                self.caption(t)
            ),
            None => format!(
                "{} must not be {} in {} {key}.",
                self.field_caption(t, pos),
                self.message_value(&row[pos])?,
                self.caption(t)
            ),
        };
        if let Some(info) = info {
            if let Some(copy) = al_builtins::errorinfo::copy_of(&info) {
                let collectible = al_builtins::errorinfo::set_message_of(&copy, &full);
                if collectible && self.collect_error(&copy) {
                    return Ok(Value::Null);
                }
            }
        }
        // The flag: P86LastError.al:270, :273 (the text given as a literal), :276, :279. Only the two forms the corpus pins;
        // the "You must specify" form and an ErrorInfo are not pinned.
        let redacted = match (given_text, blank_value, info_given) {
            (false, false, false) => Some(format!(
                "FieldError for incorrect value of {}.{}",
                self.caption(t),
                self.field_caption(t, pos)
            )),
            _ => None,
        };
        match redacted {
            Some(r) => Err(error_redacted(full, "NCLCSRTS:TableErrorStr", r)),
            None => Err(error(full, "NCLCSRTS:TableErrorStr")),
        }
    }
}
