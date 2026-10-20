//! `Record.IsDirty` (BC 29, runtime 18.0): whether the buffer of a record variable differs from what the variable last
//! read or wrote. Pinned by lt-ac/ACRecordIsDirty.al (codeunit 77504). The snapshot lives in `RecordState::is_dirty_base`
//! and is kept by [`Session::record_op_tracked`], around every record method.
//!
//! What the corpus pins, and so what is implemented:
//! - a declared variable is compared with a fresh buffer: not dirty, also after Init and DeleteAll; dirty after a field
//!   assignment (ACRecordIsDirty.al:37-43);
//! - Get, FindFirst, FindLast, Insert, Modify and Rename take a snapshot (ACRecordIsDirty.al:44-51, 59-60, 103-110);
//! - values are compared, not writes: assigning the value a field holds, or setting a field back, is not dirty
//!   (ACRecordIsDirty.al:52-56); Validate is a write like any other (ACRecordIsDirty.al:61-62);
//! - Init and Reset keep the snapshot (ACRecordIsDirty.al:111-115);
//! - `:=` copies it (ACRecordIsDirty.al:63-64);
//! - TransferFields and Delete give a result where both readings (the snapshot stays, or the buffer becomes the snapshot)
//!   agree (ACRecordIsDirty.al:65-71); where they differ IsDirty is refused. Whether system fields count is not pinned
//!   either: an answer that depends on them is refused.
//!
//! Everything else is refused: a temporary record, a partial record (SetLoadFields), a table with an InitValue on a
//! fresh variable, a write that ran triggers or subscribers, Find/FindSet/Next, Copy, Clear, CalcFields, a copy of a
//! record variable (by value, xRec), a failed read or write, a FlowField or Blob that differs, and a buffer whose answer
//! differs from a comparison with the row stored under its key (the buffer may have been filled by something that is not
//! a record method, such as a page; nothing pins that).

use crate::Session;
use al_builtins::native::Signal;
use al_builtins::ops;
use al_builtins::record_op::RecordOp;
use al_builtins::value::{DirtyBase, RecordState, Value};
use al_data::Store;
use std::cell::RefCell;
use std::cmp::Ordering;
use std::rc::Rc;

type Rec = Rc<RefCell<RecordState>>;

/// What a record method does to the snapshot.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Effect {
    /// Neither reads nor writes the buffer.
    Keep,
    /// Kept only on a declared variable nothing was read into (ACRecordIsDirty.al:37-38 DeleteAll); otherwise not pinned.
    KeepFresh,
    /// Writes fields, keeps the snapshot (Validate, ACRecordIsDirty.al:61-62). AL code that runs inside sees no snapshot.
    Write,
    /// A successful read or write without triggers: the buffer becomes the snapshot.
    Snapshot,
    /// The buffer after the call is a second candidate (TransferFields, Delete).
    Alt,
    /// Not pinned: IsDirty is refused until the next snapshot.
    Lose,
}

fn effect(method: RecordOp) -> Effect {
    use RecordOp as O;
    match method {
        O::Get | O::FindFirst | O::FindLast | O::Insert | O::Modify | O::Rename => Effect::Snapshot,
        O::TransferFields | O::Delete => Effect::Alt,
        O::Validate => Effect::Write,
        O::DeleteAll | O::ModifyAll => Effect::KeepFresh,
        // Init and Reset are pinned (ACRecordIsDirty.al:39-40, 111-115); the rest neither read nor write the buffer.
        O::Init
        | O::Reset
        | O::IsDirty
        | O::AreFieldsLoaded
        | O::Ascending
        | O::CalcSums
        | O::ClearMarks
        | O::Consistent
        | O::CopyFilter
        | O::CopyFilters
        | O::Count
        | O::CountApprox
        | O::CurrentCompany
        | O::CurrentKey
        | O::FieldActive
        | O::FieldCaption
        | O::FieldError
        | O::FieldName
        | O::FieldNo
        | O::FilterGroup
        | O::GetAscending
        | O::GetFilter
        | O::GetFilters
        | O::GetPosition
        | O::GetRangeMax
        | O::GetRangeMin
        | O::GetView
        | O::HasFilter
        | O::HasLinks
        | O::IsEmpty
        | O::IsTemporary
        | O::LockTable
        | O::Mark
        | O::MarkedOnly
        | O::ReadConsistency
        | O::ReadIsolation
        | O::ReadPermission
        | O::RecordId
        | O::RecordLevelLocking
        | O::Relation
        | O::SecurityFiltering
        | O::SetAscending
        | O::SetCurrentKey
        | O::SetFilter
        | O::SetRange
        | O::SetRecFilter
        | O::SetView
        | O::TableCaption
        | O::TableName
        | O::TestField
        | O::WritePermission
        | O::AddLoadFields
        | O::SetLoadFields
        | O::SetBaseLoadFields
        | O::SetAutoCalcFields => Effect::Keep,
        _ => Effect::Lose,
    }
}

