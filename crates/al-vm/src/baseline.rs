//! The install baseline (ADR-013 decision 5, ADR-004): the data a session holds after the
//! dependency apps' install codeunits and `Company-Initialize` ran, captured once and put back
//! at the start of every test codeunit. Only data is part of it: the rows (a persistent map, so
//! the capture is a clone of a root pointer), the counters that must not repeat (system ids,
//! row versions, AutoIncrement), isolated storage and number sequences. Execution state
//! (single-instance codeunits, bindings, handlers) starts fresh, as in a new BC session.

use crate::Session;

/// What a session keeps of the baseline run; see the module comment.
#[derive(Clone)]
pub struct Baseline {
    store: al_data::memory::MemStore,
    next_guid: u64,
    next_row_version: i64,
    committed_row_version: i64,
    autoinc: std::collections::HashMap<(String, u32), i64>,
    isolated_storage: std::collections::HashMap<(String, String), (String, bool)>,
    sequences: std::collections::HashMap<(String, String), crate::sessionops::Sequence>,
    last_stamp: i64,
    /// The workspace app's own install codeunits ran once, before this capture (the runner does it once per run, as
    /// publishing an app does, not once per test codeunit): the tables of those that could not run, and why.
    /// `None`: they did not run here and each test codeunit's session runs them.
    pub workspace_installs: Option<Vec<(std::collections::BTreeSet<u32>, String)>>,
}

impl Baseline {
    /// The table objects that hold a row in the baseline.
    pub fn nonempty_tables(&self) -> std::collections::BTreeSet<u32> {
        self.store.nonempty_tables()
    }
}

impl Session {
    /// Whether a restored company (test data, ADR-020) has rows for table `object` in the session
    /// company. Reading it loads the table.
    pub fn backs_rows(&self, object: u32) -> bool {
        // A company-independent table (DataPerCompany = false) is stored under no company.
        let own = self.engine.tables.get(object as usize).and_then(|t| t.as_ref());
        let company = if own.is_some_and(|t| !t.per_company) {
            ""
        } else {
            &*self.cx.company
        };
        self.store.backed_rows(company, object).is_some_and(|r| !r.is_empty())
    }

    /// Commits and captures the session's data.
    pub fn capture_baseline(&mut self) -> Baseline {
        self.commit_transaction();
        Baseline {
            store: self.store.clone().detached(),
            next_guid: self.next_guid,
            next_row_version: self.next_row_version,
            committed_row_version: self.tx.committed_row_version,
            autoinc: self.autoinc.clone(),
            isolated_storage: self.isolated_storage.clone(),
            sequences: self.sequences.clone(),
            last_stamp: self.last_stamp,
            workspace_installs: None,
        }
    }

    /// Chooses the company the session works in; the empty name is no company, as in
    /// `OnInstallAppPerDatabase`, where `CompanyName()` is empty and a per-company table cannot
    /// be accessed.
    pub fn set_company(&mut self, company: &str) -> String {
        std::mem::replace(&mut self.cx.company, company.into()).to_string()
    }

    /// Replaces the session's data with the baseline's (a new session starts here).
    pub fn restore_baseline(&mut self, b: &Baseline) {
        let provider = self.store.virtual_tables();
        self.store = b.store.clone().attached(provider);
        self.next_guid = b.next_guid;
        self.next_row_version = b.next_row_version;
        self.tx.committed_row_version = b.committed_row_version;
        self.autoinc = b.autoinc.clone();
        self.isolated_storage = b.isolated_storage.clone();
        self.isolated_committed = b.isolated_storage.clone();
        self.sequences = b.sequences.clone();
        self.last_stamp = b.last_stamp;
        self.baseline_rows = b.nonempty_tables();
    }
}
