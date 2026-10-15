//! `RecordRef`, `FieldRef` and `KeyRef` on the VM.
//!
//! An open `RecordRef` owns a [`RecordState`] exactly as a `Record` variable
//! does, so every data operation is the same code path as the record
//! methods in `records.rs` (triggers included). `FieldRef` and `KeyRef` point
//! into that buffer by field position and key index. Behaviour the corpus
//! does not pin raises `Signal::Unsupported` instead of guessing.

use crate::Session;
use al_builtins::catalog::Catalog;
use al_builtins::describe::Describe;
use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::value::{LibraryValue, RecordState, RtType, Value};
use al_data::TableDef;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::OnceLock;

type Rec = Rc<RefCell<RecordState>>;
type Lib = Rc<RefCell<LibraryValue>>;
/// Whether a field is an Enum (not an Option) and its members as (ordinal, name, caption).
type FieldMembers = (bool, Vec<(i32, String, String)>);

/// First number of the platform's system fields (`SystemId`, ...); they are
/// not counted by `FieldCount` (docs/probes-needed.md).
const SYSTEM_FIELD_BASE: u32 = 2_000_000_000;

/// The field number AL code sees for a field. SystemRowVersion is field 0 there: the corpus pins that
/// `KeyRef.FieldIndex(n).Number()` equals `FieldNo(SystemRowVersion)`
/// (recordref/TestKeyMetadataRowVersionRemoved.al:51, :85) and that the rowversion "sits at field id 0"
/// (record/TestCalcFormulaSystemFieldsTests.Codeunit.al:213-215); a real project reads
/// `KeyRef.FieldIndex(1).Number = 0` for a key that starts with it ("FieldRef 0 is rowversion").
/// No corpus test names the number itself: docs/probes-needed.md, batch 7. The other system fields
/// keep 2000000000 to 2000000004.
pub(crate) fn al_field_no(id: u32) -> i32 {
    if id == SYSTEM_ROW_VERSION_ID { 0 } else { id as i32 }
}

/// The internal id of SystemRowVersion, which AL code sees as field 0.
pub(crate) const SYSTEM_ROW_VERSION_ID: u32 = 2_000_000_005;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn catalog() -> &'static Catalog {
    static C: OnceLock<Catalog> = OnceLock::new();
    C.get_or_init(Catalog::load)
}

fn handle(name: &str, v: LibraryValue) -> Value {
    Value::Library(name.into(), Rc::new(RefCell::new(v)))
}

/// A `RecordRef` handle on a copy of the record `rec` (what an event hands its subscriber).
pub(crate) fn record_ref_value(rec: &crate::records::Rec) -> Value {
    let copy = rec.borrow().clone();
    handle("RecordRef", LibraryValue::RecordRef(Rc::new(RefCell::new(copy))))
}

fn al_error(message: String) -> Signal {
    Signal::Error(AlError {
        message,
        code: "Dialog".into(),
        call_stack: String::new(),
    })
}

fn int_arg(args: &[Value], i: usize) -> Result<i64, Signal> {
    match args.get(i) {
        Some(Value::Integer(n)) => Ok(i64::from(*n)),
        Some(Value::BigInteger(n)) => Ok(*n),
        other => unsupported(format!("integer argument {i}: {}", other.describe())),
    }
}

/// The fields of key `k` of a RecordRef: a declared key, or the implicit key on `SystemId` that
/// follows them (corpus recordref/TestRecordRefRuntimeContracts.al:KeyRefAndFieldRef_Active_AreTrue_KeyCount
/// pins two keys for a table that declares only its primary key; tableextension/
/// TestTableExtKeysAndInitValue.al names it the implicit SystemId key).
fn key_fields(t: &TableDef, k: usize) -> Option<Vec<usize>> {
    if let Some(f) = t.keys.get(k) {
        return Some(f.clone());
    }
    if k == t.keys.len() {
        return t.fields.iter().position(|f| f.id == SYSTEM_FIELD_BASE).map(|p| vec![p]);
    }
    None
}

/// The table as the program declares it: without the ad hoc key a record may have appended.
impl Session {
    fn base_table(&self, t: &TableDef) -> std::sync::Arc<TableDef> {
        self.engine.tables[t.object as usize]
            .clone()
            .expect("a record's table has a schema")
    }
}

fn key_count(t: &TableDef) -> usize {
    t.keys.len() + usize::from(key_fields(t, t.keys.len()).is_some())
}

/// A table's fields a caller can enumerate (`FieldCount`, `FieldIndex`): in
/// field-number order, system fields left out.
fn enumerable_fields(s: &Session, t: &TableDef) -> Vec<usize> {
    let mut v: Vec<usize> = (0..t.fields.len())
        .filter(|&i| t.fields[i].id < SYSTEM_FIELD_BASE && s.field_enabled(t, i) && !s.field_removed(t, i))
        .collect();
    v.sort_by_key(|&i| t.fields[i].id);
    v
}