/// Whether the trigger flag of a write asks for triggers (`Insert(true)`, `Modify(true)`, `Delete(true)`).
fn run_trigger(method: RecordOp, args: &[Value]) -> bool {
    matches!(method, RecordOp::Insert | RecordOp::Modify | RecordOp::Delete)
        && matches!(args.first(), Some(Value::Boolean(true)))
}

impl Session {
    pub(crate) fn record_op_tracked(
        &mut self,
        method: RecordOp,
        rec: &Rec,
        args: &mut [Value],
    ) -> Result<Value, Signal> {
        let effect = effect(method);
        if effect == Effect::Keep {
            return self.record_op_inner(method, rec, args);
        }
        if effect == Effect::KeepFresh {
            let r = self.record_op_inner(method, rec, args);
            let mut s = rec.borrow_mut();
            if !matches!(s.is_dirty_base, DirtyBase::Fresh) {
                s.is_dirty_base =
                    DirtyBase::Unknown("after DeleteAll or ModifyAll on a record that was read (not pinned)");
            }
            return r;
        }
        if effect == Effect::Lose {
            let r = self.record_op_inner(method, rec, args);
            rec.borrow_mut().is_dirty_base = DirtyBase::Unknown(lose_reason(method));
            return r;
        }
        let triggers = run_trigger(method, args);
        if effect != Effect::Snapshot {
            // Validate, TransferFields and Delete write the buffer or compare with it afterwards: a snapshot that is the
            // buffer itself becomes a copy first.
            let mut s = rec.borrow_mut();
            let RecordState {
                is_dirty_base, fields, ..
            } = &mut *s;
            is_dirty_base.materialize(fields);
        }
        // AL code that runs inside (a trigger, a subscriber) may call IsDirty or another record method on the same record:
        // the first is refused while any tracked method runs, the second makes `ran_code` true.
        let calls = self.calls_made;
        self.dirty_ops_open += 1;
        let r = self.record_op_inner(method, rec, args);
        self.dirty_ops_open -= 1;
        let ran_code = self.calls_made != calls;
        let mut s = rec.borrow_mut();
        let saved = std::mem::replace(&mut s.is_dirty_base, DirtyBase::Synced);
        s.is_dirty_base = match (&r, effect) {
            (Err(_), _) => DirtyBase::Unknown("after a record method that failed (not pinned)"),
            (Ok(Value::Boolean(false)), Effect::Snapshot | Effect::Alt) => {
                DirtyBase::Unknown("after a read or write that returned false (not pinned)")
            }
            _ if triggers || ran_code => {
                DirtyBase::Unknown("after a record method that ran triggers or subscribers (not pinned)")
            }
            (Ok(_), Effect::Write) => saved,
            // No copy: the buffer is the snapshot until a field is written (RecordState::before_field_write).
            (Ok(_), Effect::Snapshot) => DirtyBase::Synced,
            (Ok(_), _) => match saved {
                DirtyBase::Row { base, .. } => DirtyBase::Row {
                    base,
                    alt: Some(s.fields.clone()),
                },
                _ => DirtyBase::Unknown("TransferFields or Delete on a record that was not read (not pinned)"),
            },
        };
        r
    }

