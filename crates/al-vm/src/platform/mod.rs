//! Row providers for the platform's system tables (ADR-013 decision 7, ADR-018).
//!
//! The schema of a platform table is the real declaration in the platform `System` app, compiled
//! like any other table. Its rows are not AL: the platform computes them. A [`Providers`] is
//! handed to the session's store, which asks it for every table it `serves`. Which tables those
//! are is the registry `al_builtins::platform::REGISTRY`; a table with no entry is refused at
//! compile time, so nothing here is ever asked for an unregistered table.
//!
//! A provider reads the compiled program, never the OS (ADR-006). Every observable rule carries
//! the corpus line that pins it; a column or a shape nothing pins is refused, not guessed.

mod date;
mod integer;
pub(crate) mod ranges;

use crate::Engine;
use al_builtins::filter::FieldFilter;
use al_builtins::platform::Virtual;
use al_builtins::value::Value;
use al_data::{AccessContext, Row, Scan, StoreError, TableDef, VirtualTables};
use std::sync::Arc;

/// The rows of every virtual platform table, computed on read.
pub(crate) struct Providers {
    /// Per object index: the provider of that table, if it is a virtual platform table.
    served: Vec<Option<Virtual>>,
}

impl Providers {
    pub(crate) fn new(engine: &Arc<Engine>) -> Providers {
        let served = engine
            .program
            .objects
            .iter()
            .map(|o| {
                if o.kind == "table" {
                    o.id.and_then(al_builtins::platform::virtual_table)
                } else {
                    None
                }
            })
            .collect();
        Providers { served }
    }

    fn kind(&self, t: &TableDef) -> Virtual {
        self.served[t.object as usize].expect("serves() was true")
    }
}

/// Position in `t.fields` of the field with id `id`.
pub(crate) fn pos(t: &TableDef, id: u32) -> usize {
    t.fields
        .iter()
        .position(|f| f.id == id)
        .unwrap_or_else(|| panic!("table '{}' has no field {id}: the platform declaration changed", t.name))
}

/// A row of default values; the providers set the columns they know.
pub(crate) fn blank(t: &TableDef) -> Row {
    t.fields.iter().map(|f| Value::default_for(&f.ty)).collect()
}

/// An `Option` value of field `id` with ordinal `n`.
pub(crate) fn option(t: &TableDef, id: u32, n: i32) -> Value {
    match Value::default_for(&t.fields[pos(t, id)].ty) {
        Value::Option(_, m) => Value::Option(n, m),
        other => panic!("field {id} of '{}' is not an option: {other:?}", t.name),
    }
}

pub(crate) fn refused(why: impl Into<String>) -> StoreError {
    StoreError::Refused(why.into())
}

/// Whether `row` passes every filter.
pub(crate) fn passes(row: &Row, filters: &[FieldFilter]) -> bool {
    filters.iter().all(|f| f.expr.matches(&row[f.field]))
}

impl VirtualTables for Providers {
    fn serves(&self, t: &TableDef) -> bool {
        self.served.get(t.object as usize).is_some_and(Option::is_some)
    }

    fn get(&self, cx: &AccessContext, t: &TableDef, pk: &[Value]) -> Result<Option<Row>, StoreError> {
        match self.kind(t) {
            Virtual::Integer => integer::get(t, pk),
            Virtual::Date => date::get(t, pk),
        }
        .map(|r| {
            let _ = cx;
            r
        })
    }

    fn next(&self, _cx: &AccessContext, t: &TableDef, scan: &Scan) -> Result<Option<Row>, StoreError> {
        match self.kind(t) {
            Virtual::Integer => integer::next(t, scan),
            Virtual::Date => date::next(t, scan),
        }
    }

    fn count(&self, _cx: &AccessContext, t: &TableDef, filters: &[FieldFilter]) -> Result<usize, StoreError> {
        match self.kind(t) {
            Virtual::Integer => integer::count(t, filters),
            Virtual::Date => date::count(t, filters),
        }
    }
}