impl Session {
    /// A field without `Enabled = false` (record/TestRowVersionFieldNumberFieldActiveContracts.al:
    /// a disabled field is not counted, not listed, not `FieldExist` and not `FieldActive`).
    /// A field with `ObsoleteState = Removed`: RecordRef does not count it, list it or find it
    /// (recordref/PWRecordMisc.al:109-167, BC 27.5).
    pub(crate) fn field_removed(&self, t: &TableDef, pos: usize) -> bool {
        self.field_property(t, pos, "ObsoleteState")
            .is_some_and(|v| v.trim().eq_ignore_ascii_case("Removed"))
    }

    pub(crate) fn field_enabled(&self, t: &TableDef, pos: usize) -> bool {
        self.field_property(t, pos, "Enabled")
            .is_none_or(|v| !v.trim().eq_ignore_ascii_case("false"))
    }

    pub(crate) fn ref_op(&mut self, id: &str, recv: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
        let Some(sig) = catalog().by_id(id) else {
            return unsupported(format!("builtin {id}"));
        };
        let lib: Lib = match recv {
            Some(Value::Library(_, l)) => l.clone(),
            other => return unsupported(format!("{id} on {}", other.describe())),
        };
        match sig.receiver.as_str() {
            "RecordRef" => self.recordref_op(&sig.name, &lib, args),
            "FieldRef" => self.fieldref_op(&sig.name, &lib, args),
            "KeyRef" => self.keyref_op(&sig.name, &lib, args),
            // GetRecord answers a new RecordRef on the RecordId's table with the primary key set and
            // nothing else loaded; the row need not exist; a null RecordId raises (recordref/
            // TestRecordIdGetRecordContracts.al).
            "RecordId" if sig.name == "GetRecord" => {
                let (table, key) = match &*lib.borrow() {
                    LibraryValue::RecordId { table, key, .. } => (*table, key.clone()),
                    _ => {
                        return Err(Signal::Error(al_builtins::native::AlError {
                            message: "The record is not open.".into(),
                            code: "RecordNotOpened".into(),
                            call_stack: String::new(),
                        }));
                    }
                };
                let Value::Record(rec) = self.default_value(&RtType::Record(table, false)) else {
                    return unsupported("record buffer for RecordId.GetRecord");
                };
                let t = self.table(&rec)?;
                for (&p, v) in t.keys[0].iter().zip(key) {
                    rec.borrow_mut().fields[p] = v;
                }
                Ok(handle("RecordRef", LibraryValue::RecordRef(rec)))
            }
            "RecordId" => match (&*lib.borrow(), sig.name.as_str()) {
                (LibraryValue::RecordId { table_no, .. }, "TableNo") => Ok(Value::Integer(*table_no as i32)),
                (LibraryValue::Empty, "TableNo") => Ok(Value::Integer(0)),
                (_, other) => unsupported(format!("RecordId.{other}")),
            },
            other => unsupported(format!("{other} methods")),
        }
    }

    fn open_ref(lib: &Lib, what: &str) -> Result<Rec, Signal> {
        match &*lib.borrow() {
            LibraryValue::RecordRef(r)
            | LibraryValue::FieldRef(r, _)
            | LibraryValue::KeyFieldRef(r, _)
            | LibraryValue::KeyRef(r, _) => Ok(r.clone()),
            LibraryValue::SystemTable(n) => unsupported(format!(
                "{what} on a RecordRef opened on system table {n} (the platform's system tables are not modelled)"
            )),
            // BC's answer is not pinned by the corpus: the error text, and whether `Get(RecordId)` opens the
            // table of the RecordId (Base Application's Data Type Management does `if not Get(RecID) then
            // Open(RecID.TableNo)`). See docs/probes-needed.md.
            _ => unsupported(format!(
                "{what} on a RecordRef that is not open: BC's error text, and whether Get(RecordId) opens the table of the RecordId, are not pinned (docs/probes-needed.md)"
            )),
        }
    }

    pub(crate) fn table_object(&self, no: i64) -> Option<usize> {
        let no = u32::try_from(no).ok()?;
        self.engine
            .program
            .objects
            .iter()
            .position(|o| o.table.is_some() && o.id == Some(no))
    }

