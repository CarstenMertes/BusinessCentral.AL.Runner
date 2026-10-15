//! Record builtins that read table metadata or guard on session state:
//! `TestField`, `Truncate`, `FieldCaption`, `FieldName`, `FieldNo`,
//! `TableCaption`, `TableName`.

use crate::Session;
use al_builtins::describe::Describe;
use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::value::{RecordState, RtType, Value};
use al_data::{Store, TableDef};
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

type Rec = Rc<RefCell<RecordState>>;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn error(message: String, code: &str) -> Signal {
    Signal::Error(AlError {
        message,
        code: code.into(),
        call_stack: String::new(),
    })
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
        Value::Text(s) | Value::Code(s) | Value::DateFormula(s) => s.is_empty(),
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
        let ir_table = self.engine.program.objects[t.object as usize].table.as_ref();
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
        let flow_filter = |i: usize| ir_table.and_then(|s| s.fields.get(i)).is_some_and(|f| f.flow_filter);
        // The FlowFilter fields come last, with the value the field holds (0 for an Integer) whatever
        // filter was set on them (types/ProbeTypeFormsAndFilters.al:FormatRecord_WithFlowFilter_BeforeAndAfterSetRange,
        // BC 27.5). Where they stand among the FlowFields is not pinned.
        let (filters, rest): (Vec<usize>, Vec<usize>) = order.into_iter().partition(|&i| flow_filter(i));
        let (normal, calculated): (Vec<usize>, Vec<usize>) = rest.into_iter().partition(|&i| t.fields[i].stored);
        let mut parts = Vec::new();
        for i in normal.into_iter().chain(calculated).chain(filters) {
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
            | Value::DateFormula(_) => al_builtins::format::try_format(v, 0, 0, self).map_err(Signal::Unsupported),
            // A RecordId reads as Format gives it (`Table: key`, record/TestProbeRecordRefContracts.al:472-502); that an
            // error message shows it the same way is assumed (docs/probes-needed.md).
            Value::Library(name, _) if name == "RecordId" => {
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
        method: &str,
        rec: &Rec,
        t: &TableDef,
        args: &mut [Value],
    ) -> Result<Value, Signal> {
        self.connection_check(rec)?;
        match method {
            "AddLink" | "DeleteLink" | "DeleteLinks" | "HasLinks" | "CopyLinks" => {
                self.record_links(method, rec, t, args)
            }
            "TableName" => Ok(Value::Text(t.name.clone())),
            "TableCaption" => Ok(Value::Text(self.caption(t))),
            "FieldName" => Ok(Value::Text(t.fields[arg_pos(args, 0)?].name.clone())),
            "FieldCaption" => self.field_caption_checked(t, arg_pos(args, 0)?).map(Value::Text),
            "FieldNo" => Ok(Value::Integer(crate::refs::al_field_no(t.fields[arg_pos(args, 0)?].id))),
            // True for an enabled field (corpus record/TestRecordTriggers.al:223-231). What a field
            // with `Enabled = false` answers is not pinned: refused (docs/probes-needed.md).
            "FieldActive" => {
                let pos = arg_pos(args, 0)?;
                // False for `Enabled = false`, true for the rest, a Pending obsolete field included
                // (record/TestRowVersionFieldNumberFieldActiveContracts.al:110-119).
                Ok(Value::Boolean(self.field_enabled(t, pos)))
            }
            "RecordId" => Ok(self.record_id(rec, t)),
            "IsTemporary" => Ok(Value::Boolean(rec.borrow().temporary)),
            // Without an argument, whether the current row is marked; with one, marks or unmarks it.
            // What the call returns then (the previous state?) is not pinned, so a used result is refused.
            "Mark" => {
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
            "MarkedOnly" => match args.first() {
                None => Ok(Value::Boolean(rec.borrow().marked_only)),
                Some(v) => {
                    rec.borrow_mut().marked_only = ops::truthy(v)?;
                    Ok(Value::Null)
                }
            },
            "ClearMarks" => {
                rec.borrow_mut().marks.clear();
                Ok(Value::Null)
            }
            // The bounds of a range filter on the field in the current group. With no filter BC raises an
            // error whose text the corpus does not pin (record/TestFilterContracts.al:190-211): refused.
            "GetRangeMin" | "GetRangeMax" => {
                use al_builtins::filter::FilterExpr as F;
                let pos = arg_pos(args, 0)?;
                let r = rec.borrow();
                let Some(f) = r.filters.iter().find(|f| f.field == pos && f.group == r.filter_group) else {
                    return unsupported(format!("{method} without a filter (error text not pinned)"));
                };
                match (&f.expr, method) {
                    (F::Range(Some(lo), _), "GetRangeMin") | (F::Eq(lo), "GetRangeMin") => Ok(lo.clone()),
                    (F::Range(_, Some(hi)), "GetRangeMax") | (F::Eq(hi), "GetRangeMax") => Ok(hi.clone()),
                    _ => unsupported(format!("{method} of a filter that is not a single range")),
                }
            }
            // corpus record/TestRecordExtended.al:15-40: Record.Relation on a field without a TableRelation
            // raises an error whose text is not pinned (FieldRef.Relation answers 0 instead).
            "Relation" => {
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
            "LockTable" => {
                // corpus record/TestRecordLock.al:33-41 expects an error mentioning 'not supported' for Wait = false.
                if matches!(args.first(), Some(Value::Boolean(false))) {
                    return unsupported("LockTable(false) is rejected in the cloud (error text not pinned)");
                }
                Ok(Value::Null)
            }
            "Consistent" => Ok(Value::Null),
            "ReadConsistency" => Ok(Value::Boolean(false)),
            "RecordLevelLocking" => Ok(Value::Boolean(true)),
            "ReadPermission" | "WritePermission" => Ok(Value::Boolean(true)),
            "CountApprox" => self.record_op("Count", rec, &mut []),
            "CurrentCompany" => Ok(Value::Text(self.current_company_of(rec))),
            "ReadIsolation" | "SecurityFiltering" => {
                let (enum_name, is_iso) = if method == "ReadIsolation" {
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
            "GetPosition" => {
                let use_names = match args.first() {
                    Some(v) => ops::truthy(v)?,
                    None => true,
                };
                self.get_position(rec, t, use_names).map(Value::Text)
            }
            "SetPosition" => {
                let text = al_builtins::native::text_of(args.first().unwrap_or(&Value::Null)).unwrap_or_default();
                self.set_position(rec, t, &text).map(|()| Value::Null)
            }
            "TransferFields" => self.transfer_fields(rec, t, args),
            "GetBySystemId" => self.get_by_system_id(rec, t, args, false),
            "ModifyAll" => self.modify_all(rec, t, args),
            // Doc (Record.Ascending): with an argument sets the sort direction of the current key,
            // without one reads it (corpus record/TestRecordSort.al:111, TestCopyOntoTemporaryRecord.al).
            "Ascending" => {
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
            "ChangeCompany" => self.change_company(rec, args),
            "SetAscending" | "GetAscending" => {
                let pos = arg_pos(args, 0)?;
                if t.keys[rec.borrow().key] != [pos] {
                    return unsupported(format!(
                        "{method} on a field that is not the only field of the current key (per-field direction not pinned)"
                    ));
                }
                if method == "GetAscending" {
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
            "CurrentKey" => {
                let key = rec.borrow().key;
                Ok(Value::Text(
                    t.keys[key]
                        .iter()
                        // The hidden key on SystemId (recordref/TestRecordRefKeyIndexSetRecFilterContracts.al:101).
                        .map(|&p| {
                            if t.fields[p].id == 2_000_000_000 {
                                "System ID"
                            } else {
                                t.fields[p].name.as_str()
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(","),
                ))
            }
            "HasFilter" => Ok(Value::Boolean(Self::has_filter(rec))),
            "GetFilters" => self.get_filters(rec, t).map(Value::Text),
            "GetFilter" => self.field_filter_text(rec, arg_pos(args, 0)?).map(Value::Text),
            "GetView" => {
                let use_names = match args.first() {
                    Some(v) => ops::truthy(v)?,
                    None => true,
                };
                self.get_view(rec, t, use_names).map(Value::Text)
            }
            "SetView" => {
                let view = al_builtins::native::text_of(args.first().unwrap_or(&Value::Null)).unwrap_or_default();
                self.set_view(rec, t, &view).map(|()| Value::Null)
            }
            // Returns the group it sets, or the current one without an argument (corpus
            // record/TestRecordRuntimeTextsAndViews.al:Record_FilterGroup_ReturnsTheGroupJustSet_...).
            "FilterGroup" => Self::filter_group(rec, args),
            // Doc (Record.SetRecFilter): filters the primary key fields to the current values; the
            // filters on other fields stay (record/TestRecordViewCopyMarkContracts.al:41,
            // SetRecFilter_KeepsFiltersOnOtherFields_AndFiltersOnThePrimaryKey).
            "SetRecFilter" => {
                let group = rec.borrow().filter_group;
                let pk = &t.keys[0];
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
            "CopyFilter" => {
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
                                "The type of the constant must be compatible with that of the {to_name} field, and the length of the constant must not exceed the length of the field.\r\\\r\\Type: Text <-- Code\r\\Field: {to_name} <-- {from_name}\r\\Table: {table}",
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
            "CopyFilters" => {
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
            "Copy" => {
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
                d.load.buffer = s.load.buffer;
                d.load.partial = s.load.partial;
                if share && s.temporary {
                    d.temp_id = s.temp_id.clone();
                }
                Ok(Value::Null)
            }
            "TestField" => self.test_field(rec, t, args),
            "FieldError" => self.field_error(rec, t, args),
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
        // `TestField(Field [, Value], ErrorInfo)`: the check is the one without the ErrorInfo, and one
        // that holds raises nothing (documentation only, docs/probes-needed.md, issue 41; Base
        // Application's "Gen. Jnl.-Check Line" calls it 176 times over). What a failing one raises
        // from the ErrorInfo is not pinned: refused.
        if let Some((Value::Library(kind, _), plain)) = args.split_last()
            && kind.as_str() == "ErrorInfo"
        {
            return match self.test_field(rec, t, plain) {
                Ok(v) => {
                    al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::TestFieldErrorInfoPasses);
                    Ok(v)
                }
                Err(Signal::Error(_)) => unsupported(
                    "TestField with an ErrorInfo whose check fails: the error BC raises from the ErrorInfo is not pinned (docs/probes-needed.md)",
                ),
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
                    return Err(error(
                        format!("{caption} must have a value in {table}: {key}. It cannot be zero or empty."),
                        "TestField",
                    ));
                }
            }
            Some(want) => {
                let want = ops::convert(want.clone(), &t.fields[pos].ty)?;
                if ops::compare(&current, &want)? != Ordering::Equal {
                    let key = self.key_text(t, &row)?;
                    // record/TestRecordRuntimeTextsAndViews.al:Record_TestField_MismatchTexts: the
                    // wanted and the current value are quoted, and two spaces precede "in".
                    return Err(error(
                        format!(
                            "{caption} must be equal to '{}'  in {table}: {key}. Current value is '{}'.",
                            self.member_caption(t, pos, &want)?,
                            self.member_caption(t, pos, &current)?
                        ),
                        "TestField",
                    ));
                }
            }
        }
        Ok(Value::Null)
    }

    /// `GetPosition`: the primary key fields, whatever the current key is, as `Name=CONST(value)`
    /// (`Field<n>=0(value)` with `UseNames = false`); corpus
    /// record/TestRecordRuntimeTextsAndViews.al:RecordId_Format_And_GetPosition.
    fn get_position(&self, rec: &Rec, t: &TableDef, use_names: bool) -> Result<String, Signal> {
        let r = rec.borrow();
        let mut parts = Vec::new();
        for &p in &t.keys[0] {
            let value = self.message_value(&r.fields[p])?;
            if value.contains([')', ',', '\'', '"']) {
                return unsupported("a position value that needs quoting");
            }
            if use_names {
                parts.push(format!("{}=CONST({value})", t.fields[p].name));
            } else {
                parts.push(format!("Field{}=0({value})", t.fields[p].id));
            }
        }
        Ok(parts.join(","))
    }

    /// `SetPosition`: puts the key values of a position into the buffer (the row is not read).
    fn set_position(&self, rec: &Rec, t: &TableDef, text: &str) -> Result<(), Signal> {
        let mut values = Vec::new();
        for item in text.split(',') {
            let (name, rest) = item
                .split_once('=')
                .ok_or_else(|| Signal::Unsupported(format!("position `{text}`")))?;
            let value = rest
                .trim()
                .strip_prefix("CONST(")
                .or_else(|| rest.trim().strip_prefix("0("))
                .and_then(|v| v.strip_suffix(')'))
                .ok_or_else(|| Signal::Unsupported(format!("position `{text}`")))?;
            let name = name.trim();
            let pos = t
                .fields
                .iter()
                .position(|f| f.name.eq_ignore_ascii_case(name))
                .or_else(|| {
                    name.strip_prefix("Field")
                        .and_then(|n| n.parse::<u32>().ok())
                        .and_then(|id| t.fields.iter().position(|f| f.id == id))
                })
                .ok_or_else(|| Signal::Unsupported(format!("position field `{name}`")))?;
            let expr = self
                .parse_filter_text(value, &t.fields[pos].ty)
                .map_err(|e| Signal::Unsupported(format!("SetPosition: {e}")))?;
            let al_builtins::filter::FilterExpr::Eq(v) = expr else {
                return unsupported(format!("position value `{value}`"));
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
            // (assumed from Microsoft's passing tests; no corpus test, docs/probes-needed.md). The ordinal moves.
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
                    al_builtins::value::RtType::Enum(id) => {
                        if !self
                            .engine
                            .program
                            .objects
                            .get(*id as usize)
                            .is_some_and(|o| o.enum_values.iter().any(|(v, _)| *v == ordinal))
                        {
                            return unsupported(format!(
                                "TransferFields of the Option ordinal {ordinal} into the Enum field {}, which has no such value (not pinned)",
                                df.name
                            ));
                        }
                        Value::Enum(*id, ordinal)
                    }
                    _ => unreachable!("enum_option"),
                };
                al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::TransferFieldsEnumOption);
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
                Err(Signal::Error(AlError {
                    message: format!(
                        "The {} does not exist. Identification fields and values: {id}",
                        self.caption(t)
                    ),
                    code: "DB:RecordNotFound".into(),
                    call_stack: String::new(),
                }))
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
            self.record_op("Modify", &copy, &mut [Value::Boolean(run_trigger)])?;
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
        let what = match args.get(1) {
            Some(v) => match (al_builtins::native::text_of(v), al_builtins::errorinfo::message_of(v)) {
                (Some(text), _) => text,
                (None, Some(message)) => {
                    info = Some(v.clone());
                    message
                }
                _ => return unsupported("FieldError with an argument that is neither text nor an ErrorInfo"),
            },
            None => format!("must not be {}", self.message_value(&row[pos])?),
        };
        let full = format!("{} {what} in {} {key}.", self.field_caption(t, pos), self.caption(t));
        if let Some(info) = info {
            if let Some(copy) = al_builtins::errorinfo::copy_of(&info) {
                let collectible = al_builtins::errorinfo::set_message_of(&copy, &full);
                if collectible && self.collect_error(&copy) {
                    return Ok(Value::Null);
                }
            }
        }
        Err(error(full, "NCLCSRTS:TableErrorStr"))
    }
}
