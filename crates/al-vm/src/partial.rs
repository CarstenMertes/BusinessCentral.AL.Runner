//! Partial records: `SetLoadFields`, `AddLoadFields`, `AreFieldsLoaded`.
//!
//! Reading a field that was left out of the load set returns its stored value, so the only thing
//! AL can observe is `AreFieldsLoaded` (record/TestRecordPartialLoad.al). The model, each rule
//! pinned by the corpus test named beside it:
//!
//! - `SetLoadFields(f...)` sets the request; with no arguments it resets to the default, a full
//!   load (:61-99). `AddLoadFields` adds to a request that exists.
//! - A record that fetched nothing reports nothing loaded, the primary key included (:102-141).
//! - A fetch loads the request, the fields of the current key and the fields of every active
//!   filter (:193-285).
//! - Reading an omitted field loads it (:144-190).
//! - The loaded set only grows until `Clear`: a narrower request after a full fetch keeps the
//!   fields in hand, even across a re-fetch of the row, and a wider one loads nothing until the
//!   next fetch (record/TestRecordPartialLoadNarrowAfterFetch.al:56-185).
//!
//! What the corpus does not pin is refused when asked about: a primary key field under a secondary
//! current key, a field of a row other than the one the buffer held, a field assigned without
//! being loaded, a record written but never fetched, a field that is not stored.

use crate::Session;
use crate::records::Rec;
use al_builtins::describe::Describe;
use al_builtins::native::Signal;
use al_builtins::value::{Buffer, FieldSet, Value};
use al_data::{Store, TableDef};

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn positions(args: &[Value]) -> Result<Vec<usize>, Signal> {
    let mut out: Vec<usize> = Vec::new();
    for a in args {
        match a {
            Value::Integer(p) => {
                let p = *p as usize;
                if !out.contains(&p) {
                    out.push(p);
                }
            }
            other => return unsupported(format!("field argument {}", other.describe())),
        }
    }
    Ok(out)
}

/// The primary key of a row, as a comparable text.
fn row_id(t: &TableDef, row: &[Value]) -> String {
    format!("{:?}", t.primary_key(row))
}

impl Session {
    /// A row is about to be read into a record whose request is partial.
    pub(crate) fn note_partial_fetch(&mut self, rec: &Rec, row: &[Value]) -> Result<(), Signal> {
        let t = self.table(rec)?;
        let r = rec.borrow();
        let mut have = r.load.request.clone().unwrap_or_default();
        // The current key and the filters are needed to find the row (:193-285).
        let key = &t.keys[r.key.min(t.keys.len() - 1)];
        have.extend(key.iter().copied());
        have.extend(r.filters.iter().map(|f| f.field));
        have.sort_unstable();
        have.dedup();
        // A secondary key ends in the primary key; whether that is loaded is not pinned.
        let mut unsure = Vec::new();
        if r.key != 0 {
            unsure.extend(t.keys[0].iter().copied().filter(|p| !have.contains(p)));
        }
        let new_row = row_id(&t, row);
        let buffer = match &r.load.buffer {
            None => Buffer {
                have: FieldSet::Of(have),
                unsure: FieldSet::Of(unsure),
                row: Some(new_row),
            },
            Some(old) => {
                let old_row = old.row.clone().unwrap_or_else(|| row_id(&t, &r.fields));
                let mut next = old.clone();
                next.row = Some(new_row.clone());
                if old_row == new_row {
                    // The same row again: nothing is evicted (record/TestRecordPartialLoadNarrowAfterFetch.al:121-185).
                    if let FieldSet::Of(h) = &mut next.have {
                        for p in have {
                            if !h.contains(&p) {
                                h.push(p);
                            }
                        }
                    }
                    for p in unsure {
                        if !next.have.contains(p) {
                            next.unsure.add(p);
                        }
                    }
                } else {
                    // Another row: what the old buffer held is not known to carry over.
                    let carried = match &old.have {
                        FieldSet::All => FieldSet::All,
                        FieldSet::Of(h) => FieldSet::Of(h.iter().copied().filter(|p| !have.contains(p)).collect()),
                    };
                    next.have = FieldSet::Of(have);
                    next.unsure = match carried {
                        FieldSet::All => FieldSet::All,
                        FieldSet::Of(mut v) => {
                            if let FieldSet::Of(u) = &old.unsure {
                                v.extend(u.iter().copied());
                            }
                            v.extend(unsure);
                            FieldSet::Of(v)
                        }
                    };
                }
                next
            }
        };
        drop(r);
        let mut r = rec.borrow_mut();
        r.load.partial = !matches!(buffer.have, FieldSet::All);
        r.load.buffer = Some(buffer);
        Ok(())
    }

