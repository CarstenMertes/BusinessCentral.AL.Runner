//! The rows a tenant holds before any AL runs, for the stored platform tables whose rows AL reads
//! as facts about the session: the Company, the User and the SUPER Access Control row (ADR-013
//! decision 5, decision 9; AL Runner `RecordPatches.CompanySystemTable`, `UserSystemTable`,
//! `AccessControlSeed`).
//!
//! They are ordinary rows of ordinary tables. AL may read, change and delete them, and a test may
//! insert more (record/TestRecordChangeCompanyRows.al inserts and deletes a company). They are
//! written and committed when the session starts, so a rollback never removes them.

use super::{Ident, pos};
use crate::Session;
use al_builtins::native::Ctx;
use al_builtins::value::{RtType, Value};
use al_data::{Row, Store, TableDef};

pub(super) const COMPANY: u32 = 2_000_000_006;
pub(super) const USER: u32 = 2_000_000_120;
pub(super) const ACCESS_CONTROL: u32 = 2_000_000_053;
pub(super) const USER_PROPERTY: u32 = crate::usertable::USER_PROPERTY;

/// A text value for the field `id`, as a Code or a Text by the field's type.
fn text(t: &TableDef, id: u32, s: &str) -> Value {
    match t.fields[pos(t, id)].ty {
        RtType::Code(_) => Value::Code(s.to_string()),
        _ => Value::Text(s.to_string()),
    }
}

impl Session {
    /// Writes the platform's seed rows into the tables the program compiled, then commits.
    pub(crate) fn seed_platform_rows(&mut self, i: &Ident) {
        let engine = self.engine.clone();
        let mut wrote = false;
        for (oi, o) in engine.program.objects.iter().enumerate() {
            let (Some(id), "table") = (o.id, o.kind.as_str()) else {
                continue;
            };
            let Some(t) = engine.tables[oi].as_ref() else { continue };
            let row: Row = match id {
                COMPANY => {
                    let mut r = super::blank(t);
                    r[pos(t, 1)] = text(t, 1, &i.company);
                    r[pos(t, 8000)] = Value::Guid(i.company_id);
                    r
                }
                USER => {
                    let mut r = super::blank(t);
                    r[pos(t, 1)] = Value::Guid(i.user_sid);
                    r[pos(t, 2)] = text(t, 2, &i.user_id);
                    r[pos(t, 3)] = text(t, 3, &i.user_id);
                    r
                }
                // Every User has a User Property row (onprem access-control/TestUserPropertySessionUser.al:312-330).
                USER_PROPERTY => {
                    let telemetry = self.new_guid();
                    crate::usertable::property_seed(t, i.user_sid, telemetry)
                }
                ACCESS_CONTROL => {
                    let mut r = super::blank(t);
                    r[pos(t, 1)] = Value::Guid(i.user_sid);
                    r[pos(t, 2)] = text(t, 2, "SUPER");
                    r
                }
                _ => continue,
            };
            let mut row = row;
            // Every table has a SystemId; a seeded row gets one like any inserted row.
            if let Some(p) = t.fields.iter().position(|f| f.id == 2_000_000_000) {
                row[p] = Value::Guid(self.new_guid());
            }
            let _ = self.store.insert(&self.cx, t, row);
            wrote = true;
        }
        if wrote {
            self.commit_transaction();
        }
    }
}
