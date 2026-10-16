//! Record operations on the store (ADR-003) for the `Record.*` builtins.
//!
//! Pinned by the corpus or the docs as noted; behaviour neither pins yet
//! raises `Signal::Unsupported` instead of guessing.

use crate::Session;
use crate::globaltriggers::GlobalOp;
use al_builtins::describe::Describe;
use al_builtins::filter::{FieldFilter, FilterExpr, key_cmp};
use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::record_op::RecordOp;
use al_builtins::value::{Instance, RecordState, RtType, Value};
use al_data::{InsertOutcome, RenameOutcome, Scan, Store, TableDef, WriteOutcome, memory::MemStore, position};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

pub(crate) type Rec = Rc<RefCell<RecordState>>;

/// BC's line break inside error texts: a carriage return and a backslash.
pub(crate) const BR: &str = "\\";

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn arg_bool(args: &[Value], i: usize) -> Result<bool, Signal> {
    match args.get(i) {
        None => Ok(false),
        Some(v) => ops::truthy(v),
    }
}

fn arg_pos(args: &[Value], i: usize) -> Result<usize, Signal> {
    match args.get(i) {
        Some(Value::Integer(p)) => Ok(*p as usize),
        other => unsupported(format!("field argument {}", other.describe())),
    }
}

/// What the `Rec` and `xRec` slots of a table instance held before a call filled them.
#[derive(Default)]
pub(crate) struct RecSlots {
    rec: Option<(usize, Value)>,
    xrec: Option<(usize, Value)>,
}

impl Session {
    pub(crate) fn table(&self, rec: &Rec) -> Result<Arc<TableDef>, Signal> {
        let r = rec.borrow();
        let t = self.engine.tables[r.table as usize]
            .clone()
            .map_or_else(|| unsupported("record of a table without a schema"), Ok)?;
        // A key set on fields no key starts with is one more key, as far as ordering goes.
        if let Some((index, fields)) = &r.adhoc {
            if r.key == *index && *index == t.keys.len() {
                // The table with that extra key is built once per (table, key fields), not on every operation.
                let mut cache = self.adhoc_tables.borrow_mut();
                let entry = cache.entry((r.table, fields.clone())).or_insert_with(|| {
                    let mut with_key = (*t).clone();
                    with_key.keys.push(fields.clone());
                    Arc::new(with_key)
                });
                return Ok(entry.clone());
            }
        }
        Ok(t)
    }

    /// Field values of a fresh record: `InitValue` where declared, else the type default.
    pub(crate) fn initial_fields(&mut self, table: usize) -> Vec<Value> {
        // The values are the same for every record of the table, so a table whose defaults are all scalars is
        // worked out once per session and copied (a text shares its buffer, copy on write); a table with a Blob, an
        // array or another shared object builds fresh ones each time.
        if let Some(Some(template)) = self.initial_cache.get(&table) {
            return template.clone();
        }
        let engine = self.engine.clone();
        let Some(t) = engine.program.objects[table].table.as_ref() else {
            return Vec::new();
        };
        let fields: Vec<Value> = t
            .fields
            .iter()
            .map(|f| match &f.init {
                Some(c) => ops::convert(crate::constant(c), &f.ty).unwrap_or_else(|_| self.default_value(&f.ty)),
                None => self.default_value(&f.ty),
            })
            .collect();
        if let std::collections::hash_map::Entry::Vacant(slot) = self.initial_cache.entry(table) {
            let scalar = fields.iter().all(|v| {
                matches!(
                    v,
                    Value::Null
                        | Value::Boolean(_)
                        | Value::Integer(_)
                        | Value::BigInteger(_)
                        | Value::Decimal(_)
                        | Value::Byte(_)
                        | Value::Char(_)
                        | Value::Text(_)
                        | Value::Code(_)
                        | Value::Date(..)
                        | Value::Time(_)
                        | Value::DateTime(_)
                        | Value::Duration(_)
                        | Value::Guid(_)
                        | Value::DateFormula(_)
                        | Value::Option(..)
                        | Value::Enum(..)
                )
            });
            slot.insert(scalar.then(|| fields.clone()));
        }
        fields
    }

    pub(crate) fn store_for(&mut self, rec: &Rec) -> &mut MemStore {
        let id = rec.borrow().temp_id.as_ref().map(|t| t.id());
        match id {
            Some(id) => {
                // Tables whose last record variable is gone are freed first.
                if !self.temp_release.borrow().is_empty() {
                    for dead in self.temp_release.borrow_mut().drain(..) {
                        self.temps.remove(&dead);
                    }
                }
                self.temps.entry(id).or_default()
            }
            None => &mut self.store,
        }
    }

