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
use al_builtins::record_op::RecordOp;
use al_builtins::value::{Buffer, FieldSet, Value};
use al_data::TableDef;

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
pub(crate) fn row_id(t: &TableDef, row: &[Value]) -> String {
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
                cleared: false,
            },
            Some(old) if old.cleared && old.row.as_deref() != Some(new_row.as_str()) => Buffer {
                have: FieldSet::Of(have),
                unsure: FieldSet::Of(unsure),
                row: Some(new_row),
                cleared: false,
            },
            Some(old) => {
                let old_row = old.row.clone().unwrap_or_else(|| row_id(&t, &r.fields));
                let mut next = old.clone();
                next.row = Some(new_row.clone());
                next.cleared = false;
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
        method: RecordOp,
        rec: &Rec,
        t: &TableDef,
        args: &[Value],
    ) -> Result<Value, Signal> {
        let wanted = positions(args)?;
        let engine = self.engine.clone();
        let schema = engine.program.objects[t.object as usize].table.as_ref();
        let stored = |p: usize| schema.is_some_and(|s| s.fields.get(p).is_some_and(|f| f.stored));
        match method {
            RecordOp::SetLoadFields | RecordOp::AddLoadFields => {
                // A field that is not stored (a FlowField, as in Sales Line SetLoadFieldsForInvDiscoundCalculation)
                // is accepted and adds nothing to the request: it is no column to load. Not pinned by the corpus.
                let no_args = wanted.is_empty();
                let mut wanted = wanted;
                if wanted.iter().any(|&p| !stored(p)) {
                    al_builtins::assumptions::assume(al_builtins::assumptions::Assumption::LoadFieldsNonStored);
                    wanted.retain(|&p| stored(p));
                }
                let mut r = rec.borrow_mut();
                match (method, no_args) {
                    // No arguments: back to the default load set (record/TestRecordPartialLoad.al:61-99).
                    (RecordOp::SetLoadFields, true) => r.load.request = None,
                    (RecordOp::SetLoadFields, false) => r.load.request = Some(wanted),
                    // Without a request the load is already full: nothing to add. The result is
                    // the documentation's (docs/probes-needed.md).
                    (RecordOp::AddLoadFields, _) => match &mut r.load.request {
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
            RecordOp::SetBaseLoadFields => {
                // Only the full-load case is pinned as callable (record/TestRecordExtended.al:71-80);
                // what the base set holds is not.
                // After SetLoadFields(Cd), SetBaseLoadFields loads the whole row again (session/
                // TestProbeSessionMiscContracts.al:PartialRecords_AreFieldsLoaded, `baseloadfields_txt` Yes).
                let had = rec.borrow().load.request.is_some();
                if had && self.ret_used {
                    return unsupported(
                        "the result of SetBaseLoadFields on a partial record (only the statement form is pinned)",
                    );
                }
                rec.borrow_mut().load.request = None;
                if had {
                    return Ok(Value::Null);
                }
                Ok(Value::Boolean(false))
            }
            RecordOp::AreFieldsLoaded => {
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

    /// `Record.LoadFields(Fields)`: on a record that was never fetched it does not read the row (the buffer keeps
    /// what was assigned, the key stays as it is) and the function form answers false, with an existing and with a
    /// missing key; on a record that holds a whole fetched row it changes nothing (types/PYLoadFindThis.al, BC 27.5 and
    /// 29.0). The answer on a fetched record and a record that holds a partial buffer are not pinned: refused.
    pub(crate) fn load_fields_row(&mut self, rec: &Rec, t: &TableDef, args: &[Value]) -> Result<Value, Signal> {
        let wanted = positions(args)?;
        let engine = self.engine.clone();
        let schema = engine.program.objects[t.object as usize].table.as_ref();
        if let Some(p) = wanted
            .iter()
            .find(|&&p| !schema.is_some_and(|s| s.fields.get(p).is_some_and(|f| f.stored)))
        {
            return unsupported(format!("LoadFields of field position {p}, which is not a stored field"));
        }
        // A buffer that survives a Clear is for a later fetch of the same row, not a row the record holds.
        if rec
            .borrow()
            .load
            .buffer
            .as_ref()
            .is_some_and(|b| !b.cleared && !matches!(b.have, FieldSet::All))
        {
            return unsupported(
                "LoadFields on a record that holds a partly fetched row: whether BC reads the missing fields, over values assigned since, is not pinned (docs/probes-needed.md)",
            );
        }
        if self.ret_used && rec.borrow().position.is_some() {
            return unsupported(
                "LoadFields with its result used on a record that holds a fetched row: BC's answer is not pinned (docs/probes-needed.md)",
            );
        }
        Ok(Value::Boolean(false))
    }
}
