//! `RecordRef`, `FieldRef` and `KeyRef` on the VM.
//!
//! An open `RecordRef` owns a [`RecordState`] exactly as a `Record` variable
//! does, so every data operation is the same code path as the record
//! methods in `records.rs` (triggers included). `FieldRef` and `KeyRef` point
//! into that buffer by field position and key index. Behaviour the corpus
//! does not pin raises `Signal::Unsupported` instead of guessing.

use crate::Session;
use al_builtins::catalog::Catalog;
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
        other => unsupported(format!("integer argument {i}: {other:?}")),
    }
}

/// A table's fields a caller can enumerate (`FieldCount`, `FieldIndex`): in
/// field-number order, system fields left out.
fn enumerable_fields(t: &TableDef) -> Vec<usize> {
    let mut v: Vec<usize> = (0..t.fields.len())
        .filter(|&i| t.fields[i].id < SYSTEM_FIELD_BASE)
        .collect();
    v.sort_by_key(|&i| t.fields[i].id);
    v
}

impl Session {
    pub(crate) fn ref_op(&mut self, id: &str, recv: Option<&mut Value>, args: &mut [Value]) -> Result<Value, Signal> {
        let Some(sig) = catalog().by_id(id) else {
            return unsupported(format!("builtin {id}"));
        };
        let lib: Lib = match recv {
            Some(Value::Library(_, l)) => l.clone(),
            other => return unsupported(format!("{id} on {other:?}")),
        };
        match sig.receiver.as_str() {
            "RecordRef" => self.recordref_op(&sig.name, &lib, args),
            "FieldRef" => self.fieldref_op(&sig.name, &lib, args),
            "KeyRef" => self.keyref_op(&sig.name, &lib, args),
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
            LibraryValue::RecordRef(r) | LibraryValue::FieldRef(r, _) | LibraryValue::KeyRef(r, _) => Ok(r.clone()),
            LibraryValue::SystemTable(n) => unsupported(format!(
                "{what} on a RecordRef opened on system table {n} (the platform's system tables are not modelled)"
            )),
            // BC's error text for this is not pinned by the corpus.
            _ => unsupported(format!(
                "{what} on a RecordRef that is not open (error text not pinned yet)"
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
        if let Some(c) = args.get(2) {
            if !al_builtins::native::text_of(c).unwrap_or_default().is_empty() {
                return unsupported("RecordRef.Open with a company name (ChangeCompany is not implemented yet)");
            }
        }
        let Some(table) = self.table_object(no) else {
            return match no {
                // corpus recordref/TestRecordRefOpenCompilationTarget.al:27-35: an internal
                // system table is refused for a Cloud-target app (this app is Cloud).
                2_000_000_071 => Err(al_error(format!(
                    "You cannot open record {no} from a RecordRef data type when you are using target Cloud."
                ))),
                // :37-62: these system tables open for a Cloud-target app and report their number.
                2_000_000_026 | 2_000_000_187 | 2_000_000_188 => {
                    *lib.borrow_mut() = LibraryValue::SystemTable(no);
                    Ok(Value::Null)
                }
                n if n >= i64::from(SYSTEM_FIELD_BASE) => unsupported(format!(
                    "RecordRef.Open on system table {n} (whether a Cloud target may open it is not pinned)"
                )),
                // corpus recordref/TestRecordRefOpenMissingTable.al:31-49 pins both halves; how
                // BC names the calling object is not pinned (docs/probes-needed.md).
                n => Err(al_error(format!(
                    "You tried to invoke the Table object with the ID {n} from the object (caller not recorded). \
                     An object with that ID does not exist in the current application."
                ))),
            };
        };
        let Value::Record(rec) = self.default_value(&RtType::Record(table as u32, temp)) else {
            return unsupported("record buffer for RecordRef.Open");
        };
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
                    return unsupported(format!("RecordRef.GetTable of {:?}", args.first()));
                };
                let copy = src.borrow().clone();
                *lib.borrow_mut() = LibraryValue::RecordRef(Rc::new(RefCell::new(copy)));
                return Ok(Value::Null);
            }

            "Number" => {
                if let LibraryValue::SystemTable(n) = &*lib.borrow() {
                    return Ok(Value::Integer(*n as i32));
                }
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
            "CurrentCompany" => Ok(Value::Text(self.cx.company.clone())),
            "IsTemporary" => Ok(Value::Boolean(rec.borrow().temporary)),
            "HasFilter" => Ok(Value::Boolean(Self::has_filter(&rec))),
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
            // Doc (RecordRef.Duplicate): a new ref with the same table, buffer and filters, independent of the original.
            "RecordId" => Ok(self.record_id(&rec, &t)),
            "LockTable" | "ReadConsistency" | "RecordLevelLocking" | "ReadPermission" | "WritePermission"
            | "CountApprox" | "ReadIsolation" | "SecurityFiltering" | "GetPosition" | "SetPosition" => {
                self.record_meta_op(method, &rec, &t, args)
            }
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
                        other => return unsupported(format!("RecordRef.Get of {other:?}")),
                    },
                    other => return unsupported(format!("RecordRef.Get of {other:?}")),
                };
                let mut a = key;
                self.record_op("Get", &rec, &mut a)
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
                match t.fields.iter().position(|f| i64::from(f.id) == no) {
                    Some(pos) => Ok(handle("FieldRef", LibraryValue::FieldRef(rec, pos))),
                    None => unsupported(format!(
                        "RecordRef.Field({no}) on a field {} does not have (error text not pinned yet)",
                        t.name
                    )),
                }
            }
            "FieldExist" => {
                let no = int_arg(args, 0)?;
                Ok(Value::Boolean(t.fields.iter().any(|f| i64::from(f.id) == no)))
            }
            "FieldCount" => Ok(Value::Integer(enumerable_fields(&t).len() as i32)),
            "FieldIndex" => {
                let i = int_arg(args, 0)?;
                match usize::try_from(i - 1)
                    .ok()
                    .and_then(|i| enumerable_fields(&t).get(i).copied())
                {
                    Some(pos) => Ok(handle("FieldRef", LibraryValue::FieldRef(rec, pos))),
                    None => unsupported(format!(
                        "RecordRef.FieldIndex({i}) out of range (error text not pinned yet)"
                    )),
                }
            }
            "KeyCount" => Ok(Value::Integer(t.keys.len() as i32)),
            "KeyIndex" => {
                let i = int_arg(args, 0)?;
                match usize::try_from(i - 1).ok().filter(|k| *k < t.keys.len()) {
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
                    return unsupported(format!("RecordRef.SetTable of {:?}", args.first()));
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
                    if s.field_descending {
                        return unsupported("Copy of a record with a per-field SetAscending (not pinned)");
                    }
                    let mut d = dst.borrow_mut();
                    d.fields = s.fields.clone();
                    d.filters = s.filters.clone();
                    d.key = s.key;
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
            "FieldCount" => Ok(Value::Integer(t.keys[k].len() as i32)),
            "Record" => Ok(handle("RecordRef", LibraryValue::RecordRef(rec))),
            "FieldIndex" => {
                let i = int_arg(args, 0)?;
                match usize::try_from(i - 1).ok().and_then(|i| t.keys[k].get(i).copied()) {
                    Some(pos) => Ok(handle("FieldRef", LibraryValue::FieldRef(rec, pos))),
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
            LibraryValue::FieldRef(r, p) => (r.clone(), *p),
            _ => return unsupported(format!("FieldRef.{method} on a FieldRef that was never assigned")),
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
            "Number" => Ok(Value::Integer(field.id as i32)),
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
                other => unsupported(format!("FieldRef.Length of a {other:?} field")),
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
            other => return unsupported(format!("FieldRef.Type of a {other:?} field")),
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
    fn members_of_field(&self, t: &TableDef, pos: usize) -> Result<Option<FieldMembers>, Signal> {
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