    pub(crate) fn caption(&self, t: &TableDef) -> String {
        self.engine.program.objects[t.object as usize]
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("Caption"))
            .map(|(_, v)| v.trim().trim_matches('\'').to_string())
            .unwrap_or_else(|| t.name.clone())
    }

    /// `No.='A'` style identification of a row's primary key. A key value whose format is not
    /// pinned refuses.
    pub(crate) fn identification(&self, t: &TableDef, fields: &[Value]) -> Result<String, Signal> {
        let mut parts = Vec::new();
        for &i in &t.keys[0] {
            let v = al_builtins::format::try_format(&fields[i], 0, 0, self).map_err(Signal::Unsupported)?;
            parts.push(format!("{}='{}'", t.fields[i].name, v));
        }
        Ok(parts.join(","))
    }

    /// Refuses a read whose answer depends on the rows of table `t` when the install baseline could
    /// not create the rows BC has there (`Engine::unseeded`, ADR-013 decision 5): the answer would
    /// be "empty" or "fewer rows", which is not BC's. Rows this session wrote itself do not make
    /// the answer BC's, so they are not an exception. Temporary records are private to the session.
    pub(crate) fn unseeded_read(&self, t: &TableDef, temporary: bool, what: &str) -> Result<(), Signal> {
        if !temporary && self.engine.unseeded.contains(&(t.object as usize)) {
            return unsupported(format!(
                "{what} on {}: BC holds rows here that an install trigger or Company-Initialize creates, and the install baseline could not create them (ADR-013 decision 5)",
                t.name
            ));
        }
        Ok(())
    }

    /// Message texts: docs/probes-needed.md (the corpus pins 'already exists'
    /// and 'does not exist' fragments only).
    fn fail(&mut self, t: &TableDef, fields: &[Value], exists: bool) -> Result<Value, Signal> {
        // Code reads rows (setup records, mostly) that installing a dependency app and
        // initialising a company create. Where the install baseline could not create them
        // (ADR-013 decision 5), a missing row there is not BC's answer, whoever asks (a dependency
        // app, or the tests of the project): refuse rather than report it as one.
        if !exists && self.engine.unseeded.contains(&(t.object as usize)) {
            return unsupported(format!(
                "{} row not found: the rows a dependency app's install and Company-Initialize create could not be created (ADR-013 decision 5)",
                t.name
            ));
        }
        if self.ret_used {
            return Ok(Value::Boolean(false));
        }
        let identification = self.identification(t, fields)?;
        let (message, code) = if exists {
            (
                format!(
                    "The record in table {} already exists. Identification fields and values: {}",
                    self.caption(t),
                    identification
                ),
                "DB:RecordExists",
            )
        } else {
            (
                format!(
                    "The {} does not exist. Identification fields and values: {}",
                    self.caption(t),
                    identification
                ),
                "DB:RecordNotFound",
            )
        };
        Err(Signal::Error(Box::new(AlError {
            message,
            code: code.into(),
            call_stack: String::new(),
        })))
    }

    /// The table-level trigger `name` (`OnBeforeInsert`, `OnInsert`, ...) of every table extension that declares it,
    /// in the order of the extensions' object ids: two extensions run the same trigger one after the other
    /// (corpus events/TestProbeEventContracts.al:ExtensionTriggers_TwoExtensionsSameTrigger_Order, ids 69800 and 69801).
    fn ext_triggers(&self, t: &TableDef, name: &str) -> Vec<(usize, usize)> {
        let mut found = Vec::new();
        if let Some(schema) = &self.engine.program.objects[t.object as usize].table {
            for &e in &schema.extensions {
                for (ri, r) in self.engine.program.objects[e].routines.iter().enumerate() {
                    if r.is_trigger && r.field.is_none() && r.name.eq_ignore_ascii_case(name) {
                        found.push((e, ri));
                    }
                }
            }
        }
        found.sort_by_key(|(e, _)| self.engine.program.objects[*e].id.unwrap_or(u32::MAX));
        found
    }

    /// Runs the extension trigger `name`, if any, with the record and `xrec`.
    fn run_ext_trigger(&mut self, t: &TableDef, name: &str, rec: &Rec, xrec: Option<&Rec>) -> Result<(), Signal> {
        for (o, r) in self.ext_triggers(t, name) {
            self.run_trigger(o, r, rec, xrec)?;
        }
        Ok(())
    }

    /// Whether `event` of table `t` would reach a subscriber now (static ones, and
    /// manual ones with a bound instance).
    fn listened(&mut self, t: &TableDef, event: &str, element: Option<&str>) -> Result<bool, Signal> {
        self.has_listeners(t.object as usize, event, element)
    }

    /// A record handle holding `fields` and otherwise `rec`'s state: an `xRec`.
    fn xrec_handle(rec: &Rec, fields: Vec<Value>) -> Rec {
        let mut st = rec.borrow().clone();
        st.fields = fields;
        // The variable's xRec keeps its globals from one operation to the next (ADR-021).
        st.globals = rec.borrow().globals.xrec();
        Rc::new(RefCell::new(st))
    }

    /// Raises table event `event` (lower-case, e.g. `onbeforeinsertevent`) for
    /// `rec`. Table events fire whether or not the trigger runs
    /// (record/TestStorageLayerFieldWriteRoutes.al: Insert(false)).
    /// `Rec` and `xRec` are passed by reference, so subscribers' changes show
    /// in the caller and in later subscribers
    /// (record/TestTableEventByRefMutation.al).
    fn table_event(
        &mut self,
        t: &TableDef,
        event: &str,
        rec: &Rec,
        xrec: Option<&Rec>,
        run_trigger: bool,
        element: Option<&str>,
    ) -> Result<(), Signal> {
        if !self.listened(t, event, element)? {
            return Ok(());
        }
        // A temporary record raises the table events and runs the triggers like any other
        // (corpus record/TestTableEventSequenceContracts.al:TemporaryRecord_RaisesTheEventsAndRunsTheTriggers).
        let (names, tail): (&[&str], Value) = match event {
            "onbeforeinsertevent" | "onafterinsertevent" | "onbeforedeleteevent" | "onafterdeleteevent" => {
                (&["Rec", "RunTrigger"], Value::Boolean(run_trigger))
            }
            "onbeforemodifyevent" | "onaftermodifyevent" | "onbeforerenameevent" | "onafterrenameevent" => {
                (&["Rec", "xRec", "RunTrigger"], Value::Boolean(run_trigger))
            }
            // CurrFieldNo is 0 for a Validate called from code (the field's own OnValidate
            // sees 0: handlers/TestPageCurrFieldNo_Tests.al); docs/probes-needed.md for the event.
            // Through a TestPage write it is the field's number (record/lt-loadfields-pagesource-validate/P81PageValidateEvents.al:
            // `cur=2`).
            "onbeforevalidateevent" | "onaftervalidateevent" => {
                (&["Rec", "xRec", "CurrFieldNo"], Value::Integer(self.curr_field_of(rec)))
            }
            other => return unsupported(format!("table event {other}")),
        };
        let mut args = vec![Value::Record(rec.clone())];
        if names.len() == 3 {
            let Some(x) = xrec else {
                return unsupported(format!("{event} without an xRec"));
            };
            args.push(Value::Record(x.clone()));
        }
        args.push(tail);
        let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        let call = crate::dispatch::EventCall {
            object: t.object as usize,
            event,
            element,
            isolated: false,
            names: &names,
            sender: None,
        };
        self.raise(&call, &mut args)
    }

    /// The Before event, the field's OnValidate and the After event of a `Validate`.
    fn validate_hooks(
        &mut self,
        t: &TableDef,
        rec: &Rec,
        xrec: &Rec,
        field: &al_ir::Field,
        snapshot: Vec<Value>,
    ) -> Result<(), Signal> {
        // Before event (Rec holds the new value, xRec the old row), the field's OnValidate,
        // After event: record/TestTriggerDispatchOrder.al:TriggerOrder_Validate_FieldPipeline_IsStable.
        // That the Before event sees the new value is docs/probes-needed.md.
        self.table_event(t, "onbeforevalidateevent", rec, Some(xrec), false, Some(&field.name))?;
        // The triggers table extensions attach to the field bracket its own OnValidate
        // (record/TestTriggerDispatchOrder.al:TriggerOrder_Validate_FieldPipeline_IsStable). The
        // order of two extensions' triggers on one field is not pinned.
        if field.ext_before_validate.len() > 1 || field.ext_after_validate.len() > 1 {
            return unsupported(format!(
                "Validate of {}.{}: more than one table extension declares OnBeforeValidate or OnAfterValidate for it (their order is not pinned)",
                t.name, field.name
            ));
        }
        for &(object, routine) in &field.ext_before_validate {
            self.run_trigger(object, routine, rec, Some(xrec))?;
        }
        if let Some((object, routine)) = field.on_validate {
            if let Err(e) = self.run_trigger(object, routine, rec, Some(xrec)) {
                // The whole record is back at its values from before the Validate when the
                // field's OnValidate raises, not only the validated field
                // (record/TestValidateRaisesAndHandlerAsserterrorContracts.al:68,
                // Validate_ThatRaises_RestoresTheWholeRecord; the validated field alone:
                // record/TestTriggerRollback.al:OnValidate_Throws_FieldValueRolledBack).
                if matches!(e, Signal::Error(_)) {
                    rec.borrow_mut().fields = snapshot;
                }
                return Err(e);
            }
        }
        for &(object, routine) in &field.ext_after_validate {
            self.run_trigger(object, routine, rec, Some(xrec))?;
        }
        self.table_event(t, "onaftervalidateevent", rec, Some(xrec), false, Some(&field.name))
    }

    /// `Record.Delete(run)` of the row `rec` holds: Before event, OnDelete, the
    /// delete, After event.
    fn delete_row(&mut self, t: &TableDef, rec: &Rec, run: bool) -> Result<Value, Signal> {
        let cx = self.cx_for(rec, t)?;
        self.table_event(t, "onbeforedeleteevent", rec, None, run, None)?;
        if run {
            let x = Self::xrec_handle(rec, rec.borrow().fields.clone());
            self.run_ext_trigger(t, "OnBeforeDelete", rec, Some(&x))?;
            if let Some(tr) = self.table_trigger(t, "OnDelete") {
                self.run_trigger(t.object as usize, tr, rec, Some(&x))?;
            }
            self.run_ext_trigger(t, "OnDelete", rec, Some(&x))?;
        }
        self.global_trigger(t, GlobalOp::Delete, rec, None)?;
        let user_row = self.is_user_row(t, rec.borrow().temporary);
        if user_row {
            self.user_before_delete(t, rec)?;
        }
        if self.is_profile_row(t, rec.borrow().temporary) {
            self.profile_before_delete(t, &rec.borrow().fields)?;
        }
        let row = rec.borrow().fields.clone();
        let pk = t.primary_key(&row);
        let company = if self.is_company_table(t) && !rec.borrow().temporary {
            let name = al_builtins::native::text_of(&row[0]).unwrap_or_default();
            self.before_company_row_delete(&name, run)?;
            Some(name)
        } else {
            None
        };
        match self
            .store_for(rec)
            .delete(&cx, t, &pk)
            .map_err(|e| Signal::Unsupported(format!("{e}")))?
        {
            WriteOutcome::Done => {
                if let Some(name) = company {
                    self.after_company_row_delete(&name);
                }
                // The platform deletes the user's User Property row with the User, after the
                // OnBeforeDelete subscribers and before the OnAfterDelete ones
                // (access-control/TestUserDeleteCascadeOrder.al:26-84 in the OnPrem app).
                if user_row {
                    self.user_after_delete(t, rec)?;
                }
                if run {
                    let x = Self::xrec_handle(rec, rec.borrow().fields.clone());
                    self.run_ext_trigger(t, "OnAfterDelete", rec, Some(&x))?;
                }
                self.table_event(t, "onafterdeleteevent", rec, None, run, None)?;
                Ok(Value::Boolean(true))
            }
            WriteOutcome::NotFound => self.fail(t, &row, false),
        }
    }

    /// The row as the store keeps it: FlowFields and FlowFilters hold no data, whatever the
    /// buffer shows after CalcFields or an assignment.
    pub(crate) fn stored_view(&mut self, t: &TableDef, row: &[Value]) -> Vec<Value> {
        row.iter()
            .enumerate()
            .map(|(i, v)| {
                if t.fields[i].stored {
                    v.clone()
                } else {
                    self.default_value(&t.fields[i].ty)
                }
            })
            .collect()
    }

    /// The first unique secondary key whose values `row` repeats in another row. `own` is the
    /// primary key of the row being rewritten (Modify, Rename), which does not count. For an
    /// Insert, a row with the same primary key is the duplicate-key failure instead.
    fn unique_violation(
        &mut self,
        rec: &Rec,
        t: &TableDef,
        row: &[Value],
        own: Option<&[Value]>,
    ) -> Result<Option<usize>, Signal> {
        let unique: Vec<usize> = {
            let schema = self.engine.program.objects[t.object as usize]
                .table
                .as_ref()
                .expect("table");
            (1..t.keys.len())
                .filter(|k| schema.unique_keys.get(*k).copied().unwrap_or(false))
                .collect()
        };
        if unique.is_empty() {
            return Ok(None);
        }
        let cx = self.cx_for(rec, t)?;
        if own.is_none() {
            let pk = t.primary_key(row);
            if self
                .store_for(rec)
                .get(&cx, t, &pk)
                .map_err(|e| Signal::Unsupported(format!("{e}")))?
                .is_some()
            {
                return Ok(None);
            }
        }
        let current = match own {
            Some(pk) => self
                .store_for(rec)
                .get(&cx, t, pk)
                .map_err(|e| Signal::Unsupported(format!("{e}")))?,
            None => None,
        };
        for k in unique {
            let filters: Vec<FieldFilter> = t.keys[k]
                .iter()
                .map(|&f| FieldFilter {
                    field: f,
                    expr: FilterExpr::Eq(row[f].clone()),
                    group: 0,
                })
                .collect();
            let mut n = self
                .store_for(rec)
                .count(&cx, t, &filters)
                .map_err(|e| Signal::Unsupported(format!("{e}")))?;
            // The row being rewritten matches itself when its key values do not change.
            if let Some(old) = &current {
                if t.keys[k]
                    .iter()
                    .all(|&f| ops::compare_keys(&[old[f].clone()], &[row[f].clone()]))
                {
                    n = n.saturating_sub(1);
                }
            }
            if n > 0 {
                return Ok(Some(k));
            }
        }
        Ok(None)
    }

    /// The refusal of a unique index; only the start of BC's message is pinned.
    fn fail_unique(&mut self, t: &TableDef, key: usize, row: &[Value]) -> Result<Value, Signal> {
        if self.ret_used {
            return Ok(Value::Boolean(false));
        }
        // The fields of the violated key with their values (corpus
        // record/TestCalcFieldsTruncateUniqueContracts.al:UniqueIndex_Violation_FullTextAndCode_...).
        let mut parts = Vec::new();
        for &i in &t.keys[key] {
            let v = al_builtins::format::try_format(&row[i], 0, 0, self).map_err(Signal::Unsupported)?;
            parts.push(format!("{}='{}'", t.fields[i].name, v));
        }
        Err(Signal::Error(Box::new(AlError {
            message: format!(
                "There is already a record in table {} that has the same values in a unique index for the following fields: {}",
                self.caption(t),
                parts.join(",")
            ),
            code: "DB:RecordExists".into(),
            call_stack: String::new(),
        })))
    }

    /// Whether an insert or modify of `t` would run table code (a trigger, an extension trigger
    /// or an event subscriber). XmlPort import refuses such tables: whether it runs them is not
    /// pinned (docs/probes-needed.md).
    #[allow(dead_code)] // the XmlPort import ran no hooks before; it runs them now
    pub(crate) fn has_write_hooks(&mut self, t: &TableDef) -> Result<bool, Signal> {
        let mut any = ["OnInsert", "OnModify"]
            .iter()
            .any(|n| self.table_trigger(t, n).is_some());
        for n in [
            "OnBeforeInsert",
            "OnInsert",
            "OnAfterInsert",
            "OnBeforeModify",
            "OnModify",
            "OnAfterModify",
        ] {
            any |= !self.ext_triggers(t, n).is_empty();
        }
        for e in [
            "onbeforeinsertevent",
            "onafterinsertevent",
            "onbeforemodifyevent",
            "onaftermodifyevent",
        ] {
            any |= self.listened(t, e, None)?;
        }
        Ok(any)
    }

    fn table_trigger(&self, t: &TableDef, name: &str) -> Option<usize> {
        self.engine.program.objects[t.object as usize]
            .routines
            .iter()
            .position(|r| r.is_trigger && r.field.is_none() && r.name.eq_ignore_ascii_case(name))
    }

    /// An instance of `object` whose Rec is `rec`, for a call on that record.
    ///
    /// A table or table extension runs on the globals of the record variable (ADR-021): one
    /// instance per object, created on the first call and kept in `rec`'s state. Its `Rec` and
    /// `xRec` slots are filled for the call; give the returned [`RecSlots`] to
    /// [`Session::leave_record`] when the call ends, whether it succeeded or not, so the slots go
    /// back to what they held (nothing, or the outer call's record) and the instance holds no
    /// pointer to its record at rest. Any other object (a codeunit with `TableNo`, a page) gets a
    /// fresh instance, as before.
    pub(crate) fn enter_record(
        &mut self,
        object: usize,
        rec: Rec,
        xrec: Option<&Rec>,
    ) -> Result<(Rc<RefCell<Instance>>, RecSlots), Signal> {
        let (rs, xs, per_variable, is_ext) = {
            let o = &self.engine.program.objects[object];
            (
                o.rec_slot,
                o.xrec_slot,
                !o.uncompiled && matches!(o.kind, al_ir::ObjKind::Table | al_ir::ObjKind::TableExtension),
                !o.uncompiled && o.kind == al_ir::ObjKind::TableExtension,
            )
        };
        let existing = if per_variable {
            rec.borrow().globals.instance(object as u32)
        } else {
            None
        };
        let inst = match existing {
            Some(i) => i,
            None => {
                let i = if per_variable {
                    self.table_instance(object)
                } else {
                    self.instance(object)
                };
                if per_variable {
                    rec.borrow().globals.set_instance(object as u32, i.clone());
                }
                i
            }
        };
        // A table extension's `BaseGlobal` places are the globals of the base table's instance for the same record
        // (assumption `table-extension-base-vars`): the link is made here, and made again when the base instance is
        // a new one (a Clear or a Reset).
        if is_ext {
            let table = rec.borrow().table;
            let base = match rec.borrow().globals.instance(table) {
                Some(b) => b,
                None => {
                    let b = self.table_instance(table as usize);
                    rec.borrow().globals.set_instance(table, b.clone());
                    b
                }
            };
            inst.borrow_mut().base = Some(Rc::downgrade(&base));
        }
        // xRec: the caller's handle when the operation has one (shared, so changes made by
        // event subscribers reach the trigger and the other subscribers), else a copy of Rec. Both
        // carry the variable's xRec globals (ADR-021 decision 5).
        let xr = match (xrec, xs) {
            (Some(x), _) => {
                if x.borrow().globals.is_unset() {
                    let g = rec.borrow().globals.xrec();
                    x.borrow_mut().globals = g;
                }
                Some(Value::Record(x.clone()))
            }
            (None, Some(_)) => {
                // In a procedure that no operation of the record variable called (a plain call), xRec
                // is a blank record, not a copy of the row: Get, Insert, Modify and edits of the
                // variable do not reach it; only the globals it was given stay (corpus
                // record/PWTableGlobals.al:196-260, XRecPlainProc_*).
                let mut st = rec.borrow().clone();
                let table = rec.borrow().table as usize;
                st.fields = self.initial_fields(table);
                st.globals = rec.borrow().globals.xrec();
                Some(Value::Record(Rc::new(RefCell::new(st))))
            }
            (None, None) => None,
        };
        let mut i = inst.borrow_mut();
        let mut saved = RecSlots::default();
        if let Some(r) = rs {
            saved.rec = Some((r, std::mem::replace(&mut i.globals[r], Value::Record(rec))));
        }
        if let (Some(x), Some(v)) = (xs, xr) {
            saved.xrec = Some((x, std::mem::replace(&mut i.globals[x], v)));
        }
        drop(i);
        Ok((inst, saved))
    }

    /// Puts the `Rec` and `xRec` slots back to what they held before [`Session::enter_record`].
    pub(crate) fn leave_record(inst: &Rc<RefCell<Instance>>, saved: RecSlots) {
        let mut i = inst.borrow_mut();
        if let Some((slot, v)) = saved.rec {
            i.globals[slot] = v;
        }
        if let Some((slot, v)) = saved.xrec {
            i.globals[slot] = v;
        }
    }

    /// Runs `routine` of `object` with `rec` as its Rec (and `xrec`, or a copy of `rec`, as its xRec).
    pub(crate) fn call_on_record(
        &mut self,
        object: usize,
        routine: usize,
        rec: Rec,
        xrec: Option<&Rec>,
        args: Vec<Value>,
    ) -> Result<(Value, Vec<Value>), Signal> {
        let (inst, saved) = self.enter_record(object, rec, xrec)?;
        let r = self.call(object, routine, inst.clone(), args);
        Self::leave_record(&inst, saved);
        r
    }

    /// Sets the table globals of `rec` back to their initial values, in place: the instances
    /// stay (a running call keeps its own), their variables are re-initialised. `xRec`'s set is
    /// not touched (ADR-021 decision 4 and 5).
    pub(crate) fn reset_table_globals(&mut self, rec: &Rec) {
        let instances = rec.borrow().globals.instances();
        for (object, inst) in instances {
            let (rs, xs, tys) = {
                let o = &self.engine.program.objects[object as usize];
                (
                    o.rec_slot,
                    o.xrec_slot,
                    o.globals.iter().map(|g| g.ty.clone()).collect::<Vec<_>>(),
                )
            };
            for (i, ty) in tys.iter().enumerate() {
                if Some(i) == rs || Some(i) == xs {
                    continue;
                }
                let v = self.default_value(ty);
                inst.borrow_mut().globals[i] = v;
            }
        }
    }

    fn run_trigger(&mut self, object: usize, routine: usize, rec: &Rec, xrec: Option<&Rec>) -> Result<(), Signal> {
        self.call_on_record(object, routine, rec.clone(), xrec, Vec::new())
            .map(|_| ())
    }

    /// Every write gets a new, increasing SystemRowVersion.
    /// Returns the previous value, so a refused write can give it back
    /// (record/TestRefusedWriteRowVersion.al).
    fn bump_row_version(&mut self, t: &TableDef, rec: &Rec) -> Option<(usize, Value)> {
        let i = t.fields.iter().position(|f| f.id == 2_000_000_005)?;
        self.next_row_version += 1;
        let old = std::mem::replace(
            &mut rec.borrow_mut().fields[i],
            Value::BigInteger(self.next_row_version),
        );
        Some((i, old))
    }

    fn restore_row_version(rec: &Rec, old: Option<(usize, Value)>) {
        if let Some((i, v)) = old {
            rec.borrow_mut().fields[i] = v;
        }
    }

    /// Gives the row its SystemId: the one in the buffer when `keep` (`Insert(_, true)`) and it is
    /// set, otherwise a fresh one (corpus record/TestRecordInsert.al:84-110).
    fn assign_system_id(&mut self, t: &TableDef, rec: &Rec, keep: bool) {
        // A temporary record has no SystemId: it stays the null Guid
        // (record/TestSystemFieldStampsContracts.al:115, TemporaryRecord_HasNoStampsAndNoSystemId).
        if rec.borrow().temporary {
            return;
        }
        if let Some(i) = t.fields.iter().position(|f| f.id == 2_000_000_000) {
            let mut r = rec.borrow_mut();
            let unset = matches!(r.fields[i], Value::Guid(g) if g == [0; 16]);
            if unset || !keep {
                // Deterministic ids that look random (ADR-006, `fresh_guid`).
                r.fields[i] = Value::Guid(self.fresh_system_id());
            }
        }
    }

    pub(crate) fn scan(
        &mut self,
        rec: &Rec,
        t: &TableDef,
        ascending: bool,
        after: Option<Vec<Value>>,
    ) -> Result<Option<Vec<Value>>, Signal> {
        if rec.borrow().field_descending {
            return self.scan_first_field_reversed(rec, t, ascending, after);
        }
        self.scan_plain(rec, t, ascending, after)
    }

    /// A scan in the order `SetAscending(Field, false)` gives: the first field of the key runs backwards and the rest
    /// of the key, then the primary key, forwards (records/W4SortKeys.al:SetAscending_SingleFieldKey: key Txt, ids
    /// `4,2,3,1,5`, where `Ascending(false)` and a view give `4,3,2,5,1`). The rows are read in key order and sorted
    /// again, so a walk over a big table costs more than one in the key's own order.
    fn scan_first_field_reversed(
        &mut self,
        rec: &Rec,
        t: &TableDef,
        forward: bool,
        after: Option<Vec<Value>>,
    ) -> Result<Option<Vec<Value>>, Signal> {
        use std::cmp::Ordering;
        let key = rec.borrow().key;
        let mut rows: Vec<(Vec<Value>, Vec<Value>)> = Vec::new();
        let mut cursor = None;
        while let Some(row) = self.scan_plain(rec, t, true, cursor.take())? {
            let pos = position(t, key, &row);
            cursor = Some(pos.clone());
            rows.push((pos, row));
        }
        let order = |a: &[Value], b: &[Value]| -> Ordering {
            for (i, (x, y)) in a.iter().zip(b).enumerate() {
                let o = ops::compare(x, y).unwrap_or(Ordering::Equal);
                let o = if i == 0 { o.reverse() } else { o };
                if o != Ordering::Equal {
                    return o;
                }
            }
            Ordering::Equal
        };
        rows.sort_by(|a, b| order(&a.0, &b.0));
        let hit = if forward {
            rows.into_iter()
                .find(|(p, _)| after.as_deref().is_none_or(|a| order(p, a) == Ordering::Greater))
        } else {
            rows.into_iter()
                .rev()
                .find(|(p, _)| after.as_deref().is_none_or(|a| order(p, a) == Ordering::Less))
        };
        Ok(hit.map(|(_, row)| row))
    }

    fn scan_plain(
        &mut self,
        rec: &Rec,
        t: &TableDef,
        ascending: bool,
        after: Option<Vec<Value>>,
    ) -> Result<Option<Vec<Value>>, Signal> {
        let temporary = rec.borrow().temporary;
        self.unseeded_read(t, temporary, "reading rows")?;
        self.coverage_read(t, temporary)?;
        let key = rec.borrow().key;
        // Filters on FlowFilter fields drop out; those on FlowFields are checked per row.
        let (filters, flow_field_filters) = self.split_filters(rec);
        let marked_only = rec.borrow().marked_only;
        let cx = self.cx_for(rec, t)?;
        let mut after = after;
        loop {
            let store = self.store_for(rec);
            let row = store
                .next(
                    &cx,
                    t,
                    &Scan {
                        key,
                        filters: &filters,
                        ascending,
                        after: after.as_deref(),
                    },
                )
                .map_err(|e| Signal::Unsupported(format!("store error {e}")))?;
            match row {
                // MarkedOnly narrows the set to the marked rows (corpus record/TestRecordFind.al:258-290).
                Some(row) if marked_only && !Self::is_marked(rec, t, &row) => after = Some(position(t, key, &row)),
                Some(row) if !flow_field_filters.is_empty() => {
                    if self.passes_flow_field_filters(rec, &row, &flow_field_filters)? {
                        return Ok(Some(row));
                    }
                    after = Some(position(t, key, &row));
                }
                other => return Ok(other),
            }
        }
    }

    /// Whether `row` passes the record's filters (every group), its FlowField filters and
    /// MarkedOnly.
    fn row_passes(&mut self, rec: &Rec, t: &TableDef, row: &[Value]) -> Result<bool, Signal> {
        let (filters, flow_field_filters) = self.split_filters(rec);
        if filters.iter().any(|f| !f.expr.matches(&row[f.field])) {
            return Ok(false);
        }
        if rec.borrow().marked_only && !Self::is_marked(rec, t, row) {
            return Ok(false);
        }
        if flow_field_filters.is_empty() {
            return Ok(true);
        }
        self.passes_flow_field_filters(rec, row, &flow_field_filters)
    }

    /// Whether the record last read the row with primary key `pk` (see `RecordState::position`).
    fn on_own_row(rec: &Rec, pk: &[Value]) -> bool {
        rec.borrow()
            .position
            .as_deref()
            .is_some_and(|p| ops::compare_keys(p, pk))
    }

    fn is_marked(rec: &Rec, t: &TableDef, row: &[Value]) -> bool {
        let pk = t.primary_key(row);
        rec.borrow()
            .marks
            .iter()
            .any(|m| key_cmp(m, &pk) == std::cmp::Ordering::Equal)
    }

    /// Refuses an explicit SystemId another row already holds. The wording after the pinned
    /// fragments is docs/probes-needed.md (record/TestRecordSystemId.al:86-168, TestAutoIncrementEdges.al:282).
    fn check_system_id_unique(&mut self, rec: &Rec, t: &TableDef) -> Result<(), Signal> {
        let Some(i) = t.fields.iter().position(|f| f.id == 2_000_000_000) else {
            return Ok(());
        };
        let id = rec.borrow().fields[i].clone();
        let filters = [FieldFilter {
            field: i,
            expr: FilterExpr::Eq(id.clone()),
            group: 0,
        }];
        let cx = self.cx_for(rec, t)?;
        let taken = self
            .store_for(rec)
            .count(&cx, t, &filters)
            .map_err(|e| Signal::Unsupported(format!("{e}")))?;
        if taken == 0 {
            return Ok(());
        }
        let shown = al_builtins::format::try_format(&id, 0, 0, self).map_err(Signal::Unsupported)?;
        Err(Signal::Error(Box::new(AlError {
            // record/TestSystemFieldStampsContracts.al:153 (SystemId_Duplicate_And_ModifyAll_Texts;
            // the caption of the field is "System ID").
            message: format!(
                "There is already a record in table {} that has the same values in a unique index for the following fields: System ID='{}'",
                self.caption(t),
                shown
            ),
            code: "DB:RecordExists".into(),
            call_stack: String::new(),
        })))
    }

    /// Reads a row into the record, then calculates its `SetAutoCalcFields` fields. A database
    /// table hands out its own copy of every Blob, not yet loaded
    /// (streams/TestBlobUncommittedWriteIsolation.al, TestBlob.al Blob_Get_WithoutCalcFields_InStreamReadsEmpty);
    /// a temporary table's rows hold the record's own Blob objects
    /// (TempBlob_UncommittedWrite_SameInstanceReGet_KeepsWrite).
    pub(crate) fn load(&mut self, rec: &Rec, mut row: Vec<Value>) -> Result<(), Signal> {
        if rec.borrow().temp_id.is_none() {
            for v in &mut row {
                if let Value::Blob(b) = v {
                    *b = al_builtins::stream::copy_blob(b, false);
                }
            }
        }
        // A full load (the default) needs no bookkeeping beyond the flag; a partial one is worked
        // out in partial.rs.
        if rec.borrow().load.request.is_some() {
            self.note_partial_fetch(rec, &row)?;
        } else {
            let mut r = rec.borrow_mut();
            r.load.buffer = Some(al_builtins::value::Buffer {
                have: al_builtins::value::FieldSet::All,
                unsure: al_builtins::value::FieldSet::Of(Vec::new()),
                row: None,
                cleared: false,
            });
            r.load.partial = false;
        }
        let t = self.table(rec)?;
        let pk = t.primary_key(&row);
        // The record remembers the row as its own only when the row passed its filters when it was read; a filter set later forgets it
        // (record/TestFindSetSortKeyModify.al:OwnFilter_ModifiedOutOfIt_FindStillFindsTheRow against misc/lt-probes-needed-3/
        // PnC02Filters.al:FindEqual_OutsideFilter and record/lt-record-read/RcAMisc.al:MarkedOnly_With_Find_Get_And_Order).
        let passes = self.row_passes(rec, &t, &row)?;
        {
            let mut r = rec.borrow_mut();
            r.position = passes.then_some(pk);
            r.fields = row;
        }
        self.auto_calculate(rec)
    }

    /// The row to hand to the store: a database table keeps its own copy of every Blob.
    fn to_store(rec: &Rec, mut row: Vec<Value>) -> Vec<Value> {
        if rec.borrow().temp_id.is_none() {
            for v in &mut row {
                if let Value::Blob(b) = v {
                    *b = al_builtins::stream::copy_blob(b, true);
                }
            }
        }
        row
    }

    /// `CalcFields` of Blob fields: a database record's Blob is fetched from the stored row unless
    /// the buffer already holds it (an in-memory write is kept:
    /// streams/TestBlobCalcFieldsUncommittedWrite.al); a temporary record's Blob is always loaded.
    pub(crate) fn load_blobs(&mut self, rec: &Rec, positions: &[usize]) -> Result<(), Signal> {
        let t = self.table(rec)?;
        let cx = self.cx_for(rec, &t)?;
        let mut stored: Option<Vec<Value>> = None;
        for &pos in positions {
            let Value::Blob(buf) = rec.borrow().fields[pos].clone() else {
                return unsupported("CalcFields of a Blob field that holds no Blob");
            };
            // A temporary record's row that was never inserted has no stored content: CalcFields empties a Blob a stream
            // wrote (P94XpDelim.al:Export_NothingDeclared exports into such a Blob and reads back nothing).
            let temporary = rec.borrow().temporary;
            if buf.borrow().loaded && !buf.borrow().refetch && !temporary {
                continue;
            }
            if stored.is_none() {
                let pk = t.primary_key(&rec.borrow().fields);
                stored = self
                    .store_for(rec)
                    .get(&cx, &t, &pk)
                    .map_err(|e| Signal::Unsupported(format!("{e}")))?;
            }
            // A row that is not stored (a copy of a record never inserted) leaves the Blob empty: the buffer
            // showed `*` after `Copy1 := Row`, and CalcFields reads nothing back
            // (types/ProbeTypeFormsAndFilters.al:FormatRecord_AssignmentCopiesBlobBytes, BC 27.5).
            let bytes = match stored.as_ref().map(|row| row[pos].clone()) {
                Some(Value::Blob(src)) => src.borrow().bytes.clone(),
                _ => Vec::new(),
            };
            let mut b = buf.borrow_mut();
            b.bytes = bytes;
            b.loaded = true;
            b.refetch = false;
        }
        Ok(())
    }

    /// The first use of a record of a `TableType = CRM` table fails while no CRM table connection is
    /// registered, whatever the use (Init, TableCaption, a field, IsEmpty): text and code from
    /// session/TestCrmTableNoConnectionContracts.al:15-17,111-118. With a connection registered what
    /// the table does is not pinned.
    pub(crate) fn connection_check(&self, rec: &Rec) -> Result<(), Signal> {
        // A temporary record of a CRM table is an in-memory table of its own and needs no connection (not
        // pinned by a corpus line; Base Application's `CRM Synch. Helper.ClearCache` relies on it,
        // docs/probes-needed.md).
        if !rec.borrow().crm || rec.borrow().temporary {
            return Ok(());
        }
        if self.table_connections.iter().any(|(kind, _)| *kind == 0) {
            return unsupported("a TableType = CRM table through a registered table connection (not modelled)");
        }
        Err(crate::transactions::al_error_code(
            "Table connection  for table type CRM must be registered using RegisterTableConnection or cmdlet New-NAVTableConnection before it can be used.",
            "Argument",
        ))
    }

    pub(crate) fn record_op(&mut self, method: RecordOp, rec: &Rec, args: &mut [Value]) -> Result<Value, Signal> {
        self.connection_check(rec)?;
        let t = self.table(rec)?;
        if matches!(
            method,
            RecordOp::Find
                | RecordOp::FindFirst
                | RecordOp::FindLast
                | RecordOp::FindSet
                | RecordOp::Next
                | RecordOp::Count
                | RecordOp::IsEmpty
                | RecordOp::CalcSums
        ) {
            let temporary = rec.borrow().temporary;
            self.unseeded_read(&t, temporary, method.name())?;
            self.coverage_read(&t, temporary)?;
        }
        if matches!(
            method,
            RecordOp::Get | RecordOp::GetBySystemId | RecordOp::CalcFields | RecordOp::LoadFields
        ) {
            let temporary = rec.borrow().temporary;
            self.coverage_read(&t, temporary)?;
        }
        let cx = self.cx_for(rec, &t)?;
        // The next access of the table raises a deferred duplicate-key Insert.
        if matches!(
            method,
            RecordOp::Insert
                | RecordOp::Modify
                | RecordOp::Delete
                | RecordOp::DeleteAll
                | RecordOp::ModifyAll
                | RecordOp::Rename
                | RecordOp::Truncate
                | RecordOp::Get
                | RecordOp::LoadFields
                | RecordOp::Find
                | RecordOp::FindFirst
                | RecordOp::FindLast
                | RecordOp::FindSet
                | RecordOp::Next
                | RecordOp::Count
                | RecordOp::IsEmpty
                | RecordOp::CalcFields
                | RecordOp::CalcSums
        ) && !rec.borrow().temporary
            && self
                .tx
                .deferred_insert
                .as_ref()
                .is_some_and(|(o, _)| *o == t.object as usize)
        {
            let (_, e) = self.tx.deferred_insert.take().expect("checked");
            return Err(Signal::Error(Box::new(e)));
        }
        if let Some(schema) = self.engine.program.objects[t.object as usize]
            .table
            .as_ref()
            .filter(|_| self.engine.table_facts[t.object as usize].unresolved_init)
        {
            if let Some(f) = schema.fields.iter().find(|f| f.init_text.is_some() && f.init.is_none()) {
                // An InitValue BC accepts at compile time and cannot evaluate when it builds the
                // buffer: Init() on a written record raises, on an untouched one does nothing
                // (record/TestDateTimeInitValue.Codeunit.al:29-77). Nothing else is pinned.
                let messages: std::collections::BTreeSet<&String> = (0..schema.fields.len())
                    .filter_map(|pos| self.engine.unevaluable_init.get(&(t.object, pos)))
                    .collect();
                let only_unevaluable = schema
                    .fields
                    .iter()
                    .enumerate()
                    .filter(|(_, f)| f.init_text.is_some() && f.init.is_none())
                    .all(|(pos, _)| self.engine.unevaluable_init.contains_key(&(t.object, pos)));
                if method == RecordOp::Init && only_unevaluable && messages.len() == 1 {
                    if !rec.borrow().touched {
                        return Ok(Value::Null);
                    }
                    return Err(Signal::Error(Box::new(AlError {
                        message: (*messages.iter().next().expect("one message")).clone(),
                        code: String::new(),
                        call_stack: String::new(),
                    })));
                }
                return unsupported(format!(
                    "{}.{}: InitValue `{}` is not implemented yet",
                    t.name,
                    f.name,
                    f.init_text.as_deref().unwrap_or("")
                ));
            }
        }
        // OnInstallAppPerDatabase runs with CompanyName() empty; a per-company table fails there
        // with "You must choose a company before you can access the ..." (measured on BC 27.5,
        // docs/probes-needed.md; the rest of the text is not known, so this is refused).
        if self.cx.company.is_empty()
            && t.per_company
            && !rec.borrow().temporary
            && matches!(
                method,
                RecordOp::Insert
                    | RecordOp::Modify
                    | RecordOp::Delete
                    | RecordOp::DeleteAll
                    | RecordOp::ModifyAll
                    | RecordOp::Rename
                    | RecordOp::Truncate
                    | RecordOp::Get
                    | RecordOp::Find
                    | RecordOp::FindFirst
                    | RecordOp::FindLast
                    | RecordOp::FindSet
                    | RecordOp::Next
                    | RecordOp::Count
                    | RecordOp::IsEmpty
                    | RecordOp::CalcFields
                    | RecordOp::CalcSums
            )
        {
            return unsupported(format!(
                "{method} on the per-company table {} with no company chosen (BC: \"You must choose a company before you can access the ...\")",
                t.name
            ));
        }
        if matches!(
            method,
            RecordOp::Insert
                | RecordOp::Modify
                | RecordOp::Delete
                | RecordOp::DeleteAll
                | RecordOp::ModifyAll
                | RecordOp::Rename
                | RecordOp::Truncate
        ) && !rec.borrow().temporary
        {
            self.bg_refuse_write(t.object as usize, method.name())?;
            self.gate_write(method)?;
            self.user_bulk_refusal(&t, rec.borrow().temporary, method)?;
            // A write call starts the transaction, also a DeleteAll that finds no row
            // (session/ProbeSystemEnumMembers.al:143-161, session/TestDatabaseExtended.al:
            // Database_CurrentTransactionType_SetAfterWriteOp_Throws over fixture DeleteAlls).
            self.tx.tx_phase = crate::transactions::TX_WRITE;
        } else if self.tx.tx_phase == crate::transactions::TX_FRESH
            && !rec.borrow().temporary
            && matches!(
                method,
                RecordOp::Get
                    | RecordOp::GetBySystemId
                    | RecordOp::Find
                    | RecordOp::FindFirst
                    | RecordOp::FindLast
                    | RecordOp::FindSet
                    | RecordOp::Next
                    | RecordOp::Count
                    | RecordOp::IsEmpty
                    | RecordOp::CalcFields
                    | RecordOp::CalcSums
            )
        {
            // A read call starts it too (session/ProbeSystemEnumMembers.al:94-112).
            self.tx.tx_phase = crate::transactions::TX_READ;
        }
        match method {
            RecordOp::Init => {
                let fresh = self.initial_fields(t.object as usize);
                let mut r = rec.borrow_mut();
                for (i, v) in fresh.into_iter().enumerate() {
                    // Doc (Record.Init): primary key fields keep their values.
                    if !t.keys[0].contains(&i) {
                        r.fields[i] = v;
                    }
                }
                Ok(Value::Null)
            }
            RecordOp::Clear => {
                // Clear resets the buffer, the filters and the keys but does not empty the table of a
                // temporary record: its rows stay and it stays temporary
                // (types/ProbeTypeFormsAndFilters.al:Clear_TemporaryRecordWithRows_ThenCount, BC 27.5).
                let fresh = self.initial_fields(t.object as usize);
                let old_row_id = Some(crate::partial::row_id(&t, &rec.borrow().fields));
                // Clear sets the table's globals back to their initial values (ADR-021 decision 4;
                // corpus probe TableGlobals_Clear_WhatHappensToTheGlobals).
                self.reset_table_globals(rec);
                let mut r = rec.borrow_mut();
                r.fields = fresh;
                r.filters.clear();
                r.filter_group = 0;
                r.marks.clear();
                r.marked_only = false;
                r.key = 0;
                r.adhoc = None;
                r.ascending = true;
                r.field_descending = false;
                r.position = None;
                // A Clear leaves the record's record of what it fetched: the fields in hand stay loaded for a later
                // partial fetch (session/TestProbeSessionMiscContracts.al:PartialRecords_AreFieldsLoaded: a full Get,
                // Clear, SetLoadFields(Cd) and Get still reports Txt loaded).
                let (mut buffer, partial) = (r.load.buffer.take(), r.load.partial);
                if let Some(b) = buffer.as_mut() {
                    if b.row.is_none() {
                        b.row = old_row_id.clone();
                    }
                    b.cleared = true;
                }
                r.load = Default::default();
                r.load.buffer = buffer;
                r.load.partial = partial;
                Ok(Value::Null)
            }
            RecordOp::Insert => {
                // A temporary record has no sequence: the key stays as it is (corpus
                // record/TestTempTableContracts.al:52).
                let auto = if rec.borrow().temporary {
                    None
                } else {
                    self.auto_increment_field(&t)?
                };
                let run = arg_bool(args, 0)?;
                // A duplicate key does not skip the Before event and the trigger: they run, then
                // the write fails and the After event does not (corpus record/TestTableEventSequenceContracts.al:
                // FailingWrites_RaiseTheBeforeEventAndTheTrigger_ThenTheError).
                // Before event, then the trigger, then the write, then the After event
                // (record/TestTriggerDispatchOrder.al:TriggerOrder_Insert_RecordPipeline_IsStable).
                // Rec carries its SystemId from the Before event on, the one that is stored (record/TestXRecStampFields.al:80-90;
                // events/TestProbeEventContracts.al:SystemId_BeforeOnInsert_AndPresetBuffer, `sysid_null=No` in the Before event);
                // the stamps and the key come below it.
                let keep_id = match args.get(1) {
                    Some(v) => ops::truthy(v)?,
                    None => false,
                };
                // xRec in the insert triggers is the buffer as it was before the SystemId is given: the SystemId an
                // earlier insert left in the variable, else the null Guid (Insert_Events_TriggerFalseAndTrue).
                let before_id = t
                    .fields
                    .iter()
                    .position(|f| f.id == 2_000_000_000)
                    .map(|i| rec.borrow().fields[i].clone());
                self.assign_system_id(&t, rec, keep_id);
                self.table_event(&t, "onbeforeinsertevent", rec, None, run, None)?;
                let id_slot = t.fields.iter().position(|f| f.id == 2_000_000_000);
                let assigned = id_slot.map(|i| rec.borrow().fields[i].clone());
                // xRec in the insert triggers is the buffer without the SystemId it was just given
                // (record/TestXRecStampFields.al:38-52).
                let insert_xrec = id_slot.map(|i| {
                    let mut copy = rec.borrow().clone();
                    copy.fields[i] = before_id.clone().unwrap_or(Value::Guid([0; 16]));
                    Rc::new(RefCell::new(copy))
                });
                if run {
                    // The extension's triggers bracket the base table's own (tableextensiontrigger/
                    // TestTableExtensionTableTriggers_Tests.al); Insert(false) runs none of them.
                    self.run_ext_trigger(&t, "OnBeforeInsert", rec, insert_xrec.as_ref())?;
                    if let Some(tr) = self.table_trigger(&t, "OnInsert") {
                        self.run_trigger(t.object as usize, tr, rec, insert_xrec.as_ref())?;
                    }
                    self.run_ext_trigger(&t, "OnInsert", rec, insert_xrec.as_ref())?;
                }
                // The database event follows the table's triggers and precedes the write
                // (record/TestTriggerDispatchOrder.al:TriggerOrder_Insert_RecordPipeline_IsStable).
                self.global_trigger(&t, GlobalOp::Insert, rec, None)?;
                // The platform's own User trigger runs below the events and triggers (usertable.rs).
                let user_row = self.is_user_row(&t, rec.borrow().temporary);
                if user_row {
                    self.user_before_insert(&t, rec)?;
                }
                // Only Insert(_, true) takes a SystemId the code put in the buffer; a trigger
                // that sets one on any other insert changes nothing about the stored id.
                if let (false, Some(i), Some(id)) = (keep_id, id_slot, assigned) {
                    rec.borrow_mut().fields[i] = id;
                }
                if keep_id {
                    self.check_system_id_unique(rec, &t)?;
                }
                let old_version = self.bump_row_version(&t, rec);
                let stamped = self.stamp_insert(&t, rec);
                // The key is chosen below the trigger: OnInsert starts with it still 0 and a
                // refused Insert never takes a value (corpus record/TestAutoIncrementExplicitKey.al:63,
                // TestAutoIncrementEdges.al:354).
                let mut taken = false;
                if let Some(a) = auto {
                    let mut row = rec.borrow().fields.clone();
                    taken = self.take_auto_increment(&t, a, &mut row, &cx.company)?;
                    rec.borrow_mut().fields = row;
                }
                let row = rec.borrow().fields.clone();
                // A unique secondary key refuses the row after its AutoIncrement value was taken
                // (corpus record/TestAutoIncrementExplicitKey.al:131,163).
                if let Some(unique_key) = self.unique_violation(rec, &t, &row, None)? {
                    Self::restore_row_version(rec, old_version);
                    Self::unstamp(rec, stamped);
                    if let Some(a) = auto {
                        if taken {
                            rec.borrow_mut().fields[a] = self.default_value(&t.fields[a].ty);
                        } else {
                            self.note_explicit_key(&t, a, &row, &cx.company);
                        }
                    }
                    return self.fail_unique(&t, unique_key, &row);
                }
                let stored = Self::to_store(rec, self.stored_view(&t, &row));
                match self
                    .store_for(rec)
                    .insert(&cx, &t, stored.clone())
                    .map_err(|e| Signal::Unsupported(format!("{e}")))?
                {
                    InsertOutcome::Inserted => {
                        if let (Some(a), false) = (auto, taken) {
                            self.note_explicit_key(&t, a, &row, &cx.company);
                        }
                        if user_row {
                            self.user_after_insert(&t, rec)?;
                        }
                        if run {
                            self.run_ext_trigger(&t, "OnAfterInsert", rec, None)?;
                        }
                        self.table_event(&t, "onafterinsertevent", rec, None, run, None)?;
                        Ok(Value::Boolean(true))
                    }
                    InsertOutcome::AlreadyExists => {
                        Self::restore_row_version(rec, old_version);
                        Self::unstamp(rec, stamped);
                        // The value stays consumed; the key it took does not stay on the record.
                        if let (Some(a), true) = (auto, taken) {
                            rec.borrow_mut().fields[a] = self.default_value(&t.fields[a].ty);
                        }
                        // Without a Blob field the platform defers the failure of an unguarded
                        // Insert to the next access of the table or Commit; a guarded Insert
                        // answers false at once, and a Blob field makes it fail at once (corpus
                        // record/TestRecordRuntimeTextsAndViews.al:Record_Insert_DuplicateKey_*).
                        // A temporary table's behaviour is not pinned: it fails at once.
                        let has_blob = self.engine.program.objects[t.object as usize]
                            .table
                            .as_ref()
                            .is_some_and(|s| {
                                s.fields.iter().any(|f| {
                                    matches!(&f.ty, RtType::Library(al_builtins::library_kind::LibraryKind::Blob))
                                })
                            });
                        // A table with an AutoIncrement field needs the platform's answer at once
                        // (record/TestAutoIncrementContracts.al:AutoIncrement_SecondInsert_...).
                        if !self.ret_used
                            && !has_blob
                            && auto.is_none()
                            && !self.tx.immediate_errors
                            && !rec.borrow().temporary
                        {
                            if (run && !self.ext_triggers(&t, "OnAfterInsert").is_empty())
                                || self.listened(&t, "onafterinsertevent", None)?
                            {
                                return unsupported(format!(
                                    "deferred duplicate-key Insert into {}, which has an After insert event or trigger (whether it runs is not pinned)",
                                    t.name
                                ));
                            }
                            match self.fail(&t, &row, true) {
                                Err(Signal::Error(e)) => {
                                    self.tx.deferred_insert.get_or_insert((t.object as usize, *e));
                                }
                                // A key whose format is not pinned stays a refusal.
                                Err(other) => return Err(other),
                                Ok(_) => {}
                            }
                            return Ok(Value::Boolean(true));
                        }
                        self.fail(&t, &row, true)
                    }
                }
            }
            RecordOp::Modify => {
                let run = arg_bool(args, 0)?;
                let pk = t.primary_key(&rec.borrow().fields);
                let stored = self
                    .store_for(rec)
                    .get(&cx, &t, &pk)
                    .map_err(|e| Signal::Unsupported(format!("{e}")))?;
                // A buffer older than a committed change to its row cannot be saved
                // (record/TestRecordStaleBufferWrite.al); before a commit the last writer wins.
                if let (Some(row), Some(i)) = (&stored, t.fields.iter().position(|f| f.id == 2_000_000_005)) {
                    let (mine, theirs) = (rec.borrow().fields[i].clone(), row[i].clone());
                    let version = |v: &Value| if let Value::BigInteger(n) = v { *n } else { 0 };
                    // Stale means the stored row is NEWER than the buffer. A buffer that is newer than the row (the
                    // transaction was rolled back to the last Commit after the buffer's own Modify, as an asserterror
                    // does) is not stale: Microsoft's SMB Service Item TestItem_Reserve modifies such a buffer
                    // and passes in BC (assumed from that test; docs/probes-needed.md).
                    if !rec.borrow().temporary
                        && version(&mine) > version(&theirs)
                        && version(&theirs) <= self.tx.committed_row_version
                    {
                        // Under the rule "a different version is stale" this write would have been refused.
                        al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::StaleBufferNewerThanRow);
                    }
                    if !rec.borrow().temporary
                        && version(&mine) != 0
                        && version(&mine) < version(&theirs)
                        && version(&theirs) <= self.tx.committed_row_version
                    {
                        let identification = self.identification(&t, &rec.borrow().fields)?;
                        return Err(Signal::Error(Box::new(AlError {
                            message: format!(
                                "The changes to the {} record cannot be saved because some information on the page is not up-to-date. Close the page, reopen it, and try again.{BR}{BR}Identification fields and values:{BR}{BR}{}",
                                self.caption(&t),
                                identification
                            ),
                            code: "DB:RecordChanged".into(),
                            call_stack: String::new(),
                        })));
                    }
                }
                // Modify never changes the stored SystemId, whatever the buffer carries
                // (corpus record/TestRecordSystemId.al:176-215, :285-330).
                let kept: Vec<(usize, Value)> = match &stored {
                    Some(old) => [2_000_000_000, 2_000_000_001, 2_000_000_002]
                        .iter()
                        .filter_map(|id| t.fields.iter().position(|f| f.id == *id))
                        .map(|i| (i, old[i].clone()))
                        .collect(),
                    None => Vec::new(),
                };
                // One xRec handle for the events, the triggers and the global trigger, so a change a
                // subscriber makes to it reaches the rest. When code drives the Modify, xRec equals
                // Rec (record/TestXRecContracts.al:69-87 OnModify_xRec_MirrorsRecValues_WhenCalledFromCode;
                // record/TestTableEventSequenceContracts.al:ModifyRenameDelete_EventsAndTrigger pins the
                // same for the events). A row a page saves carries the page's own previous row
                // (record/TestXRecDriverContracts.al:32-51 Record_Modify_FromPage_xRecHoldsPreviousValue).
                let stored_row = stored.clone();
                let before_image = self
                    .pages
                    .page_xrec
                    .take()
                    .unwrap_or_else(|| rec.borrow().fields.clone());
                let xrec = Some(Self::xrec_handle(rec, before_image));
                self.table_event(&t, "onbeforemodifyevent", rec, xrec.as_ref(), run, None)?;
                if run {
                    self.run_ext_trigger(&t, "OnBeforeModify", rec, xrec.as_ref())?;
                    if let Some(tr) = self.table_trigger(&t, "OnModify") {
                        self.run_trigger(t.object as usize, tr, rec, xrec.as_ref())?;
                    }
                    self.run_ext_trigger(&t, "OnModify", rec, xrec.as_ref())?;
                }
                self.global_trigger(&t, GlobalOp::Modify, rec, xrec.as_ref())?;
                if self.is_user_row(&t, rec.borrow().temporary) {
                    self.user_before_modify(&t, rec)?;
                }
                let old_version = self.bump_row_version(&t, rec);
                let stamped = self.stamp_modify(&t, rec, stored_row.as_deref());
                let mut row = rec.borrow().fields.clone();
                for (i, v) in kept {
                    row[i] = v;
                }
                // A unique secondary key refuses the write after the trigger (corpus
                // record/TestCalcFieldsTruncateUniqueContracts.al:UniqueIndex_Violation_..., Modify).
                if let Some(unique_key) = self.unique_violation(rec, &t, &row, Some(&pk))? {
                    Self::restore_row_version(rec, old_version);
                    Self::unstamp(rec, stamped);
                    return self.fail_unique(&t, unique_key, &row);
                }
                let stored = Self::to_store(rec, self.stored_view(&t, &row));
                match self
                    .store_for(rec)
                    .modify(&cx, &t, stored.clone())
                    .map_err(|e| Signal::Unsupported(format!("{e}")))?
                {
                    WriteOutcome::Done => {
                        // A Blob a Modify of a temporary record persisted does not survive a later Rename
                        // (streams/TestBlobRenameIsolation.al:TempBlob_CommittedWrite_Rename_SameInstanceReGet_LosesBlobValue);
                        // one written after the Insert and never modified does.
                        if rec.borrow().temp_id.is_some() {
                            for v in &rec.borrow().fields {
                                if let Value::Blob(b) = v {
                                    b.borrow_mut().persisted = true;
                                }
                            }
                        }
                        if run {
                            self.run_ext_trigger(&t, "OnAfterModify", rec, xrec.as_ref())?;
                        }
                        self.table_event(&t, "onaftermodifyevent", rec, xrec.as_ref(), run, None)?;
                        Ok(Value::Boolean(true))
                    }
                    WriteOutcome::NotFound => {
                        Self::restore_row_version(rec, old_version);
                        Self::unstamp(rec, stamped);
                        self.fail(&t, &row, false)
                    }
                }
            }
            RecordOp::Delete => {
                let run = arg_bool(args, 0)?;
                self.delete_row(&t, rec, run)
            }
            RecordOp::DeleteAll => {
                let run = arg_bool(args, 0)?;
                // The platform takes no delete on Tenant License State (record/
                // TestPlatformTablesWritableContracts.al:96-101; the single-record Delete is not pinned).
                if self.engine.program.objects[t.object as usize].id == Some(2_000_000_189) {
                    return Err(Signal::Error(Box::new(AlError {
                        message: "It is not possible to delete any record in the Tenant License State table.".into(),
                        code: "DB:Ok".into(),
                        call_stack: String::new(),
                    })));
                }
                let (filters, flow_field_filters) = self.split_filters(rec);
                if !flow_field_filters.is_empty() {
                    return unsupported("DeleteAll with a filter on a FlowField");
                }
                let temporary = rec.borrow().temporary;
                self.coverage_cleared(&t, temporary, &filters);
                let n = self
                    .store_for(rec)
                    .count(&cx, &t, &filters)
                    .map_err(|e| Signal::Unsupported(format!("{e}")))?;
                // No row, no trigger and no event. The platform still asks for the table's Global
                // Triggers setup, as the first write to the table (session/TestEnvironmentUserTenantContracts.al:289-290,
                // `GtTable.DeleteAll()` on an empty table). A temporary record is not pinned and stays as it was.
                if n == 0 {
                    if !temporary {
                        self.global_setup_asked(&t)?;
                    }
                    return Ok(Value::Null);
                }
                let events =
                    self.listened(&t, "onbeforedeleteevent", None)? || self.listened(&t, "onafterdeleteevent", None)?;
                let mut trigger = self.table_trigger(&t, "OnDelete").is_some();
                for n in ["OnBeforeDelete", "OnDelete", "OnAfterDelete"] {
                    trigger |= !self.ext_triggers(&t, n).is_empty();
                }
                let global = !temporary && self.global_enabled(&t, GlobalOp::Delete)?;
                // With delete events or database triggers a DeleteAll deletes row by row: the events
                // and the database trigger per row, the table's and extensions' triggers only for
                // DeleteAll(true). Pinned on real BC 27.5 by the probe codeunit 68212
                // (record/TestTableExtensionTriggerOrderContracts.al:102
                // Delete_And_DeleteAll_WithAndWithoutRunTrigger).
                // The User table has no bulk delete: the platform deletes it row by row.
                let user_rows = self.is_user_row(&t, rec.borrow().temporary);
                if events || global || user_rows || (run && trigger) {
                    // One Delete per row (record/TestTriggerContracts.al:DeleteAll_WithTrigger_FiresForEachFilteredRecord).
                    let key = rec.borrow().key;
                    let mut rows = Vec::new();
                    let mut after = None;
                    while let Some(row) = self.scan(rec, &t, true, after.take())? {
                        after = Some(position(&t, key, &row));
                        rows.push(row);
                    }
                    // One fresh set of table globals for all rows, not the caller's
                    // (ADR-021 decision 6; corpus probe TableGlobals_DeleteAll_TriggersRunOnOneInstanceForAllRows).
                    let shared = al_builtins::value::TableGlobals::new_shared();
                    for row in rows {
                        let mut st = rec.borrow().clone();
                        st.fields = row;
                        st.filters.clear();
                        st.globals = shared.share();
                        self.delete_row(&t, &Rc::new(RefCell::new(st)), run)?;
                    }
                    return Ok(Value::Null);
                }
                if self.is_company_table(&t) && !temporary {
                    return unsupported(
                        "DeleteAll on the Company table without delete events (a deleted company's rows go with it; not pinned)",
                    );
                }
                self.store_for(rec)
                    .delete_all(&cx, &t, &filters)
                    .map_err(|e| Signal::Unsupported(format!("{e}")))?;
                Ok(Value::Null)
            }
            RecordOp::Get => {
                // `Get(RecordId)`: the row the RecordId names, as RecordRef.Get(RecordId) does (corpus
                // recordref/TestRecordRefKeys.al:48). The Record form is pinned for a row that exists
                // (record/TestSetAutoCalcFieldsOnGet.Codeunit.al:68-79; Base Application's Item
                // Category Management does `ItemCategory.Get(RecId)`); the RecordId of another table is refused.
                let from_id: Option<Vec<Value>> = match args {
                    [Value::Library(_, l)] => match &*l.borrow() {
                        al_builtins::value::LibraryValue::RecordId { table, key, .. } if *table == t.object => {
                            Some(key.clone())
                        }
                        al_builtins::value::LibraryValue::RecordId { .. } => {
                            return unsupported("Get of a RecordId of another table (error text not pinned)");
                        }
                        _ => None,
                    },
                    _ => None,
                };
                let by_record_id = from_id.is_some();
                let args = from_id.as_deref().unwrap_or(args);
                let mut pk = Vec::new();
                for (n, &field) in t.keys[0].iter().enumerate() {
                    let ty = t.fields[field].ty.clone();
                    pk.push(match args.get(n) {
                        Some(v) => ops::convert(v.clone(), &ty)?,
                        None => self.default_value(&ty),
                    });
                }
                match self
                    .store_for(rec)
                    .get(&cx, &t, &pk)
                    .map_err(|e| Signal::Unsupported(format!("{e}")))?
                {
                    Some(row) => {
                        self.load(rec, row)?;
                        Ok(Value::Boolean(true))
                    }
                    None => {
                        // A row that is there is pinned (record/TestSetAutoCalcFieldsOnGet.Codeunit.al:68-79);
                        // the not-found outcome of the RecordId form is not.
                        if by_record_id {
                            al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::RecordGetRecordId);
                        }
                        let mut ident = rec.borrow().fields.clone();
                        for (n, &field) in t.keys[0].iter().enumerate() {
                            ident[field] = match (args.get(n), &t.fields[field].ty) {
                                // The message names the key as the caller gave it: an Option for an Enum key
                                // reads 'Three', not the ordinal the enum lacks
                                // (corpus types/PEOptionEnumBigText.al:225-259, probed on BC 27.5).
                                (Some(o @ Value::Option(..)), RtType::Enum(_)) => o.clone(),
                                // Whether the message names an enum of another type, or an Integer, by the
                                // caller's value or by the key's is not pinned (docs/probes-needed.md); found
                                // and not found are (types/PXEnumRecordRef.al:123,127,154,156).
                                (Some(Value::Enum(from, _)), RtType::Enum(to)) if from != to && !self.ret_used => {
                                    return Err(Signal::Unsupported(
                                        "the not-found error of a Get keyed by an enum of another type: the identification text is not pinned (docs/probes-needed.md)".into(),
                                    ));
                                }
                                (Some(Value::Integer(_)), RtType::Enum(_)) if !self.ret_used => {
                                    return Err(Signal::Unsupported(
                                        "the not-found error of a Get keyed by an Integer on an Enum field: the identification text is not pinned (docs/probes-needed.md)".into(),
                                    ));
                                }
                                _ => pk[n].clone(),
                            };
                        }
                        self.fail(&t, &ident, false)
                    }
                }
            }
            RecordOp::FindFirst | RecordOp::FindSet | RecordOp::FindLast | RecordOp::Find => {
                let asc = {
                    let r = rec.borrow();
                    r.ascending
                };
                let which = match (method, args.first()) {
                    (RecordOp::FindLast, _) => "+".to_string(),
                    (RecordOp::Find, Some(v)) => al_builtins::native::text_of(v).unwrap_or_default().to_string(),
                    // Doc (Record.Find): Which defaults to '='.
                    (RecordOp::Find, None) => "=".to_string(),
                    _ => "-".to_string(),
                };
                // Only `-` or `+` alone, or one to three of `=`, `<` and `>` with none repeated, are directions; anything
                // else, the empty text included, raises (record/PZFindDirections.al:Find_InvalidDirections and
                // Find_InvalidDirection_ErrorText, Record and RecordRef alike; the text carries the backslashes as BC
                // writes them).
                let valid = match which.as_str() {
                    "-" | "+" => true,
                    w => {
                        !w.is_empty()
                            && w.chars().all(|c| matches!(c, '=' | '<' | '>'))
                            && w.char_indices().all(|(i, c)| !w[i + 1..].contains(c))
                    }
                };
                if !valid {
                    return Err(Signal::Error(Box::new(AlError {
                        message: format!(
                            "There is an invalid character in the Find expression '{which}'.\\\\In Find expressions, you can use =, >, and < in combinations, where each sign can only appear once, or you can use - or +."
                        ),
                        code: "DB:BadSearchMethod".into(),
                        call_stack: String::new(),
                    })));
                }
                let found = match which.as_str() {
                    "-" => self.scan(rec, &t, asc, None)?,
                    "+" => self.scan(rec, &t, !asc, None)?,
                    // Every form honours the active filters
                    // (record/TestFilterTextBooleanFindContracts.al:143, Find_AllForms_HonourTheActiveFilters).
                    // The one exception: a record that moved its own row out of its filter still
                    // finds the row by its key, while a second variable with the old filter does
                    // not (record/TestFindSetSortKeyModify.al:OwnFilter_ModifiedOutOfIt_FindStillFindsTheRow
                    // against OwnFilter_SecondVarWithTheSameFilter_FindDoesNotFindTheRow). A record
                    // remembers the key of the row it last read as its position.
                    "=" => {
                        let pk = t.primary_key(&rec.borrow().fields);
                        let row = self
                            .store_for(rec)
                            .get(&cx, &t, &pk)
                            .map_err(|e| Signal::Unsupported(format!("{e}")))?;
                        match row {
                            Some(row) if self.row_passes(rec, &t, &row)? || Self::on_own_row(rec, &pk) => Some(row),
                            _ => None,
                        }
                    }
                    // `>` and `<`: the first row past the buffer's position in the current key.
                    ">" | "<" => {
                        let key = rec.borrow().key;
                        let pos = position(&t, key, &rec.borrow().fields);
                        self.scan(rec, &t, (which == ">") == asc, Some(pos))?
                    }
                    // Two or three direction characters are tried in the order written, and the first that finds a row
                    // wins: `=` the row itself, `<` the row before it, `>` the row after it. So `<=` answers the
                    // previous row even when the key exists, and `=<>` takes the previous row before the next one
                    // (types/PYLoadFindThis.al:Find_DirectionCharacters_Orders, all eleven orders on BC 27.5 and 29.0;
                    // the neighbour search of a row that is gone: pageupdate/TestPageUpdateGoneRow.al:423,
                    // events/TestProbeEventContracts.al:Find_NeighbourSearch_*).
                    w if w.len() >= 2 && w.chars().all(|c| matches!(c, '<' | '=' | '>')) => {
                        let key = rec.borrow().key;
                        let pos = position(&t, key, &rec.borrow().fields);
                        let mut found = None;
                        for c in w.chars() {
                            found = match c {
                                // Another current key looks for the neighbour by that key's fields
                                // (events/TestProbeEventContracts.al:Find_NeighbourSearch_WithFilterAndOtherKey, key Name).
                                '=' if key != 0 => None,
                                '=' => {
                                    let pk = t.primary_key(&rec.borrow().fields);
                                    let exact = self
                                        .store_for(rec)
                                        .get(&cx, &t, &pk)
                                        .map_err(|e| Signal::Unsupported(format!("{e}")))?;
                                    match exact {
                                        Some(row) if self.row_passes(rec, &t, &row)? => Some(row),
                                        _ => None,
                                    }
                                }
                                '<' => self.scan(rec, &t, !asc, Some(pos.clone()))?,
                                _ => self.scan(rec, &t, asc, Some(pos.clone()))?,
                            };
                            if found.is_some() {
                                break;
                            }
                        }
                        found
                    }
                    other => return unsupported(format!("Find('{other}')")),
                };
                match found {
                    Some(row) => {
                        self.load(rec, row)?;
                        Ok(Value::Boolean(true))
                    }
                    None if self.ret_used => Ok(Value::Boolean(false)),
                    // The error BC raises for an empty set has no corpus test (no `asserterror` on a Find call):
                    // its text is not pinned (docs/probes-needed.md).
                    // events/TestProbeEventContracts.al:RecordErrors_GetMissing_TestField_Modify (FindFirst, FindLast, FindSet
                    // with a filter): the text lists the filters; without one it is not pinned.
                    None => {
                        let filters = self.get_filters(rec, &t)?;
                        if filters.is_empty()
                            || !matches!(method, RecordOp::FindFirst | RecordOp::FindLast | RecordOp::FindSet)
                        {
                            return unsupported(format!(
                                "{method} on an empty set of table '{}' without using the result and without a filter: BC's error text is not pinned (docs/probes-needed.md)",
                                t.name
                            ));
                        }
                        Err(Signal::Error(Box::new(AlError {
                            message: format!(
                                "There is no {} within the filter.{BR}{BR}Filters: {filters}",
                                self.caption(&t)
                            ),
                            code: "DB:NothingInsideFilter".into(),
                            call_stack: String::new(),
                        })))
                    }
                }
            }
            RecordOp::Next => {
                let steps = match args.first() {
                    Some(Value::Integer(n)) => *n,
                    // A whole-number Decimal (`Calendar.Next(12 / CountFrequency)`); a fraction would round or
                    // truncate, which is not pinned.
                    Some(Value::Decimal(d)) if d.fract().is_zero() && i32::try_from(d.trunc()).is_ok() => {
                        al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::NextWholeDecimal);
                        i32::try_from(d.trunc()).unwrap_or(0)
                    }
                    None => 1,
                    other => return unsupported(format!("Next({})", other.describe())),
                };
                let (key, asc) = {
                    let r = rec.borrow();
                    (r.key, r.ascending)
                };
                let forward = (steps >= 0) == asc;
                let mut moved = 0;
                for _ in 0..steps.unsigned_abs().max(if steps == 0 { 0 } else { 1 }) {
                    let pos = position(&t, key, &rec.borrow().fields);
                    match self.scan(rec, &t, forward, Some(pos))? {
                        Some(row) => {
                            self.load(rec, row)?;
                            moved += 1;
                        }
                        None => break,
                    }
                }
                Ok(Value::Integer(if steps < 0 { -moved } else { moved }))
            }
            RecordOp::Count | RecordOp::IsEmpty => {
                let (filters, flow_field_filters) = self.split_filters(rec);
                let n = if flow_field_filters.is_empty() && !rec.borrow().marked_only {
                    self.store_for(rec)
                        .count(&cx, &t, &filters)
                        .map_err(|e| Signal::Unsupported(format!("{e}")))?
                } else {
                    // Rows are tested one by one against the calculated FlowFields.
                    let mut n = 0;
                    let mut after = None;
                    while let Some(row) = self.scan(rec, &t, true, after)? {
                        n += 1;
                        if method == RecordOp::IsEmpty {
                            break;
                        }
                        after = Some(position(&t, rec.borrow().key, &row));
                    }
                    n
                };
                Ok(if method == RecordOp::Count {
                    Value::Integer(n as i32)
                } else {
                    Value::Boolean(n == 0)
                })
            }
            RecordOp::SetRange => {
                let pos = arg_pos(args, 0)?;
                let ty = t.fields[pos].ty.clone();
                let mut r = rec.borrow_mut();
                r.position = None;
                let group = r.filter_group;
                r.filters.retain(|f| !(f.field == pos && f.group == group));
                if let Some(from) = args.get(1) {
                    let from = ops::convert(from.clone(), &ty)?;
                    let to = match args.get(2) {
                        Some(v) => ops::convert(v.clone(), &ty)?,
                        None => from.clone(),
                    };
                    r.filters.push(FieldFilter {
                        field: pos,
                        expr: FilterExpr::Range(Some(from), Some(to)),
                        group,
                    });
                }
                Ok(Value::Null)
            }
            RecordOp::SetFilter => {
                let pos = arg_pos(args, 0)?;
                rec.borrow_mut().position = None;
                let ty = t.fields[pos].ty.clone();
                let text = al_builtins::native::text_of(args.get(1).unwrap_or(&Value::Null)).unwrap_or_default();
                // An empty filter text removes the field's filter: a Query column does so
                // (query/TestQueryRuntimeContracts.al:208-209, "an empty SetFilter clears the filter"), and the
                // documentation says the same of a Record. Base Application's BOM Structure does
                // `Item.SetFilter("No.", ItemFilter)` with an empty `ItemFilter`. The Record form is an assumption
                // (docs/probes-needed.md).
                if text.trim().is_empty() {
                    al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::RecordSetFilterEmpty);
                    let mut r = rec.borrow_mut();
                    let group = r.filter_group;
                    r.filters.retain(|f| !(f.field == pos && f.group == group));
                    return Ok(Value::Null);
                }
                let engine = self.engine.clone();
                let enums = |id: u32| engine.program.objects[id as usize].enum_values.clone();
        let captions = |id: u32| engine.program.objects[id as usize].enum_captions.clone();
                let expr = match al_builtins::filter::parse_filter_with_captions(&text, &args[2..], &ty, &enums, &captions) {
                    Ok(e) => e,
                    // record/TestFilterTextBooleanFindContracts.al:130 (SetFilter_BooleanField_UnknownWord_Throws).
                    Err(e) if e.starts_with(al_builtins::filter::INVALID_BOOLEAN) => {
                        let word = &e[al_builtins::filter::INVALID_BOOLEAN.len()..];
                        return Err(Signal::Error(Box::new(AlError {
                            message: format!(
                                "The filter \"{}\" is not valid for the {} field on the {} table. The value \"{word}\" can't be evaluated into type Boolean.",
                                text.trim(),
                                self.field_caption(&t, pos),
                                self.caption(&t)
                            ),
                            code: "Filter:CSide".into(),
                            call_stack: String::new(),
                        })));
                    }
                    Err(e) => return Err(Signal::Unsupported(format!("SetFilter: {e}"))),
                };
                let mut r = rec.borrow_mut();
                let group = r.filter_group;
                r.filters.retain(|f| !(f.field == pos && f.group == group));
                r.filters.push(FieldFilter {
                    field: pos,
                    expr,
                    group,
                });
                Ok(Value::Null)
            }
            RecordOp::Reset => {
                // Reset also sets the table's globals back to their initial values (ADR-021
                // decision 4; corpus probe TableGlobals_Reset_WhatHappensToTheGlobals).
                self.reset_table_globals(rec);
                // SetAutoCalcFields survives Reset and Clear (corpus record/TestCalcFieldsTruncateUniqueContracts.al:
                // SetAutoCalcFields_SurvivesResetAndClear).
                let mut r = rec.borrow_mut();
                r.filters.clear();
                // The filter group goes back to 0 (misc/lt-probes-needed-5/PnE08RecordMisc.al:RecordRef_FilterGroup `afterReset`).
                r.filter_group = 0;
                // Reset drops the marks too (record/lt-record-read/RcAMisc.al:ClearMarks_And_Reset `markAfterReset`).
                r.marks.clear();
                r.marked_only = false;
                r.key = 0;
                r.adhoc = None;
                r.ascending = true;
                r.field_descending = false;
                Ok(Value::Null)
            }
            RecordOp::SetCurrentKey => {
                let wanted: Vec<usize> = (0..args.len()).map(|i| arg_pos(args, i)).collect::<Result<_, _>>()?;
                // Only the table's own keys: `t` may end with this record's earlier ad hoc key, and a
                // prefix of it is a new ad hoc key, not that one.
                let declared = self.engine.tables[t.object as usize]
                    .as_ref()
                    .map_or(t.keys.len(), |e| e.keys.len());
                match t.keys[..declared]
                    .iter()
                    .position(|k| k.len() >= wanted.len() && k[..wanted.len()] == wanted[..])
                {
                    Some(k) => {
                        let mut r = rec.borrow_mut();
                        r.key = k;
                        r.adhoc = None;
                        Ok(Value::Boolean(true))
                    }
                    // Fields no key starts with are allowed and order the rows by themselves, then
                    // by the primary key (record/TestRecordNavContracts.al:180-195,
                    // xmlport/TestXmlPortTriggersReplaceViewContracts.al:339-375).
                    None => {
                        let schema = self.engine.program.objects[t.object as usize]
                            .table
                            .as_ref()
                            .expect("table");
                        // A FlowFilter field and a Blob cannot order rows: `Sorting cannot be done based on the
                        // <field> field(s) in the <table> table.` (code DB:Ok; records/W4SortKeys.al:SetCurrentKey_NotStoredFields
                        // and record/ProbeClearAllSetKeyHyperlink.al:276-318, BC 27.5 and 29.0). A FlowField and a Media
                        // field are accepted and leave the current key the primary key (`CurrentKey()` answers `[Id]`).
                        let is_blob = |p: usize| {
                            matches!(
                                &t.fields[p].ty,
                                RtType::Library(al_builtins::library_kind::LibraryKind::Blob)
                            )
                        };
                        if let Some(&bad) = wanted.iter().find(|&&p| {
                            is_blob(p) || matches!(t.fields[p].ty, RtType::Variant) || schema.fields[p].flow_filter
                        }) {
                            return Err(crate::transactions::al_error_code(
                                &format!(
                                    "Sorting cannot be done based on the {} field(s) in the {} table.",
                                    t.fields[bad].name, t.name
                                ),
                                "DB:Ok",
                            ));
                        }
                        let unordered = |p: usize| {
                            schema.fields[p].calc.is_some()
                                || matches!(
                                    &t.fields[p].ty,
                                    RtType::Library(
                                        al_builtins::library_kind::LibraryKind::Media
                                            | al_builtins::library_kind::LibraryKind::MediaSet
                                    )
                                )
                        };
                        if wanted.iter().all(|&p| unordered(p)) {
                            let mut r = rec.borrow_mut();
                            r.key = 0;
                            r.adhoc = None;
                            return Ok(Value::Boolean(true));
                        }
                        if let Some(&bad) = wanted.iter().find(|&&p| unordered(p) || !t.fields[p].stored) {
                            return unsupported(format!(
                                "SetCurrentKey on {} together with other fields: whether the key stays the primary key is not pinned",
                                t.fields[bad].name
                            ));
                        }
                        // `t` already holds this record's earlier ad hoc key, if any.
                        let index = self.engine.tables[t.object as usize]
                            .as_ref()
                            .map_or(0, |e| e.keys.len());
                        let mut r = rec.borrow_mut();
                        r.key = index;
                        r.adhoc = Some((index, wanted));
                        Ok(Value::Boolean(true))
                    }
                }
            }
            RecordOp::Validate => {
                let pos = arg_pos(args, 0)?;
                let engine = self.engine.clone();
                let schema = engine.program.objects[t.object as usize].table.as_ref().expect("table");
                let field = &schema.fields[pos];
                // A Validate nested in an OnValidate of the same record keeps the outer call's
                // before-image as xRec (corpus record/TestXRecContracts.al:133).
                // `handle` is the address of `rec`, which this call borrows for as long as the entry exists, and the
                // entry is popped below on every way out (the result is held, not `?`-ed, until then), so no other
                // record can get the address while it is on the stack (tests/store/xrec.rs).
                let handle = Rc::as_ptr(rec) as usize;
                let outer = self
                    .validating
                    .iter()
                    .find(|(h, _)| *h == handle)
                    .map(|(_, b)| b.clone());
                let before = outer.clone().unwrap_or_else(|| rec.borrow().fields.clone());
                let old_value = rec.borrow().fields[pos].clone();
                let snapshot = rec.borrow().fields.clone();
                let new_value = match args.get(1) {
                    Some(v) => Some(ops::convert(v.clone(), &field.ty)?),
                    None => None,
                };
                // The relation is checked before the value goes in and before OnValidate runs; a
                // refused value is never stored (corpus record/TestCalcFormulaSystemFieldsTests.Codeunit.al:253).
                self.validate_relation(rec, &t, pos, new_value.clone().unwrap_or_else(|| old_value.clone()))?;
                if let Some(v) = new_value {
                    rec.borrow_mut().fields[pos] = v;
                }
                let xrec = Self::xrec_handle(rec, before.clone());
                if outer.is_none() {
                    self.validating.push((handle, before));
                }
                let r = self.validate_hooks(&t, rec, &xrec, field, snapshot);
                if outer.is_none() {
                    self.validating.pop();
                }
                r?;
                Ok(Value::Null)
            }
            RecordOp::Rename => {
                // Fields that relate to this table follow the rename; a temporary record has none.
                let refs = if rec.borrow().temporary {
                    Vec::new()
                } else {
                    self.rename_references(&t)?
                };
                let old = rec.borrow().fields.clone();
                let mut new = old.clone();
                // A temporary record's Blob reads empty through a stream right after a Rename, whether the write was
                // persisted or not (streams/TestProbeStreamContracts.al:TempRename_UnpersistedBlobWrite,
                // TempRename_PersistedBlobWrite, TempRename_BlobInsertedWithValueThenRename), while the bytes stay in the
                // row: a Get and CalcFields read them back (streams/TestBlobRenameIsolation.al:
                // TempBlob_UncommittedWrite_Rename_SameInstanceReGet_KeepsWrite).
                if rec.borrow().temp_id.is_some() {
                    for (i, v) in new.iter_mut().enumerate() {
                        if let Value::Blob(b) = v {
                            let kept = al_builtins::stream::copy_blob(b, false);
                            *b = match self.default_value(&t.fields[i].ty) {
                                Value::Blob(empty) if b.borrow().persisted => empty,
                                _ => kept,
                            };
                        }
                    }
                }
                for (n, &field) in t.keys[0].iter().enumerate() {
                    let ty = t.fields[field].ty.clone();
                    if let Some(v) = args.get(n) {
                        new[field] = ops::convert(v.clone(), &ty)?;
                    }
                }
                self.check_rename_auto_increment(&t, &old, &new)?;
                if self.is_profile_row(&t, rec.borrow().temporary) {
                    self.profile_before_rename(&t, &old, &new)?;
                }
                let old_pk = t.primary_key(&old);
                // Rec carries the new key and xRec the old row in the events and the trigger
                // (record/TestTableEventDispatch.al:TableEvent_OnAfterRename_Subscriber_FiresWithOldAndNewKeys).
                let old_fields = old.clone();
                rec.borrow_mut().fields = new.clone();
                let xrec = Self::xrec_handle(rec, old.clone());
                // Rename always runs the trigger; RunTrigger is true (docs/probes-needed.md).
                self.table_event(&t, "onbeforerenameevent", rec, Some(&xrec), true, None)?;
                self.run_ext_trigger(&t, "OnBeforeRename", rec, Some(&xrec))?;
                if let Some(tr) = self.table_trigger(&t, "OnRename") {
                    self.run_trigger(t.object as usize, tr, rec, Some(&xrec))?;
                }
                self.run_ext_trigger(&t, "OnRename", rec, Some(&xrec))?;
                self.global_trigger(&t, GlobalOp::Rename, rec, Some(&xrec))?;
                let _ = self.bump_row_version(&t, rec);
                let _ = self.stamp_rename(&t, rec);
                let row = rec.borrow().fields.clone();
                if let Some(unique_key) = self.unique_violation(rec, &t, &row, Some(&old_pk))? {
                    rec.borrow_mut().fields = old.clone();
                    return self.fail_unique(&t, unique_key, &row);
                }
                let stored = Self::to_store(rec, self.stored_view(&t, &row));
                // A rename to the key the row has succeeds and writes the row again, which is what
                // stamps it (record/TestRecordSystemId.al:752-775).
                let same_key = ops::compare_keys(&old_pk, &t.primary_key(&row));
                let outcome = if same_key {
                    match self
                        .store_for(rec)
                        .modify(&cx, &t, stored.clone())
                        .map_err(|e| Signal::Unsupported(format!("{e}")))?
                    {
                        WriteOutcome::Done => RenameOutcome::Renamed,
                        WriteOutcome::NotFound => RenameOutcome::NotFound,
                    }
                } else {
                    self.store_for(rec)
                        .rename(&cx, &t, &old_pk, stored.clone())
                        .map_err(|e| Signal::Unsupported(format!("{e}")))?
                };
                match outcome {
                    RenameOutcome::Renamed => {
                        // A company that is renamed takes its data with it
                        // (record/TestChangeCompanyRows... TestChangeCompanyFiltersCopyDeleteContracts.al:170-212).
                        if !same_key && self.is_company_table(&t) && !rec.borrow().temporary {
                            let (from, to) = (
                                al_builtins::native::text_of(&old_fields[0]).unwrap_or_default(),
                                al_builtins::native::text_of(&row[0]).unwrap_or_default(),
                            );
                            self.store.rename_company(&from, &to);
                        }
                        if !same_key {
                            self.propagate_rename(rec, &t, &refs, &old_fields, &row)?;
                            self.move_record_links(&t, rec, &old_fields)?;
                        }
                        self.run_ext_trigger(&t, "OnAfterRename", rec, Some(&xrec))?;
                        self.table_event(&t, "onafterrenameevent", rec, Some(&xrec), true, None)?;
                        Ok(Value::Boolean(true))
                    }
                    RenameOutcome::NotFound => {
                        rec.borrow_mut().fields = old.clone();
                        self.fail(&t, &old, false)
                    }
                    RenameOutcome::AlreadyExists => {
                        rec.borrow_mut().fields = old;
                        self.fail(&t, &row, true)
                    }
                }
            }
            RecordOp::Truncate => self.truncate(&t, rec, arg_bool(args, 0)?),
            RecordOp::CalcFields | RecordOp::CalcSums | RecordOp::SetAutoCalcFields => self.flow_op(method, rec, args),
            RecordOp::LoadFields => self.load_fields_row(rec, &t, args),
            RecordOp::SetLoadFields
            | RecordOp::AddLoadFields
            | RecordOp::AreFieldsLoaded
            | RecordOp::SetBaseLoadFields => self.load_fields_op(method, rec, &t, args),
            other => self.record_meta_op(other, rec, &t, args),
        }
    }
}

#[allow(dead_code)]
fn _uses(_: RtType, a: &[Value], b: &[Value]) -> std::cmp::Ordering {
    key_cmp(a, b)
}

/// The error BC raises when it evaluates the `InitValue` of `f`, for the two forms the corpus pins as
/// unevaluable (record/TestDateTimeInitValue.Codeunit.al): `0DT` on a DateTime, and a closing-date
/// literal (`C20260101D`) on a Date. Every other form is `None`.
pub(crate) fn unevaluable_init_value(f: &al_ir::Field) -> Option<String> {
    use al_builtins::value::RtType;
    let text = f.init_text.as_deref()?.trim();
    if f.init.is_some() {
        return None;
    }
    match &f.ty {
        RtType::DateTime if text.eq_ignore_ascii_case("0DT") => {
            Some("The value \"01/01/0001 00:00:00\" can't be evaluated into type DateTime.".into())
        }
        RtType::Date => {
            let inner = text.strip_prefix(['C', 'c'])?.strip_suffix(['D', 'd'])?;
            (inner.len() == 8 && inner.chars().all(|c| c.is_ascii_digit()))
                .then(|| format!("The format of the Date '{text}' does not match your device's date settings."))
        }
        _ => None,
    }
}
