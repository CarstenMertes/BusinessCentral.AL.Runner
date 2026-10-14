//! The three code coverage tables: Code Coverage (2000000049), Code Coverage Test Lookup
//! (2000000288) and Code Coverage Tests Run (2000000289).
//!
//! While `CodeCoverageLog(true, ...)` is on, the session records which statements ran (hits per
//! source position), which objects were entered, which `[Test]` procedures started and which
//! methods ran inside each of them. The tables are stored tables that start empty (what real BC
//! holds until something is recorded: codeunit/TestCodeCoverageTable.al:`DeleteAll_AfterRecording_
//! LeavesTableEmpty`). A read of one of them first writes what was recorded into the three tables
//! ([`Session::coverage_flush`]); `DeleteAll` without filters forgets what the table showed.
//!
//! What the corpus pins:
//! - A Code Coverage row's `Line No.` is a line of the object's source file; the line of a
//!   procedure's declaration is a `Trigger/Function` row; a statement is a `Code` row with
//!   `No. of Hits`, 0 for one that did not run (codeunit/TestCodeCoverageTable.al:73-104, lines 33,
//!   36, 37). A tableextension, pageextension and reportextension have `Code` rows with hits
//!   (codeunit/TestCodeCoverageTableExtension.al:201-239).
//! - A `[Test]` procedure that starts while recording has one Tests Run row (its codeunit's id,
//!   the method's name, the owning app) and every method that runs inside it, the test procedure
//!   itself excluded, one Test Lookup row, a codeunit's `OnRun` trigger included
//!   (codeunit/TestCodeCoveragePerTest.al:54-168). A procedure that runs outside any test
//!   leaves no per-test row (same file, `PerTestTables_NoTestStartedWhileRecording_AreEmpty...`).
//! - `Test ID` and `Method ID` are only compared between rows, never to a literal.
//!
//! Not pinned, and set to a stated default (docs/probes-needed.md, "Code coverage rows"): which
//! source lines besides statements and declarations have a row, `Line` (the source text), `Code
//! Coverage Status`, the `Method Id` column, the hits of a `Trigger/Function` row, whether an
//! object that was never entered while recording has rows.

use crate::Session;
use al_builtins::filter::FieldFilter;
use al_builtins::native::Signal;
use al_builtins::value::Value;
use al_data::{Row, Store, TableDef};
use std::collections::{BTreeMap, BTreeSet};

pub(crate) const CODE_COVERAGE: u32 = 2_000_000_049;
pub(crate) const TEST_LOOKUP: u32 = 2_000_000_288;
pub(crate) const TESTS_RUN: u32 = 2_000_000_289;
pub(crate) const COVERAGE_TABLES: [u32; 3] = [CODE_COVERAGE, TEST_LOOKUP, TESTS_RUN];

/// What recording noted since the tables were last emptied.
#[derive(Default)]
pub(crate) struct Recorder {
    /// Hits per position id (`Program::positions`), grown when the first statement runs.
    hits: Vec<u32>,
    /// Objects (indexes into `Program::objects`) with a routine that ran while recording.
    entered: BTreeSet<usize>,
    /// `[Test]` procedures that started while recording; the test id is the index plus one.
    tests: Vec<(usize, usize)>,
    /// (test id, object, routine) of the methods that ran inside a recorded test.
    lookups: BTreeSet<(i32, usize, usize)>,
    /// The recorded test that is running now.
    current_test: Option<i32>,
    /// Something was recorded since the rows were last written.
    dirty: bool,
}

/// What a routine entry noted: the test that was running before it started one.
pub(crate) struct TestScope(Option<i32>);

impl Session {
    /// Counts one run of the statement at position `id`.
    #[inline]
    pub(crate) fn coverage_hit(&mut self, id: u32) {
        let hits = &mut self.recorder.hits;
        let id = id as usize;
        if hits.len() <= id {
            hits.resize(self.engine.program.positions.len().max(id + 1), 0);
        }
        hits[id] += 1;
        self.recorder.dirty = true;
    }