    pub(crate) fn load_fields_op(
        &mut self,
        method: &str,
        rec: &Rec,
        t: &TableDef,
        args: &[Value],
    ) -> Result<Value, Signal> {
        let wanted = positions(args)?;
        let engine = self.engine.clone();
        let schema = engine.program.objects[t.object as usize].table.as_ref();
        let stored = |p: usize| schema.is_some_and(|s| s.fields.get(p).is_some_and(|f| f.stored));
        match method {
            "SetLoadFields" | "AddLoadFields" => {
                if let Some(p) = wanted.iter().find(|&&p| !stored(p)) {
                    return unsupported(format!("{method} of field position {p}, which is not a stored field"));
                }
                let mut r = rec.borrow_mut();
                match (method, wanted.is_empty()) {
                    // No arguments: back to the default load set (record/TestRecordPartialLoad.al:61-99).
                    ("SetLoadFields", true) => r.load.request = None,
                    ("SetLoadFields", false) => r.load.request = Some(wanted),
                    // Without a request the load is already full: nothing to add. The result is
                    // the documentation's (docs/probes-needed.md).
                    ("AddLoadFields", _) => match &mut r.load.request {
                        None => return Ok(Value::Boolean(false)),
                        Some(req) => {
                            for p in wanted {
                                if !req.contains(&p) {
                                    req.push(p);
                                }
                            }
                            return Ok(Value::Boolean(true));
                        }
                    },
                    _ => unreachable!(),
                }
                Ok(Value::Null)
            }
            "SetBaseLoadFields" => {
                // Only the full-load case is pinned as callable (record/TestRecordExtended.al:71-80);
                // what the base set holds is not.
                if rec.borrow().load.request.is_some() {
                    return unsupported("SetBaseLoadFields on a partial load (the base set is not pinned)");
                }
                Ok(Value::Boolean(false))
            }
            "AreFieldsLoaded" => {
                if wanted.is_empty() {
                    return unsupported("AreFieldsLoaded without fields");
                }
                if let Some(p) = wanted.iter().find(|&&p| !stored(p)) {
                    return unsupported(format!(
                        "AreFieldsLoaded of field position {p}, which is not a stored field"
                    ));
                }
                let r = rec.borrow();
                let Some(b) = &r.load.buffer else {
                    // Nothing was fetched (record/TestRecordPartialLoad.al:102-141). A buffer that
                    // was filled by hand is not pinned.
                    let fresh = self.initial_fields(t.object as usize);
                    if format!("{:?}", fresh) != format!("{:?}", r.fields) {
                        return unsupported("AreFieldsLoaded on a record that was filled but never fetched");
                    }
                    return Ok(Value::Boolean(false));
                };
                let mut all = true;
                for &p in &wanted {
                    if b.have.contains(p) {
                        continue;
                    }
                    if b.unsure.contains(p) {
                        return unsupported(format!(
                            "AreFieldsLoaded of a field whose state the corpus does not pin (position {p})"
                        ));
                    }
                    all = false;
                }
                Ok(Value::Boolean(all))
            }
            _ => unreachable!(),
        }
    }

    /// `Record.LoadFields(Fields)` on a record that was never fetched and holds its initial values:
    /// reads the row of the buffer's primary key. Base Application's `Service Document Archive
    /// Mgmt.` calls it as a statement before `Get()`, so it raises nothing when that row exists; that
    /// the record then holds the row, with the named fields loaded, is the documentation's
    /// (docs/probes-needed.md, issue 41). Without such a row the statement form does nothing. Everything
    /// else is refused.
    pub(crate) fn load_fields_row(
        &mut self,
        rec: &Rec,
        t: &TableDef,
        cx: &al_data::AccessContext,
        args: &[Value],
    ) -> Result<Value, Signal> {
        let wanted = positions(args)?;
        let engine = self.engine.clone();
        let schema = engine.program.objects[t.object as usize].table.as_ref();
        if let Some(p) = wanted
            .iter()
            .find(|&&p| !schema.is_some_and(|s| s.fields.get(p).is_some_and(|f| f.stored)))
        {
            return unsupported(format!("LoadFields of field position {p}, which is not a stored field"));
        }
        if rec.borrow().load.buffer.is_some() {
            return unsupported(
                "LoadFields on a record that holds a fetched row: whether BC reads the row again, over values assigned since, is not pinned (docs/probes-needed.md)",
            );
        }
        let fresh = self.initial_fields(t.object as usize);
        {
            let r = rec.borrow();
            if format!("{:?}", fresh) != format!("{:?}", r.fields) {
                return unsupported(
                    "LoadFields on a record that was filled but never fetched: not pinned (docs/probes-needed.md)",
                );
            }
            if !r.auto_calc.is_empty() {
                return unsupported(
                    "LoadFields on a record with SetAutoCalcFields: not pinned (docs/probes-needed.md)",
                );
            }
        }
        al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::LoadFieldsReadsRow);
        let pk = t.primary_key(&rec.borrow().fields);
        let row = self
            .store_for(rec)
            .get(cx, t, &pk)
            .map_err(|e| Signal::Unsupported(format!("{e}")))?;
        let Some(row) = row else {
            // As a statement it raises nothing: Base Application's `Service Document Archive
            // Mgmt.`.GetNextOccurrenceNo calls it on a fresh archive record, where no row has the
            // blank key, on every service document insert. The record stays as it was. The answer
            // when the result is used is not pinned.
            if !self.ret_used {
                return Ok(Value::Boolean(false));
            }
            return unsupported(
                "LoadFields with its result used when no row has the record's primary key: BC's answer is not pinned (docs/probes-needed.md)",
            );
        };
        let id = row_id(t, &row);
        self.load(rec, row)?;
        let mut have = wanted;
        have.extend(t.keys[0].iter().copied());
        have.sort_unstable();
        have.dedup();
        let mut r = rec.borrow_mut();
        r.load.buffer = Some(Buffer {
            have: FieldSet::Of(have),
            unsure: FieldSet::All,
            row: Some(id),
        });
        r.load.partial = true;
        Ok(Value::Boolean(true))
    }
}
