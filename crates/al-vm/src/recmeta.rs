//! Record builtins that read table metadata or guard on session state:
//! `TestField`, `Truncate`, `FieldCaption`, `FieldName`, `FieldNo`,
//! `TableCaption`, `TableName`.

use crate::Session;
use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::value::{RecordState, Value};
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
fn is_blank(v: &Value) -> Result<bool, Signal> {
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
        other => return unsupported(format!("TestField of a {other:?} field")),
    })
}

fn arg_pos(args: &[Value], i: usize) -> Result<usize, Signal> {
    match args.get(i) {
        Some(Value::Integer(p)) => Ok(*p as usize),
        other => unsupported(format!("field argument {other:?}")),
    }
}

impl Session {
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
            | Value::DateFormula(_) => Ok(al_builtins::format::format(v, 0, 0, self)),
            other => unsupported(format!("a {other:?} value in an error message")),
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
        match method {
            "TableName" => Ok(Value::Text(t.name.clone())),
            "TableCaption" => Ok(Value::Text(self.caption(t))),
            "FieldName" => Ok(Value::Text(t.fields[arg_pos(args, 0)?].name.clone())),
            "FieldCaption" => self.field_caption_checked(t, arg_pos(args, 0)?).map(Value::Text),
            "FieldNo" => Ok(Value::Integer(t.fields[arg_pos(args, 0)?].id as i32)),
            "TestField" => self.test_field(rec, t, args),
            "FieldError" => self.field_error(rec, t, args),
            "Truncate" => self.truncate(rec, t, args),
            other => unsupported(format!("Record.{other}")),
        }
    }

    /// `TestField(Field [, Value])`. The "must have a value" text is pinned by the corpus
    /// (record/TestRecordTestFieldLookupPageId_Tests.al:54-72); the mismatch text is Assert.al's
    /// documented "new format" (docs/probes-needed.md).
    pub(crate) fn test_field(&mut self, rec: &Rec, t: &TableDef, args: &[Value]) -> Result<Value, Signal> {
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
                    return Err(error(
                        format!(
                            "{caption} must be {} in {table}: {key}. Currently it's {}.",
                            self.message_value(&want)?,
                            self.message_value(&current)?
                        ),
                        "TestField",
                    ));
                }
            }
        }
        Ok(Value::Null)
    }

    /// `FieldError(Field, Text)`: `<caption> <text> in <table>: <key>.` A bare `FieldError(Field)`
    /// has no pinned wording (the corpus only checks that the caption appears): refused.
    fn field_error(&mut self, rec: &Rec, t: &TableDef, args: &[Value]) -> Result<Value, Signal> {
        let pos = arg_pos(args, 0)?;
        let Some(text) = args.get(1).and_then(al_builtins::native::text_of) else {
            return unsupported("FieldError without a message (default wording not pinned)");
        };
        let row = rec.borrow().fields.clone();
        let key = self.key_text(t, &row)?;
        Err(error(
            format!("{} {text} in {}: {key}.", self.field_caption(t, pos), self.caption(t)),
            "Dialog",
        ))
    }

    /// `Truncate(ResetAutoIncrement)`: the guards BC runs first are pinned by
    /// record/TestRecordTruncate*.al; what a filter does to the delete is not.
    fn truncate(&mut self, rec: &Rec, t: &TableDef, args: &[Value]) -> Result<Value, Signal> {
        // record/TestRecordTruncateTryScope.al:36-110
        if self.try_depth > 0 {
            return Err(error("Truncate is not supported in try functions.".into(), "Dialog"));
        }
        // record/TestRecordTruncateEventAndFilterGuards.al:70-95: a delete subscriber refuses Truncate.
        // Which events a table's subscribers listen to is not modelled yet, so any subscriber refuses.
        self.check_writable(t, "Delete")?;
        let schema = self.engine.program.objects[t.object as usize].table.clone();
        let filters = rec.borrow().filters.clone();
        // :112-125: a filter on a FlowField (not a FlowFilter).
        if let Some(schema) = &schema {
            let is_flowfield = |pos: usize| {
                schema.fields[pos]
                    .properties
                    .iter()
                    .any(|(k, v)| k.eq_ignore_ascii_case("FieldClass") && v.trim().eq_ignore_ascii_case("FlowField"))
            };
            if filters.iter().any(|f| is_flowfield(f.field)) {
                return Err(error(
                    "Truncate does not support filters on FlowFields.".into(),
                    "Dialog",
                ));
            }
            if schema
                .fields
                .iter()
                .any(|f| matches!(&f.ty, al_builtins::value::RtType::Library(n) if n == "Media" || n == "MediaSet"))
            {
                return unsupported("Truncate on a table with a Media field (guard not pinned)");
            }
            if schema.fields.iter().any(|f| f.auto_increment) && matches!(args.first(), Some(Value::Boolean(true))) {
                return unsupported(
                    "Truncate(true) on a table with an AutoIncrement field (counter reset not implemented)",
                );
            }
        }
        if rec.borrow().temporary {
            return unsupported("Truncate of a temporary table (guard not pinned)");
        }
        if !filters.is_empty() {
            return unsupported("Truncate with a filter (whether the filter limits the delete is not pinned)");
        }
        let cx = self.cx.clone();
        self.store_for(rec)
            .delete_all(&cx, t, &[])
            .map_err(|e| Signal::Unsupported(format!("{e:?}")))?;
        Ok(Value::Boolean(true))
    }
}