    /// A routine starts while recording. Returns what to restore when it ends, if it started a test.
    pub(crate) fn coverage_enter(&mut self, object: usize, routine: usize) -> Option<TestScope> {
        let engine = self.engine.clone();
        let r = &engine.program.objects[object].routines[routine];
        let rec = &mut self.recorder;
        rec.entered.insert(object);
        rec.dirty = true;
        if r.attributes.iter().any(|a| a.name.eq_ignore_ascii_case("Test")) {
            rec.tests.push((object, routine));
            let previous = rec.current_test.replace(rec.tests.len() as i32);
            return Some(TestScope(previous));
        }
        if let Some(test) = rec.current_test {
            rec.lookups.insert((test, object, routine));
        }
        None
    }

    pub(crate) fn coverage_leave(&mut self, scope: TestScope) {
        self.recorder.current_test = scope.0;
    }

    /// A read of a coverage table: what was recorded is in the tables first.
    pub(crate) fn coverage_read(&mut self, t: &TableDef, temporary: bool) -> Result<(), Signal> {
        if temporary || self.coverage_table(t).is_none() {
            return Ok(());
        }
        self.coverage_flush()
    }

    fn coverage_table(&self, t: &TableDef) -> Option<u32> {
        let id = self.engine.program.objects[t.object as usize].id?;
        COVERAGE_TABLES.contains(&id).then_some(id)
    }

    /// `DeleteAll` without filters leaves the table empty, and what it showed is forgotten.
    pub(crate) fn coverage_cleared(&mut self, t: &TableDef, temporary: bool, filters: &[FieldFilter]) {
        if temporary || !filters.is_empty() {
            return;
        }
        let rec = &mut self.recorder;
        match self.engine.program.objects[t.object as usize].id {
            Some(CODE_COVERAGE) => {
                rec.hits.clear();
                rec.entered.clear();
            }
            Some(TEST_LOOKUP) => rec.lookups.clear(),
            // The lookups name the tests: they go with them.
            Some(TESTS_RUN) => {
                rec.tests.clear();
                rec.lookups.clear();
                rec.current_test = None;
            }
            _ => {}
        }
    }

    /// Writes the recorded data into the three tables, replacing what they held.
    fn coverage_flush(&mut self) -> Result<(), Signal> {
        if !self.recorder.dirty {
            return Ok(());
        }
        self.recorder.dirty = false;
        let engine = self.engine.clone();
        let program = &engine.program;
        for (oi, o) in program.objects.iter().enumerate() {
            let (Some(id), "table") = (o.id, o.kind.as_str()) else {
                continue;
            };
            if !COVERAGE_TABLES.contains(&id) {
                continue;
            }
            let Some(t) = engine.tables[oi].as_ref() else { continue };
            let rows = match id {
                CODE_COVERAGE => self.coverage_rows(t),
                TEST_LOOKUP => self.lookup_rows(t),
                _ => self.tests_run_rows(t),
            };
            let cx = self.cx.clone();
            self.store
                .delete_all(&cx, t, &[])
                .map_err(|e| Signal::Unsupported(format!("{e}")))?;
            for row in rows {
                self.store
                    .insert(&cx, t, row)
                    .map_err(|e| Signal::Unsupported(format!("{e}")))?;
            }
        }
        Ok(())
    }

    /// The option value of field `id` named `member` (ignoring case and spaces).
    fn option_member(t: &TableDef, id: u32, member: &str) -> Value {
        let blank = super::option(t, id, 0);
        let Value::Option(_, names) = &blank else { return blank };
        let want: String = member.chars().filter(|c| !c.is_whitespace()).collect();
        match names.iter().position(|n| {
            n.chars()
                .filter(|c| !c.is_whitespace())
                .collect::<String>()
                .eq_ignore_ascii_case(&want)
        }) {
            Some(i) => Value::Option(i as i32, names.clone()),
            None => blank,
        }
    }