    /// `RecordRef.Open(No, Temp, CompanyName)`.
    fn rr_open(&mut self, lib: &Lib, args: &[Value]) -> Result<Value, Signal> {
        let no = int_arg(args, 0)?;
        let temp = match args.get(1) {
            Some(v) => ops::truthy(v)?,
            None => false,
        };
        let company = match args.get(2) {
            None => None,
            Some(c) => {
                let name = al_builtins::native::text_of(c).unwrap_or_default();
                match self.resolve_company(&name)? {
                    Some(company) => company,
                    None => return Err(Self::company_missing(&name)),
                }
            }
        };
        // corpus recordref/TestRecordRefOpenCompilationTarget.al:27-35: an internal system table is
        // refused for a Cloud-target app (this app is Cloud), whether or not a dependency app
        // declares an object with that number (the platform's `System.app` does, as
        // `Object Metadata`; a complete compile of the dependencies must not change the answer).
        if no == 2_000_000_071 {
            return Err(al_error(format!(
                "You cannot open record {no} from a RecordRef data type when you are using target Cloud."
            )));
        }
        let Some(table) = self.table_object(no) else {
            return match no {
                // :37-62: these system tables open for a Cloud-target app and report their number.
                2_000_000_026 | 2_000_000_187 | 2_000_000_188 => {
                    *lib.borrow_mut() = LibraryValue::SystemTable(no);
                    Ok(Value::Null)
                }
                n if n >= i64::from(SYSTEM_FIELD_BASE) => unsupported(format!(
                    "RecordRef.Open on system table {n} (whether a Cloud target may open it is not pinned)"
                )),
                // A table of a dependency app that nothing reached is not compiled (ADR-013); BC has it.
                n if self
                    .engine
                    .program
                    .objects
                    .iter()
                    .any(|o| o.kind == "uncompiled table" && i64::from(o.id.unwrap_or(0)) == n) =>
                {
                    unsupported(format!(
                        "RecordRef.Open on table {n}, which belongs to a dependency app and was not compiled (no code the workspace reaches uses it)"
                    ))
                }
                // corpus recordref/TestRecordRefOpenMissingTable.al:31-49 pins both halves; the full
                // text and the code AL:MetadataNotFound: recordref/TestRecordRefRuntimeContracts.al:30.
                n => Err(self.missing_object("Table", n)),
            };
        };
        // A table of TableType CRM can only be opened through a registered table connection; with none
        // registered the open fails (session/TestCrmTableNoConnectionContracts.al:111-118, BC 27.5).
        // With one registered, what the open does is not pinned. Other table types are not pinned.
        let table_type = self.engine.program.objects[table]
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("TableType"))
            .map(|(_, v)| v.trim().to_ascii_lowercase());
        match table_type.as_deref() {
            None | Some("normal") | Some("temporary") => {}
            Some("crm") if !self.table_connections.iter().any(|(kind, _)| *kind == 0) => {
                return Err(crate::transactions::al_error_code(
                    "Table connection  for table type CRM must be registered using RegisterTableConnection or cmdlet New-NAVTableConnection before it can be used.",
                    "Argument",
                ));
            }
            Some(other) => {
                return unsupported(format!(
                    "RecordRef.Open of a table with TableType = {other} (a table connection is not modelled)"
                ));
            }
        }
        let Value::Record(rec) = self.default_value(&RtType::Record(table as u32, temp)) else {
            return unsupported("record buffer for RecordRef.Open");
        };
        if company.is_some() {
            if temp {
                return unsupported("RecordRef.Open of a temporary record on another company (not pinned)");
            }
            rec.borrow_mut().company = company;
        }
        *lib.borrow_mut() = LibraryValue::RecordRef(rec);
        Ok(Value::Null)
    }

    fn recordref_op(&mut self, method: &str, lib: &Lib, args: &mut [Value]) -> Result<Value, Signal> {
        match method {
            "Open" => return self.rr_open(lib, args),
            // Close and Clear leave the ref unusable; closing an unopened ref is harmless.
            "Close" => {
                *lib.borrow_mut() = LibraryValue::Empty;
                return Ok(Value::Null);
            }
            // Doc (RecordRef.GetTable): the ref takes the record variable's table, buffer and
            // filters. The corpus pins the other half: the Record is left alone
            // (recordref/TestRecordRefGetSetTableContracts.al:30-60).
            // A ref that was never opened takes its table from the record (Assert.RecordIsEmpty does this).
            "GetTable" => {
                let Some(Value::Record(src)) = args.first() else {
                    return unsupported(format!("RecordRef.GetTable of {}", args.first().describe()));
                };
                let copy = src.borrow().clone();
                *lib.borrow_mut() = LibraryValue::RecordRef(Rc::new(RefCell::new(copy)));
                return Ok(Value::Null);
            }

            "Number" => match &*lib.borrow() {
                LibraryValue::SystemTable(n) => return Ok(Value::Integer(*n as i32)),
                // A RecordRef never opened is table 0 (corpus branch al-runtime-rs-staging, ae3fe88,
                // tests/al-language/types/PXEnumRecordRef.al:293-299).
                LibraryValue::Empty => return Ok(Value::Integer(0)),
                _ => {}
            },
            // A RecordRef never opened answers false (PXEnumRecordRef.al:293-298); with the result
            // unused nothing pins what it raises.
            "GetBySystemId" if matches!(&*lib.borrow(), LibraryValue::Empty) => {
                return if self.ret_used {
                    Ok(Value::Boolean(false))
                } else {
                    unsupported(
                        "RecordRef.GetBySystemId on a RecordRef that is not open with the result unused: what BC raises is not pinned (docs/probes-needed.md)",
                    )
                };
            }
            _ => {}
        }
        let rec = Self::open_ref(lib, &format!("RecordRef.{method}"))?;
        let t = self.table(&rec)?;
        match method {
            "Number" => Ok(Value::Integer(
                self.engine.program.objects[t.object as usize].id.unwrap_or(0) as i32,
            )),
            "Name" => Ok(Value::Text(t.name.clone())),
            "Caption" => Ok(Value::Text(self.caption(&t))),
            // corpus recordref/TestRecordRefGetSet.al:16-30 pins only that it is not empty.
            "CurrentCompany" => Ok(Value::Text(self.current_company_of(&rec))),
            "ChangeCompany" => self.change_company(&rec, args),
            "IsTemporary" => Ok(Value::Boolean(rec.borrow().temporary)),
            "HasFilter" => Ok(Value::Boolean(Self::has_filter(&rec))),
            "FilterGroup" => {
                al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::RecordRefFilterGroup);
                Self::filter_group(&rec, args)
            }
            "GetFilters" => self.get_filters(&rec, &t).map(Value::Text),
            "GetView" => {
                let use_names = match args.first() {
                    Some(v) => ops::truthy(v)?,
                    None => true,
                };
                self.get_view(&rec, &t, use_names).map(Value::Text)
            }
            "SetView" => {
                let view = al_builtins::native::text_of(args.first().unwrap_or(&Value::Null)).unwrap_or_default();
                self.set_view(&rec, &t, &view).map(|()| Value::Null)
            }
            // The load set of a RecordRef is given by field numbers (record/TestRecordPartialLoad.al
            // PartialLoad_SetLoadFields_UnknownFieldNo_Throws: a number no field has is an error that
            // names it; its text is not pinned). The rest is the Record rule (partial.rs).
            "SetLoadFields" | "AddLoadFields" => {
                let mut positions = Vec::with_capacity(args.len());
                for a in args.iter() {
                    let Value::Integer(n) = a else {
                        return unsupported(format!("RecordRef.{method} with {}", a.describe()));
                    };
                    match t.fields.iter().position(|f| i64::from(f.id) == i64::from(*n)) {
                        Some(p) => positions.push(Value::Integer(p as i32)),
                        None => return Err(Signal::Error(al_builtins::native::unpinned_error())),
                    }
                }
                self.load_fields_op(method, &rec, &t, &positions)
            }
            // Doc (RecordRef.Duplicate): a new ref with the same table, buffer and filters, independent of the original.
            "RecordId" => Ok(self.record_id(&rec, &t)),
            "LockTable" | "ReadConsistency" | "RecordLevelLocking" | "ReadPermission" | "WritePermission"
            | "CountApprox" | "ReadIsolation" | "SecurityFiltering" | "GetPosition" | "SetPosition" | "Ascending"
            | "SetRecFilter" => self.record_meta_op(method, &rec, &t, args),
            // Doc (RecordRef.Get): loads the row a RecordId names; the table must be the ref's own.
            "Get" => {
                let key = match args.first() {
                    Some(Value::Library(_, l)) => match &*l.borrow() {
                        LibraryValue::RecordId { table, key, .. } if *table == t.object => key.clone(),
                        LibraryValue::RecordId { .. } => {
                            return unsupported(
                                "RecordRef.Get of a RecordId of another table (error text not pinned yet)",
                            );
                        }
                        other => return unsupported(format!("RecordRef.Get of {}", other.describe())),
                    },
                    other => return unsupported(format!("RecordRef.Get of {}", other.describe())),
                };
                let mut a = key;
                self.record_op("Get", &rec, &mut a)
            }
            // As Record.GetBySystemId, and a miss with the result unused raises "The <table> does not
            // exist" naming the Guid (PXEnumRecordRef.al:199-301).
            "GetBySystemId" => self.get_by_system_id(&rec, &t, args, true),
            "AddLink" | "DeleteLink" | "DeleteLinks" | "HasLinks" | "CopyLinks" => {
                self.record_links(method, &rec, &t, args)
            }
            "Duplicate" => {
                if rec.borrow().temporary {
                    return unsupported("Duplicate of a temporary RecordRef (shared table not pinned)");
                }
                let copy = rec.borrow().clone();
                Ok(handle(
                    "RecordRef",
                    LibraryValue::RecordRef(Rc::new(RefCell::new(copy))),
                ))
            }
            "SystemIdNo" | "SystemCreatedAtNo" | "SystemCreatedByNo" | "SystemModifiedAtNo" | "SystemModifiedByNo" => {
                let field = method.trim_end_matches("No").replace("SystemIdNo", "SystemId");
                match t.fields.iter().find(|f| f.name == field) {
                    Some(f) => Ok(Value::Integer(f.id as i32)),
                    None => unsupported(format!("RecordRef.{method} on a table without {field}")),
                }
            }
            "IsEmpty" | "Count" | "Init" | "Insert" | "Modify" | "Delete" | "DeleteAll" | "Find" | "FindFirst"
            | "FindLast" | "FindSet" | "Next" | "Reset" | "Rename" => self.record_op(method, &rec, args),
            "Field" => {
                let no = int_arg(args, 0)?;
                match t.fields.iter().position(|f| i64::from(al_field_no(f.id)) == no) {
                    // A removed field raises the obsolete error with its reason (recordref/PWRecordMisc.al:128-167).
                    Some(pos) if self.field_removed(&t, pos) => {
                        let Some(reason) = self
                            .field_property(&t, pos, "ObsoleteReason")
                            .and_then(|r| crate::recmeta::string_literal(&r))
                        else {
                            return unsupported(format!(
                                "RecordRef.Field({no}) of a removed field without an ObsoleteReason (the text is not pinned)"
                            ));
                        };
                        let table_id = self.engine.program.objects[t.object as usize].id.unwrap_or(0);
                        Err(crate::transactions::al_error_code(
                            &format!(
                                "Field {} ({no}) of table {} ({table_id}) is obsoleted, reason: {reason}",
                                t.fields[pos].name, t.name
                            ),
                            "MetadataObsolete",
                        ))
                    }
                    Some(pos) => Ok(handle("FieldRef", LibraryValue::FieldRef(rec, pos))),
                    None => unsupported(format!(
                        "RecordRef.Field({no}) on a field {} does not have (error text not pinned yet)",
                        t.name
                    )),
                }
            }
            "FieldExist" => {
                let no = int_arg(args, 0)?;
                // Field 0 (SystemRowVersion) and a disabled field do not exist
                // (record/TestRowVersionFieldNumberFieldActiveContracts.al:73-75, 120-121).
                Ok(Value::Boolean(t.fields.iter().enumerate().any(|(i, f)| {
                    i64::from(f.id) == no
                        && f.id != SYSTEM_ROW_VERSION_ID
                        && self.field_enabled(&t, i)
                        && !self.field_removed(&t, i)
                })))
            }
            "FieldCount" => Ok(Value::Integer(enumerable_fields(self, &t).len() as i32)),
            "FieldIndex" => {
                let i = int_arg(args, 0)?;
                // Past the count the index reaches the system fields in number order: $systemId,
                // SystemCreatedAt, SystemCreatedBy, SystemModifiedAt, SystemModifiedBy (corpus
                // recordref/TestRecordRefRuntimeContracts.al:RecordRef_FieldCount_ExcludesSystemFields_...);
                // what follows them is not pinned.
                let normal = enumerable_fields(self, &t);
                let reached = usize::try_from(i - 1).ok().and_then(|i| match normal.get(i) {
                    Some(&p) => Some(p),
                    None => {
                        let id = SYSTEM_FIELD_BASE + (i - normal.len()) as u32;
                        (id <= SYSTEM_FIELD_BASE + 4)
                            .then(|| t.fields.iter().position(|f| f.id == id))
                            .flatten()
                    }
                });
                match reached {
                    Some(pos) => Ok(handle("FieldRef", LibraryValue::FieldRef(rec, pos))),
                    // record/TestRowVersionFieldNumberFieldActiveContracts.al:84-86.
                    None => Err(Signal::Error(al_builtins::native::AlError {
                        message: "Index out of bounds.".into(),
                        code: "IndexOutOfBounds".into(),
                        call_stack: String::new(),
                    })),
                }
            }
            "KeyCount" => Ok(Value::Integer(key_count(&self.base_table(&t)) as i32)),
            "CurrentKey" => self.record_meta_op("CurrentKey", &rec, &t, args),
            // recordref/TestRecordRefKeyIndexSetRecFilterContracts.al:75-176. Keys number from 1 in
            // declaration order and the hidden SystemId key is last; an index outside them raises
            // DB:KeyNotFound; a current key of fields no declared key starts with reports -1.
            "CurrentKeyIndex" => {
                let bt = self.base_table(&t);
                let base = bt.keys.len();
                if let Some(v) = args.first() {
                    let i = int_arg(std::slice::from_ref(v), 0)?;
                    let count = key_count(&bt) as i64;
                    if i < 1 || i > count {
                        return Err(Signal::Error(AlError {
                            message: format!("The {} table does not have an active key at index {i}.", t.name),
                            code: "DB:KeyNotFound".into(),
                            call_stack: String::new(),
                        }));
                    }
                    let mut r = rec.borrow_mut();
                    if (i as usize) <= base {
                        r.key = i as usize - 1;
                        r.adhoc = None;
                    } else {
                        let fields = key_fields(&bt, base).expect("a counted key");
                        r.key = base;
                        r.adhoc = Some((base, fields));
                    }
                    return Ok(Value::Integer(i as i32));
                }
                let r = rec.borrow();
                Ok(Value::Integer(match &r.adhoc {
                    _ if r.key < base => r.key as i32 + 1,
                    Some((_, f)) if r.key == base && key_fields(&bt, base).as_ref() == Some(f) => base as i32 + 1,
                    _ => -1,
                }))
            }
            "KeyIndex" => {
                let i = int_arg(args, 0)?;
                match usize::try_from(i - 1)
                    .ok()
                    .filter(|k| *k < key_count(&self.base_table(&t)))
                {
                    Some(k) => Ok(handle("KeyRef", LibraryValue::KeyRef(rec, k))),
                    None => unsupported(format!(
                        "RecordRef.KeyIndex({i}) out of range (error text not pinned yet)"
                    )),
                }
            }
            // Doc (RecordRef.SetTable): the record variable takes the ref's buffer and
            // filters. The corpus pins that the Record's old buffer does not flow into the
            // ref (recordref/TestRecordRefGetSetTableContracts.al:78-104).
            "SetTable" => {
                let Some(Value::Record(dst)) = args.first() else {
                    return unsupported(format!("RecordRef.SetTable of {}", args.first().describe()));
                };
                let (same_table, same_kind) = {
                    let (s, d) = (rec.borrow(), dst.borrow());
                    (s.table == d.table, s.temporary == d.temporary)
                };
                if !same_table || !same_kind {
                    return unsupported("RecordRef.SetTable into a record of another table or temporary-ness");
                }
                if !Rc::ptr_eq(&rec, dst) {
                    let s = rec.borrow();
                    // SetTable does not move the target either (:123-127); GetTable takes the company.
                    if s.field_descending {
                        return unsupported("Copy of a record with a per-field SetAscending (not pinned)");
                    }
                    let mut d = dst.borrow_mut();
                    d.fields = s.fields.clone();
                    d.filters = s.filters.clone();
                    d.key = s.key;
                    d.adhoc = s.adhoc.clone();
                    d.ascending = s.ascending;
                    d.position = s.position.clone();
                    d.load.buffer = s.load.buffer.clone();
                    d.load.partial = s.load.partial;
                }
                Ok(Value::Null)
            }
            other => unsupported(format!("RecordRef.{other}")),
        }
    }

    fn keyref_op(&mut self, method: &str, lib: &Lib, args: &mut [Value]) -> Result<Value, Signal> {
        let (rec, k) = match &*lib.borrow() {
            LibraryValue::KeyRef(r, k) => (r.clone(), *k),
            _ => return unsupported(format!("KeyRef.{method} on a KeyRef that was never assigned")),
        };
        let t = self.table(&rec)?;
        match method {
            // Every key a table declares is enabled: the lowerer does not read key
            // properties, and no corpus key sets Enabled = false.
            "Active" => Ok(Value::Boolean(true)),
            "FieldCount" => Ok(Value::Integer(key_fields(&t, k).map_or(0, |f| f.len()) as i32)),
            "Record" => Ok(handle("RecordRef", LibraryValue::RecordRef(rec))),
            "FieldIndex" => {
                let i = int_arg(args, 0)?;
                match usize::try_from(i - 1)
                    .ok()
                    .and_then(|i| key_fields(&t, k).and_then(|f| f.get(i).copied()))
                {
                    Some(pos) => Ok(handle("FieldRef", LibraryValue::KeyFieldRef(rec, pos))),
                    // types/TestTypeSystemContracts.al:261 pins the fragment "Index out of bounds"
                    // (recordref/TestBCSumIndexFieldContracts.al:188-198 pins that it throws).
                    None => Err(Signal::Error(AlError {
                        message: "Index out of bounds".into(),
                        code: String::new(),
                        call_stack: String::new(),
                    })),
                }
            }
            other => unsupported(format!("KeyRef.{other}")),
        }
    }

    fn fieldref_op(&mut self, method: &str, lib: &Lib, args: &mut [Value]) -> Result<Value, Signal> {
        let (rec, pos) = match &*lib.borrow() {
            LibraryValue::FieldRef(r, p) | LibraryValue::KeyFieldRef(r, p) => (r.clone(), *p),
            _ => {
                return unsupported(format!(
                    "FieldRef.{method} on a FieldRef that was never assigned: what BC does (an error, or a no-op) is not pinned (docs/probes-needed.md)"
                ));
            }
        };
        let t = self.table(&rec)?;
        let field = &t.fields[pos];
        match method {
            "Value" => match args.first() {
                None => Ok(rec.borrow().fields[pos].clone()),
                Some(v) => {
                    // Assigning Value does not run OnValidate (that is `Validate`).
                    let v = ops::convert(v.clone(), &field.ty)?;
                    rec.borrow_mut().fields[pos] = v;
                    Ok(Value::Null)
                }
            },
            "Number" => Ok(Value::Integer(al_field_no(field.id))),
            // SystemId is `$systemId` and the rowversion `timestamp` to a FieldRef (record/
            // TestRowVersionFieldNumberFieldActiveContracts.al:70-72, 82-84).
            "Name" if field.id == SYSTEM_FIELD_BASE => Ok(Value::Text("$systemId".into())),
            "Name" if field.id == SYSTEM_ROW_VERSION_ID => Ok(Value::Text("timestamp".into())),
            "Name" => Ok(Value::Text(field.name.clone())),
            "Caption" => self.field_caption_checked(&t, pos).map(Value::Text),
            "Type" => self.field_type(&t, pos),
            "Class" => {
                let class = self
                    .field_property(&t, pos, "FieldClass")
                    .unwrap_or_default()
                    .trim()
                    .to_ascii_lowercase();
                let ord = match class.as_str() {
                    "flowfield" => 1,
                    "flowfilter" => 2,
                    "" | "normal" => 0,
                    other => return unsupported(format!("FieldClass `{other}`")),
                };
                let m = al_builtins::sysenum::option_members("FieldClass").expect("known");
                Ok(Value::Option(ord, m.into()))
            }
            "Relation" => self.field_relation(&rec, &t, pos).map(Value::Integer),
            "GetFilter" => self.field_filter_text(&rec, pos).map(Value::Text),
            "Record" => Ok(handle("RecordRef", LibraryValue::RecordRef(rec))),
            // fieldref/TestRelationTargetNamespaceScope.al:63-75. Record.CalcFields raises a pinned
            // error for a field that is not a FlowField (record/TestCalcFieldsFieldClass.Codeunit.al);
            // whether FieldRef.CalcField says the same is not pinned, so it is refused.
            "CalcField" => {
                let stored = self.engine.program.objects[t.object as usize]
                    .table
                    .as_ref()
                    .is_some_and(|s| s.fields[pos].stored || s.fields[pos].flow_filter);
                if stored {
                    return unsupported("FieldRef.CalcField of a field that is not a FlowField (not pinned)");
                }
                self.flow_op("CalcFields", &rec, &[Value::Integer(pos as i32)])
            }
            "Validate" => {
                let mut a = vec![Value::Integer(pos as i32)];
                a.extend(args.iter().cloned());
                self.record_op("Validate", &rec, &mut a)
            }
            "TestField" | "FieldError" => {
                let mut a = vec![Value::Integer(pos as i32)];
                a.extend(args.iter().cloned());
                self.record_meta_op(method, &rec, &t, &mut a)
            }
            "IsOptimizedForTextSearch" => Ok(Value::Boolean(
                self.field_property(&t, pos, "OptimizeForTextSearch")
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case("true")),
            )),
            "IsEnum"
            | "EnumValueCount"
            | "GetEnumValueName"
            | "GetEnumValueCaption"
            | "GetEnumValueOrdinal"
            | "GetEnumValueNameFromOrdinalValue"
            | "GetEnumValueCaptionFromOrdinalValue"
            | "OptionCaption"
            | "OptionMembers"
            | "OptionString" => self.enum_metadata(method, &t, pos, args),
            "Active" => Ok(Value::Boolean(true)),
            "Length" => match &field.ty {
                RtType::Text(Some(n)) | RtType::Code(Some(n)) => Ok(Value::Integer(*n as i32)),
                other => unsupported(format!("FieldRef.Length of a {} field", other.describe())),
            },
            "SetRange" | "SetFilter" => {
                let mut a = vec![Value::Integer(pos as i32)];
                a.extend(args.iter().cloned());
                self.record_op(method, &rec, &mut a)
            }
            other => unsupported(format!("FieldRef.{other}")),
        }
    }
}

