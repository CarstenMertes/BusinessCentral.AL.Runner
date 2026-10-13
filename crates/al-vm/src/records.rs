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

    fn store_for(&mut self, rec: &Rec) -> &mut MemStore {
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

    /// Writes into tables whose behaviour we do not reproduce yet are refused.
    fn check_writable(&self, t: &TableDef, op: &str) -> Result<(), Signal> {
        if self.engine.tables_with_subscribers.contains(&t.name.to_lowercase()) {
            return unsupported(format!(
                "{op} on {}: table event subscribers are not implemented yet",
                t.name
            ));
        }
        // Only the extension's table-level triggers for this operation matter;
        // its field OnValidate triggers already run through Validate.
        let op_triggers: &[&str] = match op {
            "Insert" => &["OnBeforeInsert", "OnInsert", "OnAfterInsert"],
            "Modify" => &["OnBeforeModify", "OnModify", "OnAfterModify"],
            "Delete" | "DeleteAll" => &["OnBeforeDelete", "OnDelete", "OnAfterDelete"],
            "Rename" => &["OnBeforeRename", "OnRename", "OnAfterRename"],
            "Validate" => &["OnBeforeValidate", "OnAfterValidate"],
            _ => &[],
        };
        if let Some(schema) = &self.engine.program.objects[t.object as usize].table {
            for &e in &schema.extensions {
                let hit = self.engine.program.objects[e].routines.iter().any(|r| {
                    r.is_trigger && r.field.is_none() && op_triggers.iter().any(|n| r.name.eq_ignore_ascii_case(n))
                });
                if hit {
                    return unsupported(format!(
                        "{op} on {}: table extension triggers are not implemented yet",
                        t.name
                    ));
                }
            }
        }
        Ok(())
    }

    /// The row as the store keeps it: FlowFields and FlowFilters hold no data, whatever the
    /// buffer shows after CalcFields or an assignment.
    fn stored_view(&mut self, t: &TableDef, row: &[Value]) -> Vec<Value> {
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

    fn table_trigger(&self, t: &TableDef, name: &str) -> Option<usize> {
        self.engine.program.objects[t.object as usize]
            .routines
            .iter()
            .position(|r| r.is_trigger && r.field.is_none() && r.name.eq_ignore_ascii_case(name))
    }

    /// An instance of `object` (a table or table extension) whose Rec is `rec`.
    pub(crate) fn record_instance(&mut self, object: usize, rec: Rec) -> Result<Rc<RefCell<Instance>>, Signal> {
        let inst = self.instance(object);
        let (rs, xs) = (
            self.engine.program.objects[object].rec_slot,
            self.engine.program.objects[object].xrec_slot,
        );
        let copy = Value::Record(Rc::new(RefCell::new(rec.borrow().clone())));
        if let Some(r) = rs {
            inst.borrow_mut().globals[r] = Value::Record(rec);
        }
        if let Some(x) = xs {
            inst.borrow_mut().globals[x] = copy;
        }
        Ok(inst)
    }

    fn run_trigger(
        &mut self,
        object: usize,
        routine: usize,
        rec: &Rec,
        xrec: Option<Vec<Value>>,
    ) -> Result<(), Signal> {
        let inst = self.record_instance(object, rec.clone())?;
        if let (Some(x), Some(fields)) = (self.engine.program.objects[object].xrec_slot, xrec) {
            if let Value::Record(xr) = &inst.borrow().globals[x] {
                xr.borrow_mut().fields = fields;
            }
        }
        self.call(object, routine, inst, Vec::new()).map(|_| ())
    }

    /// Every write gets a new, increasing SystemRowVersion.
    fn bump_row_version(&mut self, t: &TableDef, rec: &Rec) {
        if let Some(i) = t.fields.iter().position(|f| f.id == 2_000_000_005) {
            self.next_row_version += 1;
            rec.borrow_mut().fields[i] = Value::BigInteger(self.next_row_version);
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
        let key = rec.borrow().key;
        // Filters on FlowFilter fields drop out; those on FlowFields are checked per row.
        let (filters, flow_field_filters) = self.split_filters(rec);
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

    /// Reads a row into the record, then calculates its `SetAutoCalcFields` fields.
    fn load(&mut self, rec: &Rec, row: Vec<Value>) -> Result<(), Signal> {
        rec.borrow_mut().fields = row;
        self.auto_calculate(rec)
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
                r.key = 0;
                r.ascending = true;
                r.position = None;
                Ok(Value::Null)
            }
            "Insert" => {
                self.check_writable(&t, "Insert")?;
                // A temporary record has no sequence: the key stays as it is (corpus
                // record/TestTempTableContracts.al:52).
                let auto = if rec.borrow().temporary {
                    None
                } else {
                    self.auto_increment_field(&t)?
                };
                if arg_bool(args, 0)? {
                    if let Some(tr) = self.table_trigger(&t, "OnInsert") {
                        self.run_trigger(t.object as usize, tr, rec, None)?;
                    }
                }
                self.assign_system_id(&t, rec);
                self.bump_row_version(&t, rec);
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
                    if let Some(a) = auto {
                        if taken {
                            rec.borrow_mut().fields[a] = self.default_value(&t.fields[a].ty);
                        } else {
                            self.note_explicit_key(&t, a, &row);
                        }
                    }
                    return self.fail_unique(&t);
                }
                let stored = self.stored_view(&t, &row);
                match self
                    .store_for(rec)
                    .insert(&cx, &t, stored.clone())
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                {
                    InsertOutcome::Inserted => {
                        if let (Some(a), false) = (auto, taken) {
                            self.note_explicit_key(&t, a, &row);
                        }
                        Ok(Value::Boolean(true))
                    }
                    InsertOutcome::AlreadyExists => {
                        // The value stays consumed; the key it took does not stay on the record.
                        if let (Some(a), true) = (auto, taken) {
                            rec.borrow_mut().fields[a] = self.default_value(&t.fields[a].ty);
                        }
                        self.fail(&t, &row, true)
                    }
                }
            }
            "Modify" => {
                self.check_writable(&t, "Modify")?;
                self.check_unique_keys(&t)?;
                let pk = t.primary_key(&rec.borrow().fields);
                let stored = self
                    .store_for(rec)
                    .get(&cx, &t, &pk)
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?;
                if arg_bool(args, 0)? {
                    if let Some(tr) = self.table_trigger(&t, "OnModify") {
                        self.run_trigger(t.object as usize, tr, rec, stored)?;
                    }
                }
                self.bump_row_version(&t, rec);
                let row = rec.borrow().fields.clone();
                let stored = self.stored_view(&t, &row);
                match self
                    .store_for(rec)
                    .modify(&cx, &t, stored.clone())
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                {
                    WriteOutcome::Done => Ok(Value::Boolean(true)),
                    WriteOutcome::NotFound => self.fail(&t, &row, false),
                }
            }
            "Delete" => {
                self.check_writable(&t, "Delete")?;
                if arg_bool(args, 0)? {
                    if let Some(tr) = self.table_trigger(&t, "OnDelete") {
                        let x = Some(rec.borrow().fields.clone());
                        self.run_trigger(t.object as usize, tr, rec, x)?;
                    }
                }
                let row = rec.borrow().fields.clone();
                let pk = t.primary_key(&row);
                match self
                    .store_for(rec)
                    .delete(&cx, &t, &pk)
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                {
                    WriteOutcome::Done => Ok(Value::Boolean(true)),
                    WriteOutcome::NotFound => self.fail(&t, &row, false),
                }
            }
            "DeleteAll" => {
                self.check_writable(&t, "DeleteAll")?;
                if arg_bool(args, 0)? && self.table_trigger(&t, "OnDelete").is_some() {
                    return unsupported("DeleteAll(true) with an OnDelete trigger");
                }
                let (filters, flow_field_filters) = self.split_filters(rec);
                if !flow_field_filters.is_empty() {
                    return unsupported("DeleteAll with a filter on a FlowField");
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
            "Count" | "IsEmpty" => {
                let (filters, flow_field_filters) = self.split_filters(rec);
                let n = if flow_field_filters.is_empty() {
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
                let engine = self.engine.clone();
                let enums = |id: u32| engine.program.objects[id as usize].enum_values.clone();
                let expr = al_builtins::filter::parse_filter_with(&text, &args[2..], &ty, &enums)
                    .map_err(|e| Signal::Unsupported(format!("SetFilter: {e}")))?;
                let mut r = rec.borrow_mut();
                r.filters.retain(|f| f.field != pos);
                r.filters.push(FieldFilter { field: pos, expr });
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
                self.check_writable(&t, "Validate")?;
                if field.relation {
                    return unsupported("Validate of a field with a TableRelation");
                }
                // A Validate nested in an OnValidate of the same record keeps the outer call's
                // before-image as xRec (corpus record/TestXRecContracts.al:133).
                let handle = Rc::as_ptr(rec) as usize;
                let outer = self
                    .validating
                    .iter()
                    .find(|(h, _)| *h == handle)
                    .map(|(_, b)| b.clone());
                let before = outer.clone().unwrap_or_else(|| rec.borrow().fields.clone());
                if let Some(v) = args.get(1) {
                    let v = ops::convert(v.clone(), &field.ty)?;
                    rec.borrow_mut().fields[pos] = v;
                }
                if let Some((object, routine)) = field.on_validate {
                    if outer.is_none() {
                        self.validating.push((handle, before.clone()));
                    }
                    let r = self.run_trigger(object, routine, rec, Some(before));
                    if outer.is_none() {
                        self.validating.pop();
                    }
                    r?;
                }
                Ok(Value::Null)
            }
            "Rename" => {
                self.check_writable(&t, "Rename")?;
                self.check_unique_keys(&t)?;
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
                self.check_rename_auto_increment(&t, &old, &new)?;
                rec.borrow_mut().fields = new.clone();
                if let Some(tr) = self.table_trigger(&t, "OnRename") {
                    self.run_trigger(t.object as usize, tr, rec, Some(old.clone()))?;
                }
                self.bump_row_version(&t, rec);
                let row = rec.borrow().fields.clone();
                let old_pk = t.primary_key(&old);
                let stored = self.stored_view(&t, &row);
                match self
                    .store_for(rec)
                    .rename(&cx, &t, &old_pk, stored.clone())
                    .map_err(|e| Signal::Unsupported(format!("{e:?}")))?
                {
                    RenameOutcome::Renamed => Ok(Value::Boolean(true)),
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
            other => unsupported(format!("Record.{other}")),
        }
    }
}

#[allow(dead_code)]
fn _uses(_: RtType, a: &[Value], b: &[Value]) -> std::cmp::Ordering {
    key_cmp(a, b)
}