    /// `Record.IsDirty()`.
    pub(crate) fn record_is_dirty(&mut self, rec: &Rec) -> Result<Value, Signal> {
        if self.dirty_ops_open > 0 {
            return refuse("inside a trigger or subscriber of a record method (not pinned)");
        }
        let t = self.table(rec)?;
        let (fields, base, alt) = {
            let s = rec.borrow();
            if s.temporary {
                return refuse("on a temporary record (not pinned)");
            }
            if s.load.partial {
                return refuse("on a partial record (SetLoadFields; not pinned)");
            }
            if s.company.is_some() {
                return refuse("after ChangeCompany (not pinned)");
            }
            let (base, alt) = match &s.is_dirty_base {
                DirtyBase::Unknown(why) => return refuse(why),
                DirtyBase::Fresh => (None, None),
                DirtyBase::Synced => (Some(s.fields.clone()), None),
                DirtyBase::Row { base, alt } => (Some(base.clone()), alt.clone()),
            };
            (s.fields.clone(), base, alt)
        };
        let base = match base {
            Some(b) => b,
            None => {
                let has_init = self.engine.program.objects[t.object as usize]
                    .table
                    .as_ref()
                    .is_some_and(|s| s.fields.iter().any(|f| f.init_text.is_some()));
                if has_init {
                    return refuse("on a declared variable of a table with an InitValue (not pinned)");
                }
                self.initial_fields(t.object as usize)
            }
        };
        // (whether a field other than a system field differs, whether any field differs). Nothing pins whether the
        // system fields (SystemId, SystemCreatedAt, ...) count: where the two differ the answer is refused.
        let differs = |other: &[Value]| -> Result<(bool, bool), Signal> {
            let (mut user, mut any) = (false, false);
            for (i, f) in t.fields.iter().enumerate() {
                match ops::compare(&fields[i], &other[i]) {
                    Ok(Ordering::Equal) => {}
                    Ok(_) if !f.stored => {
                        return refuse(&format!("on a FlowField {} that differs (not pinned)", f.name));
                    }
                    Ok(_) => {
                        any = true;
                        user |= f.id < 2_000_000_000;
                    }
                    Err(_) => {
                        return refuse(&format!(
                            "on field {} of a type that is not compared (not pinned)",
                            f.name
                        ));
                    }
                }
            }
            Ok((user, any))
        };
        let (dirty, any) = differs(&base)?;
        match alt {
            None if any != dirty => return refuse("where only system fields differ from the snapshot (not pinned)"),
            None => {}
            // After TransferFields or Delete the snapshot may be kept or may become the buffer. ACRecordIsDirty.al:65-68
            // (`after_transferfields` No, after Init, where TransferFields leaves the system fields empty) rules out
            // "the snapshot is kept and system fields count"; the other three readings must agree.
            Some(alt) => {
                let (alt_user, alt_any) = differs(&alt)?;
                if alt_user != dirty || alt_any != dirty {
                    return refuse(
                        "after TransferFields or Delete, where the corpus does not tell which snapshot BC keeps",
                    );
                }
            }
        }
        // The buffer may have been filled by something other than a record method (a page, RecordRef.SetTable): the answer
        // must be the same against the row stored under the key (fields other than system fields).
        let cx = self.cx_for(rec, &t)?;
        let pk = t.primary_key(&fields);
        let stored = self
            .store_for(rec)
            .get(&cx, &t, &pk)
            .map_err(|e| Signal::Unsupported(format!("{e}")))?;
        if let Some(row) = stored {
            if differs(&row)?.0 != dirty {
                return refuse("where the snapshot and the stored row give different answers (not pinned)");
            }
        }
        Ok(Value::Boolean(dirty))
    }
}

fn refuse<T>(why: &str) -> Result<T, Signal> {
    Err(Signal::Unsupported(format!("Record.IsDirty {why}")))
}

fn lose_reason(method: RecordOp) -> &'static str {
    use RecordOp as O;
    match method {
        O::Find | O::FindSet | O::Next => "after Find, FindSet or Next (not pinned)",
        O::Copy => "after Copy (not pinned)",
        O::Clear => "after Clear (not pinned)",
        O::CalcFields => "after CalcFields (not pinned)",
        O::GetBySystemId => "after GetBySystemId (not pinned)",
        _ => "after a record method whose effect on the snapshot is not pinned",
    }
}