impl Session {
    /// `FieldRef.Type`. An Enum field is an Option field at run time (corpus fieldref/
    /// TestFieldRefEnumKeywordCase.al:95-106).
    fn field_type(&self, t: &TableDef, pos: usize) -> Result<Value, Signal> {
        let name = match &t.fields[pos].ty {
            RtType::Integer => "Integer",
            RtType::BigInteger => "BigInteger",
            RtType::Decimal => "Decimal",
            RtType::Boolean => "Boolean",
            RtType::Text(_) => "Text",
            RtType::Code(_) => "Code",
            RtType::Date => "Date",
            RtType::Time => "Time",
            RtType::DateTime => "DateTime",
            RtType::Duration => "Duration",
            RtType::Guid => "GUID",
            RtType::DateFormula => "DateFormula",
            RtType::Option(_) | RtType::Enum(_) => "Option",
            // The names are those of recordref/TestPlatformOptionsLockingContracts.al:46-63 (13 is
            // BLOB) and :66-76 (RecordID, TableFilter, MediaSet name themselves).
            RtType::Library(n) => match n.to_ascii_lowercase().as_str() {
                "blob" => "BLOB",
                "recordid" => "RecordID",
                "tablefilter" => "TableFilter",
                "media" => "Media",
                "mediaset" => "MediaSet",
                _ => return unsupported(format!("FieldRef.Type of a {} field", t.fields[pos].ty.describe())),
            },
            other => return unsupported(format!("FieldRef.Type of a {} field", other.describe())),
        };
        let members = al_builtins::sysenum::option_members("FieldType").expect("known");
        let ord = members.iter().position(|m| m == name).expect("listed") as i32;
        Ok(Value::Option(ord, members.into()))
    }

