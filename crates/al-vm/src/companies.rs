//! Companies: the company a record works in, `ChangeCompany`, and what the Company table does to
//! the rows of the companies it names.
//!
//! The store keys a per-company table by company (ADR-005), so a record in another company is a
//! record whose access context names that company: no seam changes. A company exists while its row
//! is in the Company table (record/TestRecordChangeCompanyRows.al:128-145), plus the session company.

use crate::Session;
use crate::records::Rec;
use al_builtins::native::Signal;
use al_builtins::ops;
use al_builtins::value::Value;
use al_data::{AccessContext, Store, TableDef};

/// The object id of the platform's Company table.
pub(crate) const COMPANY_TABLE: u32 = 2_000_000_006;

fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}

impl Session {
    /// The Company table, if the program compiled it.
    fn company_table(&self) -> Option<std::sync::Arc<TableDef>> {
        let oi = self
            .engine
            .program
            .objects
            .iter()
            .position(|o| o.id == Some(COMPANY_TABLE) && o.table.is_some())?;
        self.engine.tables[oi].clone()
    }

    /// Whether `t` is the Company table.
    pub(crate) fn is_company_table(&self, t: &TableDef) -> bool {
        self.engine.program.objects[t.object as usize].id == Some(COMPANY_TABLE)
    }

    /// The name of the company `name` refers to, as the Company table spells it. The session
    /// company always exists; another one exists while its row does. A program that never compiled
    /// the Company table has no way to make a company, so the session company is the only one.
    pub(crate) fn find_company(&self, name: &str) -> Result<Option<String>, Signal> {
        if !self.cx.company.is_empty() && name.eq_ignore_ascii_case(&self.cx.company) {
            return Ok(Some(self.cx.company.clone()));
        }
        let Some(t) = self.company_table() else {
            return Ok(None);
        };
        let Ok(key) = ops::convert(Value::Text(name.to_string()), &t.fields[0].ty) else {
            // A name longer than the key field cannot be a company's.
            return Ok(None);
        };
        let row = self
            .store
            .get(&self.cx, &t, &[key])
            .map_err(|e| Signal::Unsupported(format!("store error {e:?}")))?;
        Ok(row.and_then(|r| al_builtins::native::text_of(&r[0])))
    }

    /// The access context of an operation on `rec` (table `t`): the record's company, else the
    /// session's. A record in another company is refused on a table whose rows the install
    /// baseline made for the session company, since a company BC creates holds such rows too.
    pub(crate) fn cx_for(&self, rec: &Rec, t: &TableDef) -> Result<AccessContext, Signal> {
        let r = rec.borrow();
        let Some(company) = r.company.as_ref() else {
            return Ok(self.cx.clone());
        };
        if t.per_company
            && !r.temporary
            && (self.engine.unseeded.contains(&(t.object as usize)) || self.baseline_rows.contains(&t.object))
        {
            return unsupported(format!(
                "{} in company {company}: a company BC creates holds the rows installing the apps and Company-Initialize make there, and this runtime made them only for the session company",
                t.name
            ));
        }
        Ok(AccessContext {
            company: company.clone(),
            user: self.cx.user.clone(),
        })
    }

    /// `CurrentCompany` of `rec`.
    pub(crate) fn current_company_of(&self, rec: &Rec) -> String {
        rec.borrow().company.clone().unwrap_or_else(|| self.cx.company.clone())
    }

    /// The company a name stands for in `Open(.., Company)` and `ChangeCompany`: the empty name and
    /// the session company's name are the session company (`None`); another name must be in the
    /// Company table.
    pub(crate) fn resolve_company(&self, name: &str) -> Result<Option<Option<String>>, Signal> {
        if name.is_empty() {
            return Ok(Some(None));
        }
        Ok(self
            .find_company(name)?
            .map(|c| (!c.eq_ignore_ascii_case(&self.cx.company)).then_some(c)))
    }

    /// `Record.ChangeCompany` and `RecordRef.ChangeCompany`: true once `rec` works in the company;
    /// false when no such company exists. An empty name is the session company
    /// (record/TestRecordGet.al:145-153).
    pub(crate) fn change_company(&mut self, rec: &Rec, args: &[Value]) -> Result<Value, Signal> {
        let name = match args.first() {
            None => String::new(),
            Some(v) => match al_builtins::native::text_of(v) {
                Some(n) => n,
                None => return unsupported(format!("ChangeCompany with {v:?}")),
            },
        };
        let Some(target) = self.resolve_company(&name)? else {
            // `Boolean?`: whether BC raises an error when the answer is dropped is not pinned.
            if !self.ret_used {
                return unsupported(format!(
                    "ChangeCompany to `{name}`, which does not exist, with the result unused (not pinned)"
                ));
            }
            return Ok(Value::Boolean(false));
        };
        let mut r = rec.borrow_mut();
        if r.company == target {
            return Ok(Value::Boolean(true));
        }
        if r.temporary {
            return unsupported("ChangeCompany on a temporary record (not pinned)");
        }
        if !r.filters.is_empty() || !r.marks.is_empty() || r.marked_only {
            return unsupported("ChangeCompany on a record with filters or marks (not pinned)");
        }
        r.company = target;
        Ok(Value::Boolean(true))
    }

    /// Before a Company row goes: the session company's row stays (not pinned); the company's rows
    /// go with it (BC drops a company's data with it; not pinned, docs/probes-needed.md).
    pub(crate) fn before_company_row_delete(&mut self, name: &str) -> Result<(), Signal> {
        if !self.cx.company.is_empty() && name.eq_ignore_ascii_case(&self.cx.company) {
            return unsupported("Delete of the session company's row in the Company table (not pinned)");
        }
        Ok(())
    }

    /// After a Company row went: the company's rows are gone.
    pub(crate) fn after_company_row_delete(&mut self, name: &str) {
        self.store.drop_company(name);
    }
}