    fn coverage_rows(&self, t: &TableDef) -> Vec<Row> {
        let program = &self.engine.program;
        let rec = &self.recorder;
        // Per object: the declaration lines, and the statement lines with their hits.
        let mut headers: BTreeMap<usize, BTreeSet<u32>> = BTreeMap::new();
        let mut header_ids: BTreeSet<u32> = BTreeSet::new();
        for &oi in &rec.entered {
            for r in &program.objects[oi].routines {
                if let Some(p) = program.pos(r.pos) {
                    headers.entry(oi).or_default().insert(p.line);
                    header_ids.insert(r.pos);
                }
            }
        }
        let mut code: BTreeMap<(usize, u32), u32> = BTreeMap::new();
        for (id, p) in program.positions.iter().enumerate() {
            let oi = p.object as usize;
            if !rec.entered.contains(&oi) || header_ids.contains(&(id as u32)) {
                continue;
            }
            let hits = rec.hits.get(id).copied().unwrap_or(0);
            let e = code.entry((oi, p.line)).or_insert(0);
            *e = (*e).max(hits);
        }
        let mut out = Vec::new();
        let mut row = |oi: usize, line: u32, kind: &str, hits: u32| {
            let o = &program.objects[oi];
            let mut r = super::blank(t);
            r[super::pos(t, 1)] = Self::option_member(t, 1, &o.kind);
            r[super::pos(t, 2)] = Value::Integer(o.id.unwrap_or(0) as i32);
            r[super::pos(t, 3)] = Value::Integer(line as i32);
            r[super::pos(t, 4)] = Self::option_member(t, 4, kind);
            r[super::pos(t, 6)] = Value::Integer(hits as i32);
            out.push(r);
        };
        for (oi, lines) in &headers {
            if program.objects[*oi].id.is_none() {
                continue;
            }
            for &line in lines {
                row(*oi, line, "Trigger/Function", 0);
            }
        }
        for ((oi, line), hits) in &code {
            // A line that holds a declaration is that row, not a statement row.
            if program.objects[*oi].id.is_none() || headers.get(oi).is_some_and(|h| h.contains(line)) {
                continue;
            }
            row(*oi, *line, "Code", *hits);
        }
        out
    }

    fn method_id(routine: usize) -> Value {
        Value::Integer(routine as i32 + 1)
    }

    fn lookup_rows(&self, t: &TableDef) -> Vec<Row> {
        let program = &self.engine.program;
        self.recorder
            .lookups
            .iter()
            .filter_map(|&(test, oi, ri)| {
                let o = &program.objects[oi];
                let mut r = super::blank(t);
                r[super::pos(t, 1)] = Self::option_member(t, 1, &o.kind);
                r[super::pos(t, 2)] = Value::Integer(o.id? as i32);
                r[super::pos(t, 3)] = Self::method_id(ri);
                r[super::pos(t, 4)] = Value::Integer(test);
                Some(r)
            })
            .collect()
    }

    fn tests_run_rows(&self, t: &TableDef) -> Vec<Row> {
        let program = &self.engine.program;
        self.recorder
            .tests
            .iter()
            .enumerate()
            .filter_map(|(i, &(oi, ri))| {
                let o = &program.objects[oi];
                let mut r = super::blank(t);
                r[super::pos(t, 1)] = Value::Integer(i as i32 + 1);
                r[super::pos(t, 2)] = Value::Integer(o.id? as i32);
                r[super::pos(t, 3)] = Self::method_id(ri);
                r[super::pos(t, 4)] = Value::Text(o.routines[ri].name.clone());
                if let Some(m) = self.module_of_app(o.app as usize) {
                    r[super::pos(t, 5)] = Value::Guid(m.id);
                }
                Some(r)
            })
            .collect()
    }
}
