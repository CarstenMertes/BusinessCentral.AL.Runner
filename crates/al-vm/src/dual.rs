//! What the dual run needs of a session (ADR-034 decision 4): a digest of its data. The executor is `Session::exec_mode`
//! (`exec::ExecMode`, lane A).

use crate::Session;

impl Session {
    /// A hash of the rows of each table the session holds (see `MemStore::digest`).
    pub fn state_digest(&self) -> Vec<(u32, u64)> {
        self.store.digest()
    }
}
