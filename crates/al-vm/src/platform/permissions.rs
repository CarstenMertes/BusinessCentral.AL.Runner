//! The permission set tables: Metadata Permission Set, Permission Set and Aggregate Permission Set. Their rows
//! are the permission set objects of every loaded app (compiled or not: the scan reads `Caption` and
//! `Assignable`), and for the Aggregate table also the tenant's own sets, read live from the stored
//! Tenant Permission Set table.
//!
//! Pinned by corpus permissions/TestMetadataPermissionSet.al, TestPermissionSetTable.al,
//! TestPermissionSetUndeclaredAssignable.al and access-control/TestAggregatePermissionSetTable.al:
//! - "Role ID" is the object name, upper-cased (a Code field) and cut to the field length; "Name" is the
//!   Caption with its own casing, else the upper-case Role ID;
//! - "Assignable" is the declared property and false when it is not declared;
//! - SUPER and SECURITY carry the null App ID (the object is declared in System Application);
//!   from BC 29 a lookup by key under any App ID finds that row (`Gate::SuperPermissionSetIgnoresAppId`);
//! - the legacy Permission Set table lists only assignable sets when it is walked, but a read by the
//!   whole primary key finds a non-assignable one too;
//! - the System scope of the Aggregate table is the same set of assignable sets, and its Tenant scope
//!   is the Tenant Permission Set rows that are assignable, as they are now.

use super::mat::{Entry, Mat};
use super::objects::{Catalog, base_kind, prop, string_lit, truthy};
use super::{Providers, blank, pos};
use al_builtins::filter::{FieldFilter, FilterExpr};
use al_builtins::value::Value;
use al_data::{AccessContext, Scan, Store, StoreError, TableDef};
use al_data::memory::MemStore;
use std::rc::Rc;

const TENANT_PERMISSION_SET: u32 = 2_000_000_165;

/// A permission set object of the program.
pub(super) struct PSet {
    pub app: [u8; 16],
    /// Upper-case object name.
    pub role: String,
    pub name: String,
    pub app_name: String,
    pub assignable: bool,
}

fn upper_trunc(s: &str, max: usize) -> String {
    s.to_uppercase().chars().take(max).collect()
}

pub(super) fn sets(p: &Providers) -> Vec<PSet> {
    let engine = p.engine();
    let c = Catalog::new(&engine);
    c.objects
        .iter()
        .filter(|o| base_kind(o) == "permissionset" && o.id.is_some())
        .map(|o| {
            let role = o.name.to_uppercase();
            let null_app = role == "SUPER" || role == "SECURITY";
            let app = if null_app { [0; 16] } else { c.owner(o) };
            let caption = prop(o, "Caption").and_then(string_lit);
            PSet {
                app,
                name: caption.unwrap_or_else(|| role.clone()),
                app_name: c.apps.get(o.app as usize).map(|a| a.name.clone()).unwrap_or_default(),
                role,
                // An undeclared Assignable is false (TestPermissionSetUndeclaredAssignable.al:41-80).
                assignable: prop(o, "Assignable").is_some_and(truthy),
            }
        })
        .collect()
}

/// Metadata Permission Set: "App ID" (1), "Role ID" (2), Name (3), Assignable (4).
pub(super) fn metadata_permission_set_rows(p: &Providers, t: &TableDef) -> Vec<Entry> {
    sets(p)
        .into_iter()
        .map(|s| {
            let mut r = blank(t);
            r[pos(t, 1)] = Value::Guid(s.app);
            r[pos(t, 2)] = Value::Code(upper_trunc(&s.role, 30));
            r[pos(t, 3)] = Value::Text(s.name.chars().take(30).collect());
            r[pos(t, 4)] = Value::Boolean(s.assignable);
            Entry::ok(r)
        })
        .collect()
}

/// Permission Set: "Role ID" (1, Code[20]), Name (2), Hash (3, blank for a set served from metadata).
pub(super) fn permission_set_rows(p: &Providers, t: &TableDef, assignable_only: bool) -> Vec<Entry> {
    sets(p)
        .into_iter()
        .filter(|s| s.assignable || !assignable_only)
        .map(|s| {
            let mut r = blank(t);
            r[pos(t, 1)] = Value::Code(upper_trunc(&s.role, 20));
            r[pos(t, 2)] = Value::Text(s.name.chars().take(30).collect());
            Entry::ok(r)
        })
        .collect()
}

/// Whether the filters name one "Role ID" exactly: such a read is served by the key and finds a set that is
/// not assignable too (TestPermissionSetUndeclaredAssignable.al:82-92).
pub(super) fn pins_the_primary_key(t: &TableDef, filters: &[FieldFilter]) -> bool {
    let role = pos(t, 1);
    filters.iter().any(|f| {
        f.field == role
            && match &f.expr {
                FilterExpr::Eq(_) => true,
                FilterExpr::Range(Some(a), Some(b)) => al_builtins::ops::compare(a, b).is_ok_and(|o| o.is_eq()),
                _ => false,
            }
    })
}

/// Aggregate Permission Set: Scope (1), "App ID" (2), "Role ID" (3), Name (4), "App Name" (5).
pub(super) fn aggregate_rows(p: &Providers, store: &MemStore, cx: &AccessContext, t: &TableDef) -> Result<Vec<Entry>, StoreError> {
    let mut out = Vec::new();
    for s in sets(p).into_iter().filter(|s| s.assignable) {
        let mut r = blank(t);
        r[pos(t, 1)] = super::option(t, 1, 0);
        r[pos(t, 2)] = Value::Guid(s.app);
        r[pos(t, 3)] = Value::Code(upper_trunc(&s.role, 20));
        r[pos(t, 4)] = Value::Text(s.name.chars().take(30).collect());
        r[pos(t, 5)] = Value::Text(s.app_name);
        out.push(Entry::ok(r));
    }
    if let Some(tenant) = p.tenant_permission_set_def() {
        let mut after: Option<Vec<Value>> = None;
        loop {
            let scan = Scan {
                key: 0,
                filters: &[],
                ascending: true,
                after: after.as_deref(),
            };
            let Some(row) = store.next(cx, &tenant, &scan)? else { break };
            after = Some(al_data::position(&tenant, 0, &row));
            let at = |id: u32| row[pos(&tenant, id)].clone();
            if !matches!(at(4), Value::Boolean(true)) {
                continue;
            }
            let mut r = blank(t);
            r[pos(t, 1)] = super::option(t, 1, 1);
            r[pos(t, 2)] = at(1);
            r[pos(t, 3)] = at(2);
            r[pos(t, 4)] = at(3);
            out.push(Entry::ok(r));
        }
    }
    Ok(out)
}

impl Providers {
    /// The schema of the stored Tenant Permission Set table, if the program compiled it.
    pub(super) fn tenant_permission_set_def(&self) -> Option<TableDef> {
        let engine = self.engine();
        let oi = engine
            .program
            .objects
            .iter()
            .position(|o| o.kind == "table" && o.id == Some(TENANT_PERMISSION_SET) && o.table.is_some())?;
        engine.tables[oi].clone()
    }
}

pub(super) type Cache = std::cell::RefCell<Option<(u64, Rc<Mat>)>>;
