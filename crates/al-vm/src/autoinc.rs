//! `AutoIncrement` fields: a per-table counter kept outside the store.
//!
//! Pinned by corpus record/TestAutoIncrementContracts.al, TestAutoIncrementExplicitKey.al,
//! TestAutoIncrementEdges.al and TestTempTableContracts.al; what they leave open is refused.
//! Truncate (which resets the counter) and guarded `Codeunit.Run` rollbacks (which restore it)
//! are not implemented, so no code path here has to model them.

use crate::Session;
use al_builtins::native::{AlError, Signal};
use al_builtins::ops;
use al_builtins::value::{RtType, Value};
use al_data::TableDef;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

fn number(v: &Value) -> Option<i64> {
    match v {
        Value::Integer(i) => Some(i64::from(*i)),
        Value::BigInteger(i) => Some(*i),
        _ => None,
    }
}

impl Session {
    /// The AutoIncrement field of the table, if it has one this runtime can drive.
    pub(crate) fn auto_increment_field(&self, t: &TableDef) -> Result<Option<usize>, Signal> {
        let schema = self.engine.program.objects[t.object as usize]
            .table
            .as_ref()
            .expect("table");
        let mut auto = schema.fields.iter().enumerate().filter(|(_, f)| f.auto_increment);
        let Some((pos, f)) = auto.next() else { return Ok(None) };
        if auto.next().is_some() {
            return unsupported("a table with several AutoIncrement fields");
        }
        if !matches!(f.ty, RtType::Integer | RtType::BigInteger) {
            return unsupported("an AutoIncrement field that is not an Integer or BigInteger");
        }
        Ok(Some(pos))
    }

    fn counter_slot(&self, t: &TableDef) -> (String, u32) {
        (
            if t.per_company {
                self.cx.company.clone()
            } else {
                String::new()
            },
            t.object,
        )
    }

    /// Gives a zero key the next counter value (corpus
    /// record/TestAutoIncrementContracts.al:25: setting the key to 0 requests a new value).
    /// Returns whether a value was taken.
    pub(crate) fn take_auto_increment(
        &mut self,
        t: &TableDef,
        field: usize,
        row: &mut [Value],
    ) -> Result<bool, Signal> {
        if number(&row[field]).unwrap_or(0) != 0 {
            return Ok(false);
        }
        let slot = self.counter_slot(t);
        let next = self.autoinc.get(&slot).copied().unwrap_or(0) + 1;
        row[field] = ops::convert(Value::BigInteger(next), &t.fields[field].ty)?;
        self.autoinc.insert(slot, next);
        Ok(true)
    }

    /// After a successful Insert with a key the caller (or OnInsert) chose: a key above
    /// the counter raises it, one at or below leaves it (record/TestAutoIncrementExplicitKey.al:43,98).
    pub(crate) fn note_explicit_key(&mut self, t: &TableDef, field: usize, row: &[Value]) {
        let Some(k) = number(&row[field]) else { return };
        let slot = self.counter_slot(t);
        let cur = self.autoinc.get(&slot).copied().unwrap_or(0);
        if k > cur {
            self.autoinc.insert(slot, k);
        }
    }

