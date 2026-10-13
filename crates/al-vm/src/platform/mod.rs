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
mod identity;
mod ids;
mod integer;
mod mat;
mod objects;
pub(crate) mod ranges;
mod seed;

use crate::Engine;
use al_builtins::filter::FieldFilter;
use al_builtins::platform::Virtual;
use al_builtins::value::Value;
use al_data::{AccessContext, Row, Scan, StoreError, TableDef, VirtualTables};
use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

/// What the host says about the session, read once when the session starts.
#[derive(Clone)]
pub(crate) struct Ident {
    pub user_id: String,
    pub user_sid: [u8; 16],
    pub company: String,
    pub company_id: [u8; 16],
    pub session_id: i32,
    pub instance_id: i32,
    pub host_name: String,
    pub client_type: al_host::ClientType,
    /// When the session started: milliseconds since the Unix epoch.
    pub login_millis: i64,
}

impl Ident {
    pub(crate) fn of(host: &al_host::Host) -> Ident {
        let id = &host.identity;
        Ident {
            user_id: id.user_id(),
            user_sid: id.user_security_id(),
            company: id.company_name(),
            company_id: id.company_id(),
            session_id: id.session_id(),
            instance_id: id.service_instance_id(),
            host_name: id.host_name(),
            client_type: id.current_client_type(),
            login_millis: host.clock.now_utc_millis(),
        }
    }
}

/// The rows of every virtual platform table, computed on read.
pub(crate) struct Providers {
    pub(super) engine: Arc<Engine>,
    ident: Ident,
    /// Per object index: the provider of that table, if it is a virtual platform table.
    served: Vec<Option<Virtual>>,
    /// Tables listed in full, once (they depend on the program and the host only).
    listed: RefCell<HashMap<Virtual, Rc<mat::Mat>>>,
}

impl Providers {
    pub(crate) fn new(engine: &Arc<Engine>, ident: Ident) -> Providers {
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
        Providers {
            engine: engine.clone(),
            ident,
            served,
            listed: RefCell::default(),
        }
    }

    fn kind(&self, t: &TableDef) -> Virtual {
        self.served[t.object as usize].expect("serves() was true")
    }

    /// The rows of a table that is listed in full.
    fn listed(&self, t: &TableDef, kind: Virtual) -> Result<Rc<mat::Mat>, StoreError> {
        if let Some(m) = self.listed.borrow().get(&kind) {
            return Ok(m.clone());
        }
        let rows = match kind {
            Virtual::Session => vec![mat::Entry::ok(identity::session_row(t, &self.ident))],
            Virtual::ActiveSession => vec![mat::Entry::ok(identity::active_session_row(t, &self.ident))],
            Virtual::AllObj => objects::all_obj_rows(self, t),
            Virtual::AllObjWithCaption => objects::all_obj_with_caption_rows(self, t),
            Virtual::CodeunitMetadata => objects::codeunit_metadata_rows(self, t),
            Virtual::TableMetadata => objects::table_metadata_rows(self, t),
            Virtual::Integer | Virtual::Date => unreachable!("generated, never listed"),
        };
        let m = Rc::new(mat::Mat::new(t, rows));
        self.listed.borrow_mut().insert(kind, m.clone());
        let _ = &self.engine;
        Ok(m)
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

impl VirtualTables for Providers {
    fn serves(&self, t: &TableDef) -> bool {
        self.served.get(t.object as usize).is_some_and(Option::is_some)
    }

    fn get(&self, _cx: &AccessContext, t: &TableDef, pk: &[Value]) -> Result<Option<Row>, StoreError> {
        match self.kind(t) {
            Virtual::Integer => integer::get(t, pk),
            Virtual::Date => date::get(t, pk),
            k => self.listed(t, k)?.get(t, pk),
        }
    }

    fn next(&self, _cx: &AccessContext, t: &TableDef, scan: &Scan) -> Result<Option<Row>, StoreError> {
        match self.kind(t) {
            Virtual::Integer => integer::next(t, scan),
            Virtual::Date => date::next(t, scan),
            k => self.listed(t, k)?.next(t, scan),
        }
    }

    fn count(&self, _cx: &AccessContext, t: &TableDef, filters: &[FieldFilter]) -> Result<usize, StoreError> {
        match self.kind(t) {
            Virtual::Integer => integer::count(t, filters),
            Virtual::Date => date::count(t, filters),
            k => self.listed(t, k)?.count(t, filters),
        }
    }
}
