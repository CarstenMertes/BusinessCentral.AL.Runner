//! Record operations on the store (ADR-003) for the `Record.*` builtins.
//!
//! Pinned by the corpus or the docs as noted; behaviour neither pins yet
//! raises `Signal::Unsupported` instead of guessing.

use crate::Session;
use crate::globaltriggers::GlobalOp;
use al_builtins::filter::{FieldFilter, FilterExpr, key_cmp};
use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::value::{Instance, RecordState, RtType, Value};
use al_data::{InsertOutcome, RenameOutcome, Scan, Store, TableDef, WriteOutcome, memory::MemStore, position};
use std::cell::RefCell;
use std::rc::Rc;

pub(crate) type Rec = Rc<RefCell<RecordState>>;

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
        other => unsupported(format!("field argument {other:?}")),
    }
}

impl Session {
    pub(crate) fn table(&self, rec: &Rec) -> Result<TableDef, Signal> {
        let t = rec.borrow().table as usize;
        self.engine.tables[t]
            .clone()
            .map_or_else(|| unsupported("record of a table without a schema"), Ok)
    }

    /// Field values of a fresh record: `InitValue` where declared, else the type default.
    pub(crate) fn initial_fields(&mut self, table: usize) -> Vec<Value> {
        let Some(t) = self.engine.program.objects[table].table.clone() else {
            return Vec::new();
        };
        t.fields
            .iter()
            .map(|f| match &f.init {
                Some(c) => ops::convert(crate::constant(c), &f.ty).unwrap_or_else(|_| self.default_value(&f.ty)),
                None => self.default_value(&f.ty),
            })
            .collect()
    }

