//! Row providers for the platform's system tables (ADR-013 decision 7, ADR-019).
//!
//! The schema of a platform table is the real declaration in the platform `System` app, compiled
//! like any other table. Its rows are not AL: the platform computes them. A [`Providers`] is
//! handed to the session's store, which asks it for every table it `serves`. Which tables those
//! are is the registry `al_builtins::platform::REGISTRY`; a table with no entry is refused at
//! compile time, so nothing here is ever asked for an unregistered table.
//!
//! A provider reads the compiled program, never the OS (ADR-006). Every observable rule carries
//! the corpus line that pins it; a column or a shape nothing pins is refused, not guessed.

mod apps;
mod date;
mod fields;
mod host_tables;
mod identity;
mod ids;
mod integer;
mod links;
mod mat;
mod objects;
mod pages;
mod permissions;
pub(crate) mod ranges;
mod seed;

use crate::Engine;
use al_builtins::filter::FieldFilter;
use al_builtins::platform::Virtual;
use al_builtins::value::Value;
use al_data::memory::MemStore;
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
    /// The host's time zones, in `No.` order.
    pub time_zones: Vec<al_host::TimeZoneInfo>,
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
            time_zones: id.time_zones(),
        }
    }
}

/// Listings that sessions of one engine share. A listed table's rows depend on the program (and,
/// for three tables, on the host's identity: see [`Providers::shares`]), never on a session's
/// data, so a session that needs a table another session of the same engine listed takes that
/// listing. The Field table alone is 37,000 rows, listed in 0.2 s; a corpus run's test codeunits
/// each have a session of their own and several of them read it.
///
/// Per thread, because the rows hold `Rc`s; a session never leaves its thread. Only the engine
/// used last keeps its listings, which bounds the memory. The engine is named by its `uid`, not
/// held: a held `Arc` or `Weak` would stop `Arc::get_mut` on the engine, which the runner uses.
#[derive(Default)]
struct SharedListings {
    engine: u64,
    tables: HashMap<Virtual, Rc<mat::Mat>>,
}

thread_local! {
    static SHARED: RefCell<SharedListings> = RefCell::default();
}

impl SharedListings {
    fn get(&self, engine: u64, kind: Virtual) -> Option<Rc<mat::Mat>> {
        (self.engine == engine)
            .then(|| self.tables.get(&kind).cloned())
            .flatten()
    }

    fn put(&mut self, engine: u64, kind: Virtual, m: Rc<mat::Mat>) {
        if self.engine != engine {
            self.engine = engine;
            self.tables.clear();
        }
        self.tables.insert(kind, m);
    }
}