    /// A field property as written (`OptionCaption`, ...), if declared.
    pub(crate) fn field_property(&self, t: &TableDef, pos: usize, name: &str) -> Option<String> {
        self.engine.program.objects[t.object as usize]
            .table
            .as_ref()?
            .fields
            .get(pos)?
            .properties
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    }

    /// The members of an Option or Enum field as (ordinal, name, caption), in declared order.
    pub(crate) fn members_of_field(&self, t: &TableDef, pos: usize) -> Result<Option<FieldMembers>, Signal> {
        match &t.fields[pos].ty {
            RtType::Enum(id) => {
                let o = &self.engine.program.objects[*id as usize];
                let members = o
                    .enum_values
                    .iter()
                    .map(|(ord, name)| {
                        let caption = o
                            .enum_captions
                            .iter()
                            .find(|(c, _)| c == ord)
                            .map_or_else(|| name.clone(), |(_, c)| c.clone());
                        (*ord, name.clone(), caption)
                    })
                    .collect();
                Ok(Some((true, members)))
            }
            RtType::Option(names) => {
                let captions: Vec<String> = match self.field_property(t, pos, "OptionCaption") {
                    Some(v) => match crate::recmeta::string_literal(&v) {
                        Some(list) => list.split(',').map(str::to_string).collect(),
                        None => return unsupported(format!("OptionCaption `{v}`")),
                    },
                    None => names.clone(),
                };
                if captions.len() != names.len() {
                    return unsupported(
                        "an OptionCaption list as long as the OptionMembers list is not what this field has",
                    );
                }
                let members = names
                    .iter()
                    .zip(captions)
                    .enumerate()
                    .map(|(i, (n, c))| (i as i32, n.clone(), c))
                    .collect();
                Ok(Some((false, members)))
            }
            _ => Ok(None),
        }
    }