    pub(crate) fn store_for(&mut self, rec: &Rec) -> &mut MemStore {
        match rec.borrow().temp_id {
            Some(id) => self.temps.entry(id).or_default(),
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
    fn identification(&self, t: &TableDef, fields: &[Value]) -> Result<String, Signal> {
        let mut parts = Vec::new();
        for &i in &t.keys[0] {
            let v = al_builtins::format::try_format(&fields[i], 0, 0, self).map_err(Signal::Unsupported)?;
            parts.push(format!("{}='{}'", t.fields[i].name, v));
        }
        Ok(parts.join(","))
    }

    /// Raises (or, when the result is used, returns false for) a failed write.
    /// Whether the routine running now belongs to a dependency app.
    pub(crate) fn in_dependency_code(&self) -> bool {
        let program = &self.engine.program;
        self.frames
            .last()
            .and_then(|&o| program.apps.get(program.objects[o].app as usize))
            .is_some_and(|a| a.dependency)
    }

    /// Message texts: docs/probes-needed.md (the corpus pins 'already exists'
    /// and 'does not exist' fragments only).
    fn fail(&mut self, t: &TableDef, fields: &[Value], exists: bool) -> Result<Value, Signal> {
        if self.ret_used {
            return Ok(Value::Boolean(false));
        }
        // A dependency app's code reads rows (setup records, mostly) that installing the app and
        // initialising a company create. Where the install baseline could not create them
        // (ADR-013 decision 5), a missing row there is not BC's answer: refuse rather than
        // report it as one.
        if !exists && self.in_dependency_code() && self.engine.unseeded.contains(&(t.object as usize)) {
            return unsupported(format!(
                "{} not found in code of a dependency app: the rows its install and Company-Initialize create could not be created (ADR-013 decision 5)",
                t.name
            ));
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
        Err(Signal::Error(AlError {
            message,
            code: code.into(),
            call_stack: String::new(),
        }))
    }

    /// The table-level trigger `name` (`OnBeforeInsert`, `OnInsert`, ...) a table extension
    /// declares. The order of several extensions' triggers is not pinned, so two
    /// declaring the same one are refused.
    fn ext_trigger(&self, t: &TableDef, name: &str) -> Result<Option<(usize, usize)>, Signal> {
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
        match found.len() {
            0 | 1 => Ok(found.first().copied()),
            _ => unsupported(format!(
                "{name} on {}: declared by more than one table extension (their order is not pinned)",
                t.name
            )),
        }
    }

    /// Runs the extension trigger `name`, if any, with the record and `xrec`.
    fn run_ext_trigger(&mut self, t: &TableDef, name: &str, rec: &Rec, xrec: Option<&Rec>) -> Result<(), Signal> {
        match self.ext_trigger(t, name)? {
            Some((o, r)) => self.run_trigger(o, r, rec, xrec),
            None => Ok(()),
        }
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
        if rec.borrow().temporary {
            return unsupported(format!(
                "{event} of temporary records on {}: whether table events fire for them is not pinned",
                t.name
            ));
        }
        let (names, tail): (&[&str], Value) = match event {
            "onbeforeinsertevent" | "onafterinsertevent" | "onbeforedeleteevent" | "onafterdeleteevent" => {
                (&["Rec", "RunTrigger"], Value::Boolean(run_trigger))
            }
            "onbeforemodifyevent" | "onaftermodifyevent" | "onbeforerenameevent" | "onafterrenameevent" => {
                (&["Rec", "xRec", "RunTrigger"], Value::Boolean(run_trigger))
            }
            // CurrFieldNo is 0 for a Validate called from code (the field's own OnValidate
            // sees 0: handlers/TestPageCurrFieldNo_Tests.al); docs/probes-needed.md for the event.
            "onbeforevalidateevent" | "onaftervalidateevent" => (&["Rec", "xRec", "CurrFieldNo"], Value::Integer(0)),
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
        pos: usize,
        old_value: Value,
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
                // The field is back at its old value when its OnValidate raises
                // (record/TestTriggerRollback.al:OnValidate_Throws_FieldValueRolledBack).
                // Only the validated field is pinned; other changes stay.
                if matches!(e, Signal::Error(_)) {
                    rec.borrow_mut().fields[pos] = old_value;
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
        let cx = self.cx.clone();
        if self.listened(t, "onbeforedeleteevent", None)? || self.listened(t, "onafterdeleteevent", None)? {
            let pk = t.primary_key(&rec.borrow().fields);
            if self
                .store_for(rec)
                .get(&cx, t, &pk)
                .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                .is_none()
            {
                return unsupported(format!(
                    "Delete of a missing row of {}, which has delete event subscribers (the order of the error and the Before event is not pinned)",
                    t.name
                ));
            }
        }
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
        let row = rec.borrow().fields.clone();
        let pk = t.primary_key(&row);
        match self
            .store_for(rec)
            .delete(&cx, t, &pk)
            .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
        {
            WriteOutcome::Done => {
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

    /// Whether inserting `row` would repeat the values of a unique secondary key (a row with
    /// the same primary key is the duplicate-key failure instead).
    fn violates_unique_key(&mut self, rec: &Rec, t: &TableDef, row: &[Value]) -> Result<bool, Signal> {
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
            return Ok(false);
        }
        let cx = self.cx.clone();
        let pk = t.primary_key(row);
        if self
            .store_for(rec)
            .get(&cx, t, &pk)
            .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
            .is_some()
        {
            return Ok(false);
        }
        for k in unique {
            let filters: Vec<FieldFilter> = t.keys[k]
                .iter()
                .map(|&f| FieldFilter {
                    field: f,
                    expr: FilterExpr::Eq(row[f].clone()),
                    group: 0,
                })
                .collect();
            let n = self
                .store_for(rec)
                .count(&cx, t, &filters)
                .map_err(|e| Signal::Unsupported(format!("{e:?}")))?;
            if n > 0 {
                return Ok(true);
            }
        }
        Ok(false)
    }

    /// The refusal of a unique index; only the start of BC's message is pinned.
    fn fail_unique(&mut self, t: &TableDef) -> Result<Value, Signal> {
        if self.ret_used {
            return Ok(Value::Boolean(false));
        }
        Err(Signal::Error(AlError {
            message: format!(
                "There is already a record in table {} that has the same values in a unique index.",
                self.caption(t)
            ),
            code: String::new(),
            call_stack: String::new(),
        }))
    }

    /// Unique secondary keys are only enforced on Insert, so other writes to such tables are refused.
    pub(crate) fn check_unique_keys(&self, t: &TableDef) -> Result<(), Signal> {
        let schema = self.engine.program.objects[t.object as usize]
            .table
            .as_ref()
            .expect("table");
        if schema.unique_keys.iter().skip(1).any(|u| *u) {
            return unsupported(format!(
                "write to {}: unique secondary keys are not enforced yet",
                t.name
            ));
        }
        Ok(())
    }

    /// Whether an insert or modify of `t` would run table code (a trigger, an extension trigger
    /// or an event subscriber). XmlPort import refuses such tables: whether it runs them is not
    /// pinned (docs/probes-needed.md).
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
            any |= self.ext_trigger(t, n)?.is_some();
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

    /// An instance of `object` (a table or table extension) whose Rec is `rec`.
    pub(crate) fn record_instance(
        &mut self,
        object: usize,
        rec: Rec,
        xrec: Option<&Rec>,
    ) -> Result<Rc<RefCell<Instance>>, Signal> {
        let inst = self.instance(object);
        let (rs, xs) = (
            self.engine.program.objects[object].rec_slot,
            self.engine.program.objects[object].xrec_slot,
        );
        // xRec: the caller's handle when the operation has one (shared, so changes made by
        // event subscribers reach the trigger and the other subscribers), else a copy of Rec.
        let copy = match xrec {
            Some(x) => Value::Record(x.clone()),
            None => Value::Record(Rc::new(RefCell::new(rec.borrow().clone()))),
        };
        if let Some(r) = rs {
            inst.borrow_mut().globals[r] = Value::Record(rec);
        }
        if let Some(x) = xs {
            inst.borrow_mut().globals[x] = copy;
        }
        Ok(inst)
    }

    fn run_trigger(&mut self, object: usize, routine: usize, rec: &Rec, xrec: Option<&Rec>) -> Result<(), Signal> {
        let inst = self.record_instance(object, rec.clone(), xrec)?;
        self.call(object, routine, inst, Vec::new()).map(|_| ())
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
        if let Some(i) = t.fields.iter().position(|f| f.id == 2_000_000_000) {
            let mut r = rec.borrow_mut();
            let unset = matches!(r.fields[i], Value::Guid(g) if g == [0; 16]);
            if unset || !keep {
                // Deterministic ids (ADR-006: no OS randomness in the core).
                self.next_guid += 1;
                let mut g = [0u8; 16];
                g[8..].copy_from_slice(&self.next_guid.to_be_bytes());
                g[6] = 0x40;
                r.fields[i] = Value::Guid(g);
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
        let key = rec.borrow().key;
        // Filters on FlowFilter fields drop out; those on FlowFields are checked per row.
        let (filters, flow_field_filters) = self.split_filters(rec);
        let marked_only = rec.borrow().marked_only;
        let cx = self.cx.clone();
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
                .map_err(|e| Signal::Unsupported(format!("store error {e:?}")))?;
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
        let cx = self.cx.clone();
        let taken = self
            .store_for(rec)
            .count(&cx, t, &filters)
            .map_err(|e| Signal::Unsupported(format!("{e:?}")))?;
        if taken == 0 {
            return Ok(());
        }
        let shown = al_builtins::format::try_format(&id, 0, 0, self).map_err(Signal::Unsupported)?;
        Err(Signal::Error(AlError {
            message: format!(
                "There is already a record in table {} that has the same values in a unique index. \
                 The conflict is in the following fields: {}='{}'.",
                self.caption(t),
                self.field_caption(t, i),
                shown
            ),
            code: "DB:RecordExists".into(),
            call_stack: String::new(),
        }))
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
            });
            r.load.partial = false;
        }
        rec.borrow_mut().fields = row;
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
        let cx = self.cx.clone();
        let mut stored: Option<Vec<Value>> = None;
        for &pos in positions {
            let Value::Blob(buf) = rec.borrow().fields[pos].clone() else {
                return unsupported("CalcFields of a Blob field that holds no Blob");
            };
            if buf.borrow().loaded {
                continue;
            }
            if stored.is_none() {
                let pk = t.primary_key(&rec.borrow().fields);
                stored = self
                    .store_for(rec)
                    .get(&cx, &t, &pk)
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?;
            }
            let Some(Value::Blob(src)) = stored.as_ref().map(|row| row[pos].clone()) else {
                return unsupported("CalcFields of a Blob whose row is gone (error text not pinned)");
            };
            let bytes = src.borrow().bytes.clone();
            let mut b = buf.borrow_mut();
            b.bytes = bytes;
            b.loaded = true;
        }
        Ok(())
    }

    pub(crate) fn record_op(&mut self, method: &str, rec: &Rec, args: &mut [Value]) -> Result<Value, Signal> {
        let t = self.table(rec)?;
        let cx = self.cx.clone();
        if let Some(schema) = &self.engine.program.objects[t.object as usize].table {
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
                if method == "Init" && only_unevaluable && messages.len() == 1 {
                    if !rec.borrow().touched {
                        return Ok(Value::Null);
                    }
                    return Err(Signal::Error(AlError {
                        message: (*messages.iter().next().expect("one message")).clone(),
                        code: String::new(),
                        call_stack: String::new(),
                    }));
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
                "Insert"
                    | "Modify"
                    | "Delete"
                    | "DeleteAll"
                    | "ModifyAll"
                    | "Rename"
                    | "Truncate"
                    | "Get"
                    | "Find"
                    | "FindFirst"
                    | "FindLast"
                    | "FindSet"
                    | "Next"
                    | "Count"
                    | "IsEmpty"
                    | "CalcFields"
                    | "CalcSums"
            )
        {
            return unsupported(format!(
                "{method} on the per-company table {} with no company chosen (BC: \"You must choose a company before you can access the ...\")",
                t.name
            ));
        }
        if matches!(
            method,
            "Insert" | "Modify" | "Delete" | "DeleteAll" | "ModifyAll" | "Rename" | "Truncate"
        ) && !rec.borrow().temporary
        {
            self.bg_refuse_write(t.object as usize, method)?;
            self.gate_write(method)?;
        }
        match method {
            "Init" => {
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
            "Clear" => {
                if !rec.borrow().auto_calc.is_empty() {
                    return unsupported("Clear after SetAutoCalcFields (what resets it is not pinned yet)");
                }
                if rec.borrow().temporary {
                    return unsupported("Clear of a temporary record (table reset not pinned yet)");
                }
                let fresh = self.initial_fields(t.object as usize);
                let mut r = rec.borrow_mut();
                r.fields = fresh;
                r.filters.clear();
                r.filter_group = 0;
                r.marks.clear();
                r.marked_only = false;
                r.key = 0;
                r.ascending = true;
                r.field_descending = false;
                r.position = None;
                r.load = Default::default();
                Ok(Value::Null)
            }
            "Insert" => {
                // A temporary record has no sequence: the key stays as it is (corpus
                // record/TestTempTableContracts.al:52).
                let auto = if rec.borrow().temporary {
                    None
                } else {
                    self.auto_increment_field(&t)?
                };
                let run = arg_bool(args, 0)?;
                if self.listened(&t, "onbeforeinsertevent", None)? || self.listened(&t, "onafterinsertevent", None)? {
                    let pk = t.primary_key(&rec.borrow().fields);
                    if self
                        .store_for(rec)
                        .get(&cx, &t, &pk)
                        .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                        .is_some()
                    {
                        return unsupported(format!(
                            "Insert of an existing key into {}, which has insert event subscribers (the order of the error and the Before event is not pinned)",
                            t.name
                        ));
                    }
                }
                // Before event, then the trigger, then the write, then the After event
                // (record/TestTriggerDispatchOrder.al:TriggerOrder_Insert_RecordPipeline_IsStable).
                self.table_event(&t, "onbeforeinsertevent", rec, None, run, None)?;
                // Rec already carries its SystemId in OnInsert, the one that is stored
                // (record/TestXRecStampFields.al:80-90); the stamps and the key come below it.
                let keep_id = match args.get(1) {
                    Some(v) => ops::truthy(v)?,
                    None => false,
                };
                self.assign_system_id(&t, rec, keep_id);
                let id_slot = t.fields.iter().position(|f| f.id == 2_000_000_000);
                let assigned = id_slot.map(|i| rec.borrow().fields[i].clone());
                // xRec in the insert triggers is the buffer without the SystemId it was just given
                // (record/TestXRecStampFields.al:38-52).
                let insert_xrec = id_slot.map(|i| {
                    let mut copy = rec.borrow().clone();
                    copy.fields[i] = Value::Guid([0; 16]);
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
                    taken = self.take_auto_increment(&t, a, &mut row)?;
                    rec.borrow_mut().fields = row;
                }
                let row = rec.borrow().fields.clone();
                // A unique secondary key refuses the row after its AutoIncrement value was taken
                // (corpus record/TestAutoIncrementExplicitKey.al:131,163).
                if self.violates_unique_key(rec, &t, &row)? {
                    Self::restore_row_version(rec, old_version);
                    Self::unstamp(rec, stamped);
                    if let Some(a) = auto {
                        if taken {
                            rec.borrow_mut().fields[a] = self.default_value(&t.fields[a].ty);
                        } else {
                            self.note_explicit_key(&t, a, &row);
                        }
                    }
                    return self.fail_unique(&t);
                }
                let stored = Self::to_store(rec, self.stored_view(&t, &row));
                match self
                    .store_for(rec)
                    .insert(&cx, &t, stored.clone())
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                {
                    InsertOutcome::Inserted => {
                        if let (Some(a), false) = (auto, taken) {
                            self.note_explicit_key(&t, a, &row);
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
                        self.fail(&t, &row, true)
                    }
                }
            }
            "Modify" => {
                self.check_unique_keys(&t)?;
                let run = arg_bool(args, 0)?;
                let pk = t.primary_key(&rec.borrow().fields);
                let stored = self
                    .store_for(rec)
                    .get(&cx, &t, &pk)
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?;
                if stored.is_none()
                    && (self.listened(&t, "onbeforemodifyevent", None)?
                        || self.listened(&t, "onaftermodifyevent", None)?)
                {
                    return unsupported(format!(
                        "Modify of a missing row of {}, which has modify event subscribers (the order of the error and the Before event is not pinned)",
                        t.name
                    ));
                }
                // A buffer older than a committed change to its row cannot be saved
                // (record/TestRecordStaleBufferWrite.al); before a commit the last writer wins.
                if let (Some(row), Some(i)) = (&stored, t.fields.iter().position(|f| f.id == 2_000_000_005)) {
                    let (mine, theirs) = (rec.borrow().fields[i].clone(), row[i].clone());
                    let version = |v: &Value| if let Value::BigInteger(n) = v { *n } else { 0 };
                    if !rec.borrow().temporary
                        && version(&mine) != 0
                        && version(&mine) != version(&theirs)
                        && version(&theirs) <= self.committed_row_version
                    {
                        let identification = self.identification(&t, &rec.borrow().fields)?;
                        return Err(Signal::Error(AlError {
                            message: format!(
                                "The changes to the {} record cannot be saved because some information on the page is not up-to-date. Close the page, reopen it, and try again. Identification fields and values: {}",
                                self.caption(&t),
                                identification
                            ),
                            code: "DB:RecordChanged".into(),
                            call_stack: String::new(),
                        }));
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
                // xRec is the stored row (the page's own xRec for a row a page saves), one handle
                // for the events and the trigger.
                let stored_row = stored.clone();
                let from_page = self.page_xrec.is_some();
                let xrec = self.page_xrec.take().or(stored).map(|f| Self::xrec_handle(rec, f));
                self.table_event(&t, "onbeforemodifyevent", rec, xrec.as_ref(), run, None)?;
                if run {
                    self.run_ext_trigger(&t, "OnBeforeModify", rec, xrec.as_ref())?;
                    if let Some(tr) = self.table_trigger(&t, "OnModify") {
                        // The table's own OnModify sees xRec equal to Rec when code drove the
                        // Modify, and the previous row when a page did
                        // (record/TestXRecContracts.al:69-87 OnModify_xRec_MirrorsRecValues_WhenCalledFromCode,
                        // record/TestXRecDriverContracts.al:32-51 Record_Modify_FromPage_xRecHoldsPreviousValue).
                        let code_xrec = (!from_page).then(|| Self::xrec_handle(rec, rec.borrow().fields.clone()));
                        self.run_trigger(t.object as usize, tr, rec, code_xrec.as_ref().or(xrec.as_ref()))?;
                    }
                    self.run_ext_trigger(&t, "OnModify", rec, xrec.as_ref())?;
                }
                self.global_trigger(&t, GlobalOp::Modify, rec, xrec.as_ref())?;
                let old_version = self.bump_row_version(&t, rec);
                let stamped = self.stamp_modify(&t, rec, stored_row.as_deref());
                let mut row = rec.borrow().fields.clone();
                for (i, v) in kept {
                    row[i] = v;
                }
                let stored = Self::to_store(rec, self.stored_view(&t, &row));
                match self
                    .store_for(rec)
                    .modify(&cx, &t, stored.clone())
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                {
                    WriteOutcome::Done => {
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
            "Delete" => {
                let run = arg_bool(args, 0)?;
                self.delete_row(&t, rec, run)
            }
            "DeleteAll" => {
                let run = arg_bool(args, 0)?;
                let (filters, flow_field_filters) = self.split_filters(rec);
                if !flow_field_filters.is_empty() {
                    return unsupported("DeleteAll with a filter on a FlowField");
                }
                let n = self
                    .store_for(rec)
                    .count(&cx, &t, &filters)
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?;
                // No row, no trigger and no event.
                if n == 0 {
                    return Ok(Value::Null);
                }
                let events =
                    self.listened(&t, "onbeforedeleteevent", None)? || self.listened(&t, "onafterdeleteevent", None)?;
                let mut trigger = self.table_trigger(&t, "OnDelete").is_some();
                for n in ["OnBeforeDelete", "OnDelete", "OnAfterDelete"] {
                    trigger |= self.ext_trigger(&t, n)?.is_some();
                }
                let global = self.global_enabled(&t, GlobalOp::Delete)?;
                // With delete events or database triggers a DeleteAll deletes row by row: the events
                // and the database trigger per row, the table's and extensions' triggers only for
                // DeleteAll(true). Pinned on real BC 27.5 by the probe codeunit 68212
                // (record/TestTableExtensionTriggerOrderContracts.al:Delete_And_DeleteAll_...,
                // corpus branch `al-runtime-rs`; the citation is pending until the corpus bump).
                if events || global || (run && trigger) {
                    // One Delete per row (record/TestTriggerContracts.al:DeleteAll_WithTrigger_FiresForEachFilteredRecord).
                    let key = rec.borrow().key;
                    let mut rows = Vec::new();
                    let mut after = None;
                    while let Some(row) = self.scan(rec, &t, true, after.take())? {
                        after = Some(position(&t, key, &row));
                        rows.push(row);
                    }
                    for row in rows {
                        let mut st = rec.borrow().clone();
                        st.fields = row;
                        st.filters.clear();
                        self.delete_row(&t, &Rc::new(RefCell::new(st)), run)?;
                    }
                    return Ok(Value::Null);
                }
                self.store_for(rec)
                    .delete_all(&cx, &t, &filters)
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?;
                Ok(Value::Null)
            }
            "Get" => {
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
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                {
                    Some(row) => {
                        self.load(rec, row)?;
                        Ok(Value::Boolean(true))
                    }
                    None => {
                        let mut ident = rec.borrow().fields.clone();
                        for (n, &field) in t.keys[0].iter().enumerate() {
                            ident[field] = pk[n].clone();
                        }
                        self.fail(&t, &ident, false)
                    }
                }
            }
            "FindFirst" | "FindSet" | "FindLast" | "Find" => {
                let asc = {
                    let r = rec.borrow();
                    r.ascending && !r.field_descending
                };
                let which = match (method, args.first()) {
                    ("FindLast", _) => "+".to_string(),
                    ("Find", Some(v)) => al_builtins::native::text_of(v).unwrap_or_default(),
                    // Doc (Record.Find): Which defaults to '='.
                    ("Find", None) => "=".to_string(),
                    _ => "-".to_string(),
                };
                let found = match which.as_str() {
                    "-" => self.scan(rec, &t, asc, None)?,
                    "+" => self.scan(rec, &t, !asc, None)?,
                    // How '=' combines the key with active filters is only partly pinned
                    // (OwnFilter_ModifiedOutOfIt_FindStillFindsTheRow vs
                    // OwnFilter_SecondVarWithTheSameFilter_FindDoesNotFindTheRow): docs/probes-needed.md.
                    "=" if !rec.borrow().filters.is_empty() => return unsupported("Find('=') with active filters"),
                    "=" => {
                        let pk = t.primary_key(&rec.borrow().fields);
                        self.store_for(rec)
                            .get(&cx, &t, &pk)
                            .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                    }
                    // `=>`, `=<` and `=><`: the row itself, else its neighbour. Pinned for a row that is gone
                    // from a table nothing filters, by the page client asking an OnFindRecord trigger
                    // (corpus pageupdate/TestPageUpdateGoneRow.al:423 List_DeletedByAction_PassThroughFind_*);
                    // with filters, or another current key, how they combine is not pinned.
                    w @ ("=>" | "=<" | "=><") => {
                        if !rec.borrow().filters.is_empty() || rec.borrow().key != 0 {
                            return unsupported(format!("Find('{w}') with active filters or another current key"));
                        }
                        let pk = t.primary_key(&rec.borrow().fields);
                        let exact = self
                            .store_for(rec)
                            .get(&cx, &t, &pk)
                            .map_err(|e| Signal::Unsupported(format!("{e:?}")))?;
                        match exact {
                            Some(row) => Some(row),
                            None => {
                                let pos = position(&t, 0, &rec.borrow().fields);
                                let next = if w == "=<" {
                                    None
                                } else {
                                    self.scan(rec, &t, asc, Some(pos.clone()))?
                                };
                                let prev = if w == "=>" {
                                    None
                                } else {
                                    self.scan(rec, &t, !asc, Some(pos))?
                                };
                                match (next, prev) {
                                    (Some(n), Some(_)) if w == "=><" => {
                                        let _ = n;
                                        return unsupported(
                                            "Find('=><') with a row on either side (which one BC picks is not pinned)",
                                        );
                                    }
                                    (n, p) => n.or(p),
                                }
                            }
                        }
                    }
                    other => return unsupported(format!("Find('{other}')")),
                };
                match found {
                    Some(row) => {
                        self.load(rec, row)?;
                        Ok(Value::Boolean(true))
                    }
                    None if self.ret_used => Ok(Value::Boolean(false)),
                    // The "nothing in the filter" error text is not pinned yet.
                    None => unsupported(format!("{method} on an empty set without using the result")),
                }
            }
            "Next" => {
                let steps = match args.first() {
                    Some(Value::Integer(n)) => *n,
                    None => 1,
                    other => return unsupported(format!("Next({other:?})")),
                };
                let (key, asc) = {
                    let r = rec.borrow();
                    (r.key, r.ascending && !r.field_descending)
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
            "Count" | "IsEmpty" => {
                let (filters, flow_field_filters) = self.split_filters(rec);
                let n = if flow_field_filters.is_empty() && !rec.borrow().marked_only {
                    self.store_for(rec)
                        .count(&cx, &t, &filters)
                        .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                } else {
                    // Rows are tested one by one against the calculated FlowFields.
                    let mut n = 0;
                    let mut after = None;
                    while let Some(row) = self.scan(rec, &t, true, after)? {
                        n += 1;
                        if method == "IsEmpty" {
                            break;
                        }
                        after = Some(position(&t, rec.borrow().key, &row));
                    }
                    n
                };
                Ok(if method == "Count" {
                    Value::Integer(n as i32)
                } else {
                    Value::Boolean(n == 0)
                })
            }
            "SetRange" => {
                let pos = arg_pos(args, 0)?;
                let ty = t.fields[pos].ty.clone();
                let mut r = rec.borrow_mut();
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
            "SetFilter" => {
                let pos = arg_pos(args, 0)?;
                let ty = t.fields[pos].ty.clone();
                let text = al_builtins::native::text_of(args.get(1).unwrap_or(&Value::Null)).unwrap_or_default();
                let engine = self.engine.clone();
                let enums = |id: u32| engine.program.objects[id as usize].enum_values.clone();
                let expr = al_builtins::filter::parse_filter_with(&text, &args[2..], &ty, &enums)
                    .map_err(|e| Signal::Unsupported(format!("SetFilter: {e}")))?;
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
            "Reset" => {
                if !rec.borrow().auto_calc.is_empty() {
                    return unsupported("Reset after SetAutoCalcFields (whether it keeps them is not pinned yet)");
                }
                let mut r = rec.borrow_mut();
                r.filters.clear();
                r.key = 0;
                r.ascending = true;
                r.field_descending = false;
                Ok(Value::Null)
            }
            "SetCurrentKey" => {
                let wanted: Vec<usize> = (0..args.len()).map(|i| arg_pos(args, i)).collect::<Result<_, _>>()?;
                match t
                    .keys
                    .iter()
                    .position(|k| k.len() >= wanted.len() && k[..wanted.len()] == wanted[..])
                {
                    Some(k) => {
                        rec.borrow_mut().key = k;
                        Ok(Value::Boolean(true))
                    }
                    None => unsupported("SetCurrentKey without a matching key"),
                }
            }
            "Validate" => {
                let pos = arg_pos(args, 0)?;
                let schema = self.engine.program.objects[t.object as usize]
                    .table
                    .clone()
                    .expect("table");
                let field = schema.fields[pos].clone();
                // A Validate nested in an OnValidate of the same record keeps the outer call's
                // before-image as xRec (corpus record/TestXRecContracts.al:133).
                let handle = Rc::as_ptr(rec) as usize;
                let outer = self
                    .validating
                    .iter()
                    .find(|(h, _)| *h == handle)
                    .map(|(_, b)| b.clone());
                let before = outer.clone().unwrap_or_else(|| rec.borrow().fields.clone());
                let old_value = rec.borrow().fields[pos].clone();
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
                let r = self.validate_hooks(&t, rec, &xrec, &field, pos, old_value);
                if outer.is_none() {
                    self.validating.pop();
                }
                r?;
                Ok(Value::Null)
            }
            "Rename" => {
                if rec.borrow().temp_id.is_some() && rec.borrow().fields.iter().any(|f| matches!(f, Value::Blob(_))) {
                    // streams/TestBlobRenameIsolation.al: BC keeps an unpersisted Blob write across a
                    // temporary Rename but loses one persisted with Modify; no model here does both.
                    return unsupported("Rename of a temporary record with a Blob field");
                }
                self.check_unique_keys(&t)?;
                // Fields that relate to this table follow the rename; a temporary record has none.
                let refs = if rec.borrow().temporary {
                    Vec::new()
                } else {
                    self.rename_references(&t)?
                };
                let old = rec.borrow().fields.clone();
                let mut new = old.clone();
                for (n, &field) in t.keys[0].iter().enumerate() {
                    let ty = t.fields[field].ty.clone();
                    if let Some(v) = args.get(n) {
                        new[field] = ops::convert(v.clone(), &ty)?;
                    }
                }
                self.check_rename_auto_increment(&t, &old, &new)?;
                let old_pk = t.primary_key(&old);
                if self.listened(&t, "onbeforerenameevent", None)? || self.listened(&t, "onafterrenameevent", None)? {
                    let new_pk = t.primary_key(&new);
                    let from = self
                        .store_for(rec)
                        .get(&cx, &t, &old_pk)
                        .map_err(|e| Signal::Unsupported(format!("{e:?}")))?;
                    let to = self
                        .store_for(rec)
                        .get(&cx, &t, &new_pk)
                        .map_err(|e| Signal::Unsupported(format!("{e:?}")))?;
                    if from.is_none() || (to.is_some() && key_cmp(&new_pk, &old_pk) != std::cmp::Ordering::Equal) {
                        return unsupported(format!(
                            "Rename of {} to a missing or taken key, which has rename event subscribers (the order of the error and the Before event is not pinned)",
                            t.name
                        ));
                    }
                }
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
                let stored = Self::to_store(rec, self.stored_view(&t, &row));
                // A rename to the key the row has succeeds and writes the row again, which is what
                // stamps it (record/TestRecordSystemId.al:752-775).
                let same_key = ops::compare_keys(&old_pk, &t.primary_key(&row));
                let outcome = if same_key {
                    match self
                        .store_for(rec)
                        .modify(&cx, &t, stored.clone())
                        .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                    {
                        WriteOutcome::Done => RenameOutcome::Renamed,
                        WriteOutcome::NotFound => RenameOutcome::NotFound,
                    }
                } else {
                    self.store_for(rec)
                        .rename(&cx, &t, &old_pk, stored.clone())
                        .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                };
                match outcome {
                    RenameOutcome::Renamed => {
                        if !same_key {
                            self.propagate_rename(&t, &refs, &old_fields, &row)?;
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
            "Truncate" => self.truncate(&t, rec, arg_bool(args, 0)?),
            "CalcFields" | "CalcSums" | "SetAutoCalcFields" => self.flow_op(method, rec, args),
            "SetLoadFields" | "AddLoadFields" | "AreFieldsLoaded" | "SetBaseLoadFields" => {
                self.load_fields_op(method, rec, &t, args)
            }
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