    /// `Truncate`: deletes every row without triggers and, with `reset`, restarts the
    /// AutoIncrement counter (corpus record/TestAutoIncrementContracts.al:83). Guards the
    /// corpus pins raise their message; the rest of the guards BC runs are refused.
    pub(crate) fn truncate(&mut self, t: &TableDef, rec: &crate::records::Rec, reset: bool) -> Result<Value, Signal> {
        let error = |message: &str| -> Result<Value, Signal> {
            Err(Signal::Error(AlError {
                message: message.into(),
                code: String::new(),
                call_stack: String::new(),
            }))
        };
        if rec.borrow().temporary {
            return unsupported("Truncate on a temporary record (the guards BC runs are not pinned yet)");
        }
        // Corpus record/TestRecordTruncateTryScope.al:34: the scope is inherited by called frames.
        if self.try_depth > 0 {
            return error("Truncate is not supported in try functions.");
        }
        if self.delete_event_subscribed(&t.name) {
            return error(
                "Truncate is not supported when the OnBeforeDelete and/or OnAfterDelete event is subscribed. Please remove the events subscriptions or use DeleteAll.",
            );
        }
        if self.global_enabled(t, crate::globaltriggers::GlobalOp::Delete)? {
            return unsupported(format!(
                "Truncate on {}, which has the OnDatabaseDelete event enabled (whether BC refuses it is not pinned)",
                t.name
            ));
        }
        let (filters, flow_field_filters) = self.split_filters(rec);
        if !flow_field_filters.is_empty() {
            return error("Truncate does not support filters on FlowFields.");
        }
        let cx = self.cx.clone();
        let store_err = |e: al_data::StoreError| Signal::Unsupported(format!("store error {e:?}"));
        if !filters.is_empty() {
            // Whether a filter limits what Truncate deletes is not pinned (the corpus filter
            // keeps the only row): proceed only when both readings agree.
            let all = al_data::Store::count(&self.store, &cx, t, &[]).map_err(store_err)?;
            let kept = al_data::Store::count(&self.store, &cx, t, &filters).map_err(store_err)?;
            if kept != all {
                return unsupported("Truncate with a filter that excludes rows (what it deletes is not pinned yet)");
            }
        }
        let slot = self.counter_slot(t);
        if let Some(counter) = self.autoinc.get(&slot).copied() {
            self.truncate_undo.entry(slot.clone()).or_insert(counter);
        } else {
            self.truncate_undo.entry(slot.clone()).or_insert(0);
        }
        if reset {
            self.autoinc.insert(slot, 0);
        }
        al_data::Store::delete_all(&mut self.store, &cx, t, &[]).map_err(store_err)?;
        Ok(Value::Boolean(true))
    }

    /// Whether a non-manual subscriber listens to the table's OnBeforeDelete or OnAfterDelete event.
    fn delete_event_subscribed(&self, table: &str) -> bool {
        self.engine.program.objects.iter().any(|o| {
            let manual = o.properties.iter().any(|(k, v)| {
                k.eq_ignore_ascii_case("EventSubscriberInstance") && v.trim().eq_ignore_ascii_case("Manual")
            });
            !manual
                && o.routines.iter().flat_map(|r| &r.attributes).any(|a| {
                    let text = a.text.to_ascii_lowercase();
                    let parts: Vec<&str> = text.split(',').map(|p| p.trim().trim_matches('\'')).collect();
                    a.name.eq_ignore_ascii_case("EventSubscriber")
                        && text.contains("objecttype::table")
                        && parts.get(1).is_some_and(|p| {
                            p.trim_start_matches("database::").trim_matches('"') == table.to_ascii_lowercase()
                        })
                        && parts
                            .get(2)
                            .is_some_and(|e| matches!(*e, "onbeforedeleteevent" | "onafterdeleteevent"))
                })
        })
    }

    /// Rename may not change an AutoIncrement value (record/TestAutoIncrementExplicitKey.al:147,
    /// TestAutoIncrementEdges.al:296).
    pub(crate) fn check_rename_auto_increment(&self, t: &TableDef, old: &[Value], new: &[Value]) -> Result<(), Signal> {
        let Some(field) = self.auto_increment_field(t)? else {
            return Ok(());
        };
        if t.keys[0].contains(&field) && number(&old[field]) != number(&new[field]) {
            return Err(Signal::Error(AlError {
                message: format!(
                    "You cannot change the value of an AutoIncrement field.\n\nField: {}\nTable: {}",
                    t.fields[field].name,
                    self.caption(t)
                ),
                code: String::new(),
                call_stack: String::new(),
            }));
        }
        Ok(())
    }
}