    /// `IsEnum`, `EnumValueCount`, `GetEnumValue*`, `OptionCaption/Members/String` of a FieldRef.
    /// Corpus: fieldref/TestFieldRefEnumOrdinalByIndex.al (index is a 1-based position, out of range
    /// answers -1 / ''), TestFieldRefEnum.al, TestPrecompiledFieldOptionCaption.al:34-48.
    fn enum_metadata(&self, method: &str, t: &TableDef, pos: usize, args: &[Value]) -> Result<Value, Signal> {
        let Some((is_enum, members)) = self.members_of_field(t, pos)? else {
            return match method {
                "IsEnum" => Ok(Value::Boolean(false)),
                "EnumValueCount" => Ok(Value::Integer(0)),
                // An empty text, not an error (corpus recordref/TestRecordRefRuntimeContracts.al:
                // FieldRef_EnumMetadata_OnEnumOptionAndOtherFields).
                "OptionCaption" | "OptionMembers" | "OptionString" => Ok(Value::Text(String::new())),
                other => unsupported(format!("FieldRef.{other} on a field that is neither Option nor Enum")),
            };
        };
        let at = |args: &[Value]| -> Result<Option<&(i32, String, String)>, Signal> {
            let i = int_arg(args, 0)?;
            Ok(usize::try_from(i - 1).ok().and_then(|i| members.get(i)))
        };
        let by_ordinal = |args: &[Value]| -> Result<&(i32, String, String), Signal> {
            let o = int_arg(args, 0)?;
            members.iter().find(|m| i64::from(m.0) == o).map_or_else(
                || unsupported(format!("{method}({o}) for an ordinal the field does not have")),
                Ok,
            )
        };
        match method {
            "IsEnum" => Ok(Value::Boolean(is_enum)),
            "EnumValueCount" => Ok(Value::Integer(members.len() as i32)),
            "GetEnumValueOrdinal" => Ok(Value::Integer(at(args)?.map_or(-1, |m| m.0))),
            "GetEnumValueName" => Ok(Value::Text(at(args)?.map(|m| m.1.clone()).unwrap_or_default())),
            "GetEnumValueCaption" => Ok(Value::Text(at(args)?.map(|m| m.2.clone()).unwrap_or_default())),
            "GetEnumValueNameFromOrdinalValue" => Ok(Value::Text(by_ordinal(args)?.1.clone())),
            "GetEnumValueCaptionFromOrdinalValue" => Ok(Value::Text(by_ordinal(args)?.2.clone())),
            "OptionMembers" | "OptionString" | "OptionCaption" if is_enum => {
                unsupported(format!("FieldRef.{method} on an Enum field"))
            }
            "OptionMembers" | "OptionString" => Ok(Value::Text(
                members.iter().map(|m| m.1.as_str()).collect::<Vec<_>>().join(","),
            )),
            "OptionCaption" => Ok(Value::Text(
                members.iter().map(|m| m.2.as_str()).collect::<Vec<_>>().join(","),
            )),
            other => unsupported(format!("FieldRef.{other}")),
        }
    }
}
