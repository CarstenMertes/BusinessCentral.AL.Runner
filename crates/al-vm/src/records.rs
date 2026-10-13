//! Record operations on the store (ADR-003) for the `Record.*` builtins.
//!
//! Pinned by the corpus or the docs as noted; behaviour neither pins yet
//! raises `Signal::Unsupported` instead of guessing.

use crate::Session;
use al_builtins::filter::{FieldFilter, FilterExpr, key_cmp};
use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::value::{Instance, RecordState, RtType, Value};
use al_data::{InsertOutcome, RenameOutcome, Scan, Store, TableDef, WriteOutcome, memory::MemStore, position};
use std::cell::RefCell;
use std::rc::Rc;

type Rec = Rc<RefCell<RecordState>>;

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
    fn table(&self, rec: &Rec) -> Result<TableDef, Signal> {
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

    fn store_for(&mut self, rec: &Rec) -> &mut MemStore {
        match rec.borrow().temp_id {
            Some(id) => self.temps.entry(id).or_default(),
            None => &mut self.store,
        }
    }

    fn caption(&self, t: &TableDef) -> String {
        self.engine.program.objects[t.object as usize]
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("Caption"))
            .map(|(_, v)| v.trim().trim_matches('\'').to_string())
            .unwrap_or_else(|| t.name.clone())
    }

    /// `No.='A'` style identification of a row's primary key.
    fn identification(&self, t: &TableDef, fields: &[Value]) -> String {
        t.keys[0]
            .iter()
            .map(|&i| {
                format!(
                    "{}='{}'",
                    t.fields[i].name,
                    al_builtins::format::format(&fields[i], 0, 0, self)
                )
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Raises (or, when the result is used, returns false for) a failed write.
    /// Message texts: docs/probes-needed.md (the corpus pins 'already exists'
    /// and 'does not exist' fragments only).
    fn fail(&mut self, t: &TableDef, fields: &[Value], exists: bool) -> Result<Value, Signal> {
        if self.ret_used {
            return Ok(Value::Boolean(false));
        }
        let (message, code) = if exists {
            (
                format!(
                    "The record in table {} already exists. Identification fields and values: {}",
                    self.caption(t),
                    self.identification(t, fields)
                ),
                "DB:RecordExists",
            )
        } else {
            (
                format!(
                    "The {} does not exist. Identification fields and values: {}",
                    self.caption(t),
                    self.identification(t, fields)
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

    /// Hooks around `op` that this runtime cannot run are refused rather than
    /// silently skipped (hard rule 4): the triggers a table extension attaches to a
    /// field (`modify(Field) { trigger OnBeforeValidate }`), and the platform events
    /// of the Base App's `Global Triggers` codeunit.
    fn check_hooks(&self, t: &TableDef, op: &str) -> Result<(), Signal> {
        if op == "Validate" {
            if let Some(schema) = &self.engine.program.objects[t.object as usize].table {
                for &e in &schema.extensions {
                    let hit = self.engine.program.objects[e].routines.iter().any(|r| {
                        r.is_trigger
                            && r.field.is_none()
                            && ["OnBeforeValidate", "OnAfterValidate"]
                                .iter()
                                .any(|n| r.name.eq_ignore_ascii_case(n))
                    });
                    if hit {
                        return unsupported(format!(
                            "{op} on {}: table extension triggers on fields are not implemented yet",
                            t.name
                        ));
                    }
                }
            }
        }
        // The workspace subscribes to the platform's Global Triggers events for this
        // table (record/TestTriggerDispatchOrder.al: OnDatabaseInsert/OnGlobalRename ...).
        // That codeunit is Base App, so those subscribers can never run here.
        if op != "Validate" && self.engine.events.global_triggers_apply(t.object as usize) {
            return unsupported(format!(
                "{op} on {}: events of the Global Triggers codeunit (Base App) are not available",
                t.name
            ));
        }
        Ok(())
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

    fn assign_system_id(&mut self, t: &TableDef, rec: &Rec) {
        if let Some(i) = t.fields.iter().position(|f| f.id == 2_000_000_000) {
            let mut r = rec.borrow_mut();
            if matches!(r.fields[i], Value::Guid(g) if g == [0; 16]) {
                // Deterministic ids (ADR-006: no OS randomness in the core).
                self.next_guid += 1;
                let mut g = [0u8; 16];
                g[8..].copy_from_slice(&self.next_guid.to_be_bytes());
                g[6] = 0x40;
                r.fields[i] = Value::Guid(g);
            }
        }
    }

    fn scan(
        &mut self,
        rec: &Rec,
        t: &TableDef,
        ascending: bool,
        after: Option<Vec<Value>>,
    ) -> Result<Option<Vec<Value>>, Signal> {
        let (key, filters) = {
            let r = rec.borrow();
            (r.key, r.filters.clone())
        };
        let cx = self.cx.clone();
        let store = self.store_for(rec);
        store
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
            .map_err(|e| Signal::Unsupported(format!("store error {e:?}")))
    }

    fn load(rec: &Rec, row: Vec<Value>) {
        rec.borrow_mut().fields = row;
    }

    pub(crate) fn record_op(&mut self, method: &str, rec: &Rec, args: &mut [Value]) -> Result<Value, Signal> {
        let t = self.table(rec)?;
        let cx = self.cx.clone();
        if let Some(schema) = &self.engine.program.objects[t.object as usize].table {
            if let Some(f) = schema.fields.iter().find(|f| f.init_text.is_some() && f.init.is_none()) {
                return unsupported(format!(
                    "{}.{}: InitValue `{}` is not implemented yet",
                    t.name,
                    f.name,
                    f.init_text.as_deref().unwrap_or("")
                ));
            }
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
                if rec.borrow().temporary {
                    return unsupported("Clear of a temporary record (table reset not pinned yet)");
                }
                let fresh = self.initial_fields(t.object as usize);
                let mut r = rec.borrow_mut();
                r.fields = fresh;
                r.filters.clear();
                r.key = 0;
                r.ascending = true;
                r.position = None;
                Ok(Value::Null)
            }
            "Insert" => {
                self.check_hooks(&t, "Insert")?;
                let schema = self.engine.program.objects[t.object as usize]
                    .table
                    .clone()
                    .expect("table");
                if schema.fields.iter().any(|f| f.auto_increment) {
                    return unsupported(
                        "Insert into a table with an AutoIncrement field (counter semantics not implemented yet)",
                    );
                }
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
                if run {
                    // The extension's triggers bracket the base table's own (tableextensiontrigger/
                    // TestTableExtensionTableTriggers_Tests.al); Insert(false) runs none of them.
                    self.run_ext_trigger(&t, "OnBeforeInsert", rec, None)?;
                    if let Some(tr) = self.table_trigger(&t, "OnInsert") {
                        self.run_trigger(t.object as usize, tr, rec, None)?;
                    }
                    self.run_ext_trigger(&t, "OnInsert", rec, None)?;
                }
                self.assign_system_id(&t, rec);
                let old_version = self.bump_row_version(&t, rec);
                let row = rec.borrow().fields.clone();
                match self
                    .store_for(rec)
                    .insert(&cx, &t, row.clone())
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                {
                    InsertOutcome::Inserted => {
                        if run {
                            self.run_ext_trigger(&t, "OnAfterInsert", rec, None)?;
                        }
                        self.table_event(&t, "onafterinsertevent", rec, None, run, None)?;
                        Ok(Value::Boolean(true))
                    }
                    InsertOutcome::AlreadyExists => {
                        Self::restore_row_version(rec, old_version);
                        self.fail(&t, &row, true)
                    }
                }
            }
            "Modify" => {
                self.check_hooks(&t, "Modify")?;
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
                        return Err(Signal::Error(AlError {
                            message: format!(
                                "The changes to the {} record cannot be saved because some information on the page is not up-to-date. Close the page, reopen it, and try again. Identification fields and values: {}",
                                self.caption(&t),
                                self.identification(&t, &rec.borrow().fields)
                            ),
                            code: "DB:RecordChanged".into(),
                            call_stack: String::new(),
                        }));
                    }
                }
                // xRec is the stored row, one handle for the events and the trigger.
                let xrec = stored.map(|f| Self::xrec_handle(rec, f));
                self.table_event(&t, "onbeforemodifyevent", rec, xrec.as_ref(), run, None)?;
                if run {
                    self.run_ext_trigger(&t, "OnBeforeModify", rec, xrec.as_ref())?;
                    if let Some(tr) = self.table_trigger(&t, "OnModify") {
                        self.run_trigger(t.object as usize, tr, rec, xrec.as_ref())?;
                    }
                    self.run_ext_trigger(&t, "OnModify", rec, xrec.as_ref())?;
                }
                let old_version = self.bump_row_version(&t, rec);
                let row = rec.borrow().fields.clone();
                match self
                    .store_for(rec)
                    .modify(&cx, &t, row.clone())
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
                        self.fail(&t, &row, false)
                    }
                }
            }
            "Delete" => {
                self.check_hooks(&t, "Delete")?;
                let run = arg_bool(args, 0)?;
                self.delete_row(&t, rec, run)
            }
            "DeleteAll" => {
                let run = arg_bool(args, 0)?;
                let filters = rec.borrow().filters.clone();
                let n = self
                    .store_for(rec)
                    .count(&cx, &t, &filters)
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?;
                // No row, no trigger and no event.
                if n == 0 {
                    return Ok(Value::Null);
                }
                self.check_hooks(&t, "DeleteAll")?;
                let events =
                    self.listened(&t, "onbeforedeleteevent", None)? || self.listened(&t, "onafterdeleteevent", None)?;
                let mut trigger = self.table_trigger(&t, "OnDelete").is_some();
                for n in ["OnBeforeDelete", "OnDelete", "OnAfterDelete"] {
                    trigger |= self.ext_trigger(&t, n)?.is_some();
                }
                if !run && events {
                    // Whether DeleteAll(false) raises the delete events per row (as BC must when
                    // Truncate is refused for subscribed tables, record/TestRecordTruncateEventAndFilterGuards.al)
                    // is not pinned: docs/probes-needed.md.
                    return unsupported(format!(
                        "DeleteAll(false) on {}, which has delete event subscribers",
                        t.name
                    ));
                }
                if run && (events || trigger) {
                    // One Delete(true) per row (record/TestTriggerContracts.al:DeleteAll_WithTrigger_FiresForEachFilteredRecord).
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
                        self.delete_row(&t, &Rc::new(RefCell::new(st)), true)?;
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
                        Self::load(rec, row);
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
                let asc = rec.borrow().ascending;
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
                    other => return unsupported(format!("Find('{other}')")),
                };
                match found {
                    Some(row) => {
                        Self::load(rec, row);
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
                    (r.key, r.ascending)
                };
                let forward = (steps >= 0) == asc;
                let mut moved = 0;
                for _ in 0..steps.unsigned_abs().max(if steps == 0 { 0 } else { 1 }) {
                    let pos = position(&t, key, &rec.borrow().fields);
                    match self.scan(rec, &t, forward, Some(pos))? {
                        Some(row) => {
                            Self::load(rec, row);
                            moved += 1;
                        }
                        None => break,
                    }
                }
                Ok(Value::Integer(if steps < 0 { -moved } else { moved }))
            }
            "Count" | "IsEmpty" => {
                let filters = rec.borrow().filters.clone();
                let n = self
                    .store_for(rec)
                    .count(&cx, &t, &filters)
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?;
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
                r.filters.retain(|f| f.field != pos);
                if let Some(from) = args.get(1) {
                    let from = ops::convert(from.clone(), &ty)?;
                    let to = match args.get(2) {
                        Some(v) => ops::convert(v.clone(), &ty)?,
                        None => from.clone(),
                    };
                    r.filters.push(FieldFilter {
                        field: pos,
                        expr: FilterExpr::Range(Some(from), Some(to)),
                    });
                }
                Ok(Value::Null)
            }
            "SetFilter" => {
                let pos = arg_pos(args, 0)?;
                let ty = t.fields[pos].ty.clone();
                let text = al_builtins::native::text_of(args.get(1).unwrap_or(&Value::Null)).unwrap_or_default();
                let expr = al_builtins::filter::parse_filter(&text, &args[2..], &ty)
                    .map_err(|e| Signal::Unsupported(format!("SetFilter: {e}")))?;
                let mut r = rec.borrow_mut();
                r.filters.retain(|f| f.field != pos);
                r.filters.push(FieldFilter { field: pos, expr });
                Ok(Value::Null)
            }
            "Reset" => {
                let mut r = rec.borrow_mut();
                r.filters.clear();
                r.key = 0;
                r.ascending = true;
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
                self.check_hooks(&t, "Validate")?;
                if field.relation {
                    return unsupported("Validate of a field with a TableRelation");
                }
                let before = rec.borrow().fields.clone();
                if let Some(v) = args.get(1) {
                    let v = ops::convert(v.clone(), &field.ty)?;
                    rec.borrow_mut().fields[pos] = v;
                }
                let old_value = before[pos].clone();
                // A Validate nested in another one's trigger on the same record keeps the outer
                // call's xRec (record/TestXRecContracts.al:OnValidate_Nested_InnerTrigger_...).
                let id = Rc::as_ptr(rec) as usize;
                let xrec = match self.active_xrecs.iter().rev().find(|(p, _)| *p == id) {
                    Some((_, x)) => x.clone(),
                    None => Self::xrec_handle(rec, before),
                };
                self.active_xrecs.push((id, xrec.clone()));
                let r = self.validate_hooks(&t, rec, &xrec, &field, pos, old_value);
                self.active_xrecs.pop();
                r?;
                Ok(Value::Null)
            }
            "Rename" => {
                self.check_hooks(&t, "Rename")?;
                let name = t.name.to_lowercase();
                let related = self
                    .engine
                    .program
                    .objects
                    .iter()
                    .filter_map(|o| o.table.as_ref())
                    .flat_map(|tb| &tb.fields)
                    .any(|f| f.relation_tables.contains(&name));
                if related {
                    return unsupported(format!(
                        "Rename of {}: other tables relate to it (re-keying related rows is not implemented yet)",
                        t.name
                    ));
                }
                let old = rec.borrow().fields.clone();
                let mut new = old.clone();
                for (n, &field) in t.keys[0].iter().enumerate() {
                    let ty = t.fields[field].ty.clone();
                    if let Some(v) = args.get(n) {
                        new[field] = ops::convert(v.clone(), &ty)?;
                    }
                }
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
                rec.borrow_mut().fields = new.clone();
                let xrec = Self::xrec_handle(rec, old.clone());
                // Rename always runs the trigger; RunTrigger is true (docs/probes-needed.md).
                self.table_event(&t, "onbeforerenameevent", rec, Some(&xrec), true, None)?;
                self.run_ext_trigger(&t, "OnBeforeRename", rec, Some(&xrec))?;
                if let Some(tr) = self.table_trigger(&t, "OnRename") {
                    self.run_trigger(t.object as usize, tr, rec, Some(&xrec))?;
                }
                self.run_ext_trigger(&t, "OnRename", rec, Some(&xrec))?;
                let _ = self.bump_row_version(&t, rec);
                let row = rec.borrow().fields.clone();
                match self
                    .store_for(rec)
                    .rename(&cx, &t, &old_pk, row.clone())
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                {
                    RenameOutcome::Renamed => {
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
            other => unsupported(format!("Record.{other}")),
        }
    }
}

#[allow(dead_code)]
fn _uses(_: RtType, a: &[Value], b: &[Value]) -> std::cmp::Ordering {
    key_cmp(a, b)
}