/// The rows of every virtual platform table, computed on read.
pub(crate) struct Providers {
    /// Weak: the engine owns nothing of the session, and a baseline snapshot of the store (which
    /// holds this provider) must not keep the engine shared.
    engine: std::sync::Weak<Engine>,
    /// `Engine::uid` of that engine.
    engine_uid: u64,
    ident: Ident,
    /// Per object index: the provider of that table, if it is a virtual platform table.
    served: Vec<Option<Virtual>>,
    /// Tables listed in full, once (they depend on the program and the host only).
    listed: RefCell<HashMap<Virtual, Rc<mat::Mat>>>,
    /// The Aggregate Permission Set rows with the tenant's own sets in them, and the stamp of the
    /// stored Tenant Permission Set table they were composed at.
    aggregate: permissions::Cache,
    grants: permissions::Grants,
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
            engine: Arc::downgrade(engine),
            engine_uid: engine.uid,
            ident,
            served,
            listed: RefCell::default(),
            aggregate: RefCell::default(),
            grants: Default::default(),
        }
    }

    pub(super) fn engine(&self) -> Arc<Engine> {
        self.engine.upgrade().expect("a session outlives its provider's reads")
    }

    /// The Aggregate Permission Set rows as of now: composed again when the stored tenant sets changed.
    fn aggregate(&self, store: &MemStore, cx: &AccessContext, t: &TableDef) -> Result<Rc<mat::Mat>, StoreError> {
        let stamp = self
            .tenant_permission_set_def()
            .map_or(0, |tenant| store.table_stamp(cx, &tenant));
        if let Some((at, m)) = &*self.aggregate.borrow() {
            if *at == stamp {
                return Ok(m.clone());
            }
        }
        let m = Rc::new(mat::Mat::new(t, permissions::aggregate_rows(self, store, cx, t)?));
        *self.aggregate.borrow_mut() = Some((stamp, m.clone()));
        Ok(m)
    }

    /// Whether the listing of `kind` is the same for every session of the engine. Session, Active
    /// Session and Time Zone list the host's identity of this session.
    fn shares(kind: Virtual) -> bool {
        !matches!(kind, Virtual::Session | Virtual::ActiveSession | Virtual::TimeZone)
    }

    fn kind(&self, t: &TableDef) -> Virtual {
        self.served[t.object as usize].expect("serves() was true")
    }

    /// The rows of a table that is listed in full.
    fn listed(&self, t: &TableDef, kind: Virtual) -> Result<Rc<mat::Mat>, StoreError> {
        if let Some(m) = self.listed.borrow().get(&kind) {
            return Ok(m.clone());
        }
        if Self::shares(kind) {
            if let Some(m) = SHARED.with(|s| s.borrow().get(self.engine_uid, kind)) {
                self.listed.borrow_mut().insert(kind, m.clone());
                return Ok(m);
            }
        }
        let rows = match kind {
            Virtual::ApiWebhookEntity => {
                return Err(refused(
                    "API Webhook Entity: the entities of the API pages are listed by the platform and no corpus test pins them",
                ));
            }
            Virtual::TenantMedia => {
                return Err(refused(
                    "Tenant Media: media live in the session's media store, which this table does not read yet (ADR-017)",
                ));
            }
            Virtual::MetadataPermissionSet => permissions::metadata_permission_set_rows(self, t),
            Virtual::PermissionSet => permissions::permission_set_rows(self, t, false),
            Virtual::PermissionSetAssignable => permissions::permission_set_rows(self, t, true),
            Virtual::AggregatePermissionSet => unreachable!("composed per read"),
            Virtual::MetadataPermission | Virtual::ExpandedPermission | Virtual::Permission => {
                unreachable!("composed per read")
            }
            Virtual::NavAppInstalledApp => apps::nav_app_installed_app_rows(self, t),
            Virtual::PublishedApplication => apps::published_application_rows(self, t),
            Virtual::TimeZone => host_tables::time_zone_rows(t, &self.ident),
            Virtual::Session => vec![mat::Entry::ok(identity::session_row(t, &self.ident))],
            Virtual::ActiveSession => vec![mat::Entry::ok(identity::active_session_row(t, &self.ident))],
            Virtual::AllObj => objects::all_obj_rows(self, t),
            Virtual::AllObjWithCaption => objects::all_obj_with_caption_rows(self, t),
            Virtual::CodeunitMetadata => objects::codeunit_metadata_rows(self, t),
            Virtual::TableMetadata => objects::table_metadata_rows(self, t),
            Virtual::PageMetadata => pages::page_metadata_rows(self, t),
            Virtual::PageControlField => pages::page_control_field_rows(self, t),
            Virtual::Field => fields::field_rows(self, t),
            Virtual::Key => fields::key_rows(self, t),
            Virtual::Integer | Virtual::Date | Virtual::WindowsLanguage => unreachable!("generated, never listed"),
        };
        let m = Rc::new(mat::Mat::new(t, rows));
        if Self::shares(kind) {
            SHARED.with(|s| s.borrow_mut().put(self.engine_uid, kind, m.clone()));
        }
        self.listed.borrow_mut().insert(kind, m.clone());
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

    fn get(&self, store: &MemStore, cx: &AccessContext, t: &TableDef, pk: &[Value]) -> Result<Option<Row>, StoreError> {
        match self.kind(t) {
            Virtual::AggregatePermissionSet => self.aggregate(store, cx, t)?.get(t, pk),
            Virtual::MetadataPermission | Virtual::ExpandedPermission | Virtual::Permission => {
                let which = permissions::grant_table(self.kind(t));
                // A read by key names one set; the rest of the key is matched by the table.
                let filters = permissions::key_filters(t, which, pk);
                permissions::grants_mat(self, store, cx, t, which, &filters)?.get(t, pk)
            }
            // From BC 29 a read by key under any App ID finds the null-App-ID row of SUPER and
            // SECURITY (permissions/TestMetadataPermissionSet.al:85-128).
            Virtual::MetadataPermissionSet
                if al_version::gate_on(
                    self.engine().target.as_ref(),
                    al_version::Gate::SuperPermissionSetIgnoresAppId,
                ) && matches!(pk.get(1), Some(Value::Code(r) | Value::Text(r)) if r.eq_ignore_ascii_case("SUPER") || r.eq_ignore_ascii_case("SECURITY")) =>
            {
                let mut key = pk.to_vec();
                key[0] = Value::Guid([0; 16]);
                self.listed(t, Virtual::MetadataPermissionSet)?.get(t, &key)
            }
            Virtual::Integer => integer::get(t, pk),
            Virtual::Date => date::get(t, pk),
            Virtual::WindowsLanguage => host_tables::windows_language_get(t, pk),
            k => self.listed(t, k)?.get(t, pk),
        }
    }

    fn next(&self, store: &MemStore, cx: &AccessContext, t: &TableDef, scan: &Scan) -> Result<Option<Row>, StoreError> {
        match self.kind(t) {
            Virtual::AggregatePermissionSet => self.aggregate(store, cx, t)?.next(t, scan),
            Virtual::MetadataPermission | Virtual::ExpandedPermission | Virtual::Permission => {
                let which = permissions::grant_table(self.kind(t));
                permissions::grants_mat(self, store, cx, t, which, scan.filters)?.next(t, scan)
            }
            // The legacy table lists only assignable sets when walked, all of them when the key is named.
            Virtual::PermissionSet if !permissions::pins_the_primary_key(t, scan.filters) => {
                self.listed(t, Virtual::PermissionSetAssignable)?.next(t, scan)
            }
            Virtual::Integer => integer::next(t, scan),
            Virtual::Date => date::next(t, scan),
            Virtual::WindowsLanguage => host_tables::windows_language_next(t, scan),
            k => self.listed(t, k)?.next(t, scan),
        }
    }

    fn count(
        &self,
        store: &MemStore,
        cx: &AccessContext,
        t: &TableDef,
        filters: &[FieldFilter],
    ) -> Result<usize, StoreError> {
        match self.kind(t) {
            Virtual::AggregatePermissionSet => self.aggregate(store, cx, t)?.count(t, filters),
            Virtual::MetadataPermission | Virtual::ExpandedPermission | Virtual::Permission => {
                let which = permissions::grant_table(self.kind(t));
                permissions::grants_mat(self, store, cx, t, which, filters)?.count(t, filters)
            }
            Virtual::PermissionSet if !permissions::pins_the_primary_key(t, filters) => {
                self.listed(t, Virtual::PermissionSetAssignable)?.count(t, filters)
            }
            Virtual::Integer => integer::count(t, filters),
            Virtual::Date => date::count(t, filters),
            Virtual::WindowsLanguage => host_tables::windows_language_count(t, filters),
            k => self.listed(t, k)?.count(t, filters),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> Arc<Engine> {
        Engine::new(Default::default(), Vec::new())
    }

    fn some_mat() -> Rc<mat::Mat> {
        let t = TableDef {
            object: 0,
            name: "T".into(),
            fields: Vec::new(),
            keys: vec![Vec::new()],
            per_company: false,
        };
        Rc::new(mat::Mat::new(&t, Vec::new()))
    }

    #[test]
    fn engines_have_their_own_uid() {
        assert_ne!(engine().uid, engine().uid);
    }

    #[test]
    fn sessions_of_one_engine_share_a_listing_and_other_engines_do_not() {
        let mut shared = SharedListings::default();
        assert!(shared.get(1, Virtual::Field).is_none());
        let m = some_mat();
        shared.put(1, Virtual::Field, m.clone());
        assert!(Rc::ptr_eq(&shared.get(1, Virtual::Field).unwrap(), &m));
        assert!(shared.get(1, Virtual::AllObj).is_none());
        assert!(shared.get(2, Virtual::Field).is_none());
    }

    #[test]
    fn only_the_engine_used_last_keeps_its_listings() {
        let mut shared = SharedListings::default();
        shared.put(1, Virtual::Field, some_mat());
        shared.put(2, Virtual::AllObj, some_mat());
        assert!(shared.get(1, Virtual::Field).is_none(), "engine 1 was replaced");
        assert!(shared.get(2, Virtual::AllObj).is_some());
    }

    #[test]
    fn a_listing_does_not_stop_arc_get_mut_on_its_engine() {
        // The runner sets `unseeded` on the engine after the install baseline listed tables.
        let mut a = engine();
        let t = TableDef {
            object: 0,
            name: "T".into(),
            fields: Vec::new(),
            keys: vec![Vec::new()],
            per_company: false,
        };
        SHARED.with(|s| {
            s.borrow_mut()
                .put(a.uid, Virtual::Field, Rc::new(mat::Mat::new(&t, Vec::new())))
        });
        assert!(Arc::get_mut(&mut a).is_some());
    }

    #[test]
    fn what_depends_on_the_host_is_not_shared() {
        for kind in [Virtual::Session, Virtual::ActiveSession, Virtual::TimeZone] {
            assert!(!Providers::shares(kind));
        }
        for kind in [
            Virtual::Field,
            Virtual::AllObj,
            Virtual::PermissionSet,
            Virtual::PageMetadata,
        ] {
            assert!(Providers::shares(kind));
        }
    }
}
