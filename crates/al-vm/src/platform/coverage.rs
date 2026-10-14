//! The three code coverage tables: Code Coverage (2000000049), Code Coverage Test Lookup
//! (2000000288) and Code Coverage Tests Run (2000000289).
//!
//! The runtime does not record coverage (no per-statement hit counts, no per-test notes), so what
//! the tables would hold after a recording is unknown. They are stored tables that start empty,
//! which is what real BC holds until something is recorded (the corpus calls `DeleteAll` on them
//! first and finds them empty: codeunit/TestCodeCoverageTable.al:`DeleteAll_AfterRecording_LeavesTableEmpty`).
//! Once `CodeCoverageLog(true, ...)` has run, every read of a table is refused until a `DeleteAll`
//! without filters empties that table again, which is the one thing the corpus pins about its
//! content after a recording (same file, `Assert.IsTrue(CodeCoverage.IsEmpty(), 'Code Coverage must
//! be empty after DeleteAll.')`).

use crate::Session;
fn unsupported<T>(what: impl Into<String>) -> Result<T, Signal> {
    Err(Signal::Unsupported(what.into()))
}
use al_builtins::native::Signal;
use al_builtins::filter::FieldFilter;
use al_data::TableDef;

pub(crate) const COVERAGE_TABLES: [u32; 3] = [2_000_000_049, 2_000_000_288, 2_000_000_289];

impl Session {
    fn coverage_table(&self, t: &TableDef) -> Option<u32> {
        let id = self.engine.program.objects[t.object as usize].id?;
        COVERAGE_TABLES.contains(&id).then_some(id)
    }

    /// Recording started: the tables may hold what it recorded.
    pub(crate) fn coverage_started(&mut self) {
        for id in COVERAGE_TABLES {
            if !self.coverage_dirty.contains(&id) {
                self.coverage_dirty.push(id);
            }
        }
    }

    /// A read of a coverage table after recording started is refused.
    pub(crate) fn coverage_read(&self, t: &TableDef, temporary: bool) -> Result<(), Signal> {
        if self.coverage_dirty.is_empty() || temporary {
            return Ok(());
        }
        match self.coverage_table(t) {
            Some(id) if self.coverage_dirty.contains(&id) => unsupported(format!(
                "reading {} after code coverage recording: the runtime records no coverage, so what BC holds here is not known",
                t.name
            )),
            _ => Ok(()),
        }
    }

    /// `DeleteAll` without filters leaves the table empty whatever recording put there.
    pub(crate) fn coverage_cleared(&mut self, t: &TableDef, temporary: bool, filters: &[FieldFilter]) {
        if temporary || !filters.is_empty() || self.coverage_dirty.is_empty() {
            return;
        }
        if let Some(id) = self.coverage_table(t) {
            self.coverage_dirty.retain(|d| *d != id);
        }
    }
}
