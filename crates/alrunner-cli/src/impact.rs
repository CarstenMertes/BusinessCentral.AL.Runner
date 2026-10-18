//! `alrunner test --impact-of APP --impact all` (ADR-043 step 1: two runs and a diff).
//!
//! Runs the selected tests of the APPs without the extensions, then with them (the extensions given as further
//! APPs, their statements counted per test), and compares the outcome of each test. The report names the
//! extension procedures each test entered. No impact index and no static selection: every selected test runs on
//! both sides (ADR-043 section 6, steps 2 and 5 add those).

use crate::args::TestArgs;
use crate::output;
use crate::run::{self, Collected};
use crate::style;
use al_testrun::coverage::CoverageStatement;
use al_testrun::runtime::TestResult;
use al_testrun::{CaseId, Outcome, refusals};
use std::collections::{BTreeMap, HashMap};

/// How a test's outcome changed (ADR-043 section 3), in report order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Class {
    /// Passed without, fails with.
    NewlyFailing,
    /// Failed on both sides, with another error text.
    FailsDifferently,
    /// Ran without, not run with.
    NewlyRefused,
    /// Failed without, passes with.
    NewlyPassing,
    /// Not run without, runs with.
    NewlyRuns,
    /// The same outcome, and extension code ran.
    SameReached,
    /// The same outcome, and no extension code ran. Step 1 knows only the extension code a test entered; passive
    /// impact points (a new enum value, a field property) are not seen until the impact index (step 2).
    SameNotReached,
    /// Not run on either side.
    NotVerified,
}

impl Class {
    pub const ALL: [Class; 8] = [
        Class::NewlyFailing,
        Class::FailsDifferently,
        Class::NewlyRefused,
        Class::NewlyPassing,
        Class::NewlyRuns,
        Class::SameReached,
        Class::SameNotReached,
        Class::NotVerified,
    ];

    /// The heading of the class in the report.
    pub fn heading(self) -> &'static str {
        match self {
            Class::NewlyFailing => "newly failing",
            Class::FailsDifferently => "fails differently",
            Class::NewlyRefused => "newly refused",
            Class::NewlyPassing => "newly passing",
            Class::NewlyRuns => "newly runs",
            Class::SameReached => "same, reached",
            Class::SameNotReached => "same, not reached",
            Class::NotVerified => "not verified",
        }
    }

    /// Its count in the summary line.
    fn count_label(self) -> &'static str {
        match self {
            Class::SameReached => "same (reached)",
            Class::SameNotReached => "same (not reached)",
            other => other.heading(),
        }
    }

    /// A change the extension's author must look at: exit 1 (ADR-043 section 3).
    pub fn breaks(self) -> bool {
        matches!(
            self,
            Class::NewlyFailing | Class::FailsDifferently | Class::NewlyRefused
        )
    }
}

/// The class of one test, from its outcome without and with the extensions (`None`: the test is missing on that
/// side, which counts as not run) and whether it entered extension code.
pub fn classify(without: Option<&Outcome>, with: Option<&Outcome>, reached: bool) -> Class {
    use Outcome::*;
    // A failure the runner does not trust is a failure here when the test ran on the "without" side: the earlier refused
    // tests of its codeunit are the same on both sides, so what they did not write cannot be what differs (see `run_failure`).
    let unwrapped = run_failure(without, with).map(Fail);
    let with = unwrapped.as_ref().or(with);
    let same = if reached {
        Class::SameReached
    } else {
        Class::SameNotReached
    };
    match (without, with) {
        (Some(Pass), Some(Pass)) => same,
        (Some(Pass), Some(Fail(_))) => Class::NewlyFailing,
        (Some(Fail(a)), Some(Fail(b))) => {
            if a == b {
                same
            } else {
                Class::FailsDifferently
            }
        }
        (Some(Fail(_)), Some(Pass)) => Class::NewlyPassing,
        (Some(Pass | Fail(_)), Some(NotRun(_)) | None) => Class::NewlyRefused,
        (Some(NotRun(_)) | None, Some(Pass | Fail(_))) => Class::NewlyRuns,
        (Some(NotRun(_)) | None, Some(NotRun(_)) | None) => Class::NotVerified,
    }
}

/// The text of a failure the runner does not trust on the "with" side, when the same test ran (passed or failed) on the
/// "without" side. The runner reports such a failure as not run, because an earlier test of the codeunit was refused and
/// might have written what this test reads. An impact run refuses the same earlier tests on both sides, so a test that
/// passes without the extension and fails with it did not fail because of them (ADR-043, "Step 1 as built").
pub fn run_failure(without: Option<&Outcome>, with: Option<&Outcome>) -> Option<String> {
    match (without, with) {
        (Some(Outcome::Pass | Outcome::Fail(_)), Some(Outcome::NotRun(m))) => {
            m.split_once(UNTRUSTED).map(|(_, failure)| failure.to_string())
        }
        _ => None,
    }
}

/// The procedures and triggers of the extensions, from the coverage statements of the "with" run.
pub struct Routines {
    /// `Object.Procedure`, per routine.
    pub names: Vec<String>,
    /// The statement whose runs count the calls of each routine: its first statement in the source.
    first: Vec<u32>,
    /// The routine of each statement of an extension.
    of_pos: HashMap<u32, usize>,
}

impl Routines {
    /// The routines of the statements of the apps `is_extension` accepts (app index as given to the run).
    pub fn of(statements: &[CoverageStatement], is_extension: impl Fn(usize) -> bool) -> Routines {
        let mut index: BTreeMap<(usize, &str, &str, &str), usize> = BTreeMap::new();
        let mut names = Vec::new();
        let mut first: Vec<(u32, u32, u32)> = Vec::new();
        let mut of_pos = HashMap::new();
        for s in statements.iter().filter(|s| is_extension(s.app)) {
            let key = (s.app, s.file.as_str(), s.object.as_str(), s.scope.as_str());
            let r = *index.entry(key).or_insert_with(|| {
                names.push(format!("{}.{}", s.object, s.scope));
                first.push((s.line, s.column, s.pos));
                names.len() - 1
            });
            if (s.line, s.column) < (first[r].0, first[r].1) {
                first[r] = (s.line, s.column, s.pos);
            }
            of_pos.insert(s.pos, r);
        }
        Routines {
            names,
            first: first.into_iter().map(|(_, _, pos)| pos).collect(),
            of_pos,
        }
    }

    /// The extension routines a test entered, with how often: the runs of the routine's first statement (the
    /// most runs of any of its statements if the first did not run). In the order of `names`.
    pub fn entered(&self, coverage: &[(u32, u32)]) -> Vec<(usize, u32)> {
        let mut calls: BTreeMap<usize, (Option<u32>, u32)> = BTreeMap::new();
        for &(pos, runs) in coverage {
            if let Some(&r) = self.of_pos.get(&pos) {
                let e = calls.entry(r).or_insert((None, 0));
                if self.first[r] == pos {
                    e.0 = Some(runs);
                }
                e.1 = e.1.max(runs);
            }
        }
        calls
            .into_iter()
            .filter(|(_, (_, most))| *most > 0)
            .map(|(r, (first, most))| (r, first.unwrap_or(most)))
            .collect()
    }
}

/// One test of the report.
pub struct Row<'a> {
    pub class: Class,
    pub without: Option<&'a TestResult>,
    pub with: Option<&'a TestResult>,
    /// The extension routines it entered on the "with" side, with their calls.
    pub entered: Vec<(usize, u32)>,
}

/// The rows of both sides, in the order of the "without" run, then any test only the "with" run has.
pub fn diff<'a>(without: &'a [TestResult], with: &'a [TestResult], routines: &Routines) -> Vec<Row<'a>> {
    let key = |r: &TestResult| (r.app, r.codeunit, r.test.clone());
    let with_by: HashMap<_, &TestResult> = with.iter().map(|r| (key(r), r)).collect();
    let mut seen = std::collections::HashSet::new();
    let mut rows = Vec::new();
    let row = |w: Option<&'a TestResult>, x: Option<&'a TestResult>| {
        let entered = x.map(|x| routines.entered(&x.coverage)).unwrap_or_default();
        Row {
            class: classify(w.map(|r| &r.outcome), x.map(|r| &r.outcome), !entered.is_empty()),
            without: w,
            with: x,
            entered,
        }
    };
    for w in without {
        let k = key(w);
        let x = with_by.get(&k).copied();
        seen.insert(k);
        rows.push(row(Some(w), x));
    }
    for x in with {
        if !seen.contains(&key(x)) {
            rows.push(row(None, Some(x)));
        }
    }
    rows
}

/// What ends the reason of a failure the runner does not trust, before the failure's text (al-testrun `run.rs`).
const UNTRUSTED: &str = "this failure is not trusted: ";

fn outcome_word(r: Option<&TestResult>) -> String {
    let Some(r) = r else {
        return "missing".into();
    };
    let flag = if matches!(r.outcome, Outcome::Pass) && !r.assumptions.is_empty() {
        " (flagged)"
    } else {
        ""
    };
    match r.outcome {
        Outcome::Pass => format!("pass{flag}"),
        Outcome::Fail(_) => "fail".into(),
        Outcome::NotRun(_) => "not run".into(),
    }
}

fn message(r: Option<&TestResult>) -> Option<&str> {
    match r.map(|r| &r.outcome) {
        Some(Outcome::Fail(m) | Outcome::NotRun(m)) => Some(m),
        _ => None,
    }
}

/// The lines of one test in its class's list.
fn row_lines(row: &Row, routines: &Routines) -> Vec<String> {
    let any = row.with.or(row.without).expect("a row has a side");
    let (before, mut after) = (outcome_word(row.without), outcome_word(row.with));
    // A failure the runner does not trust, where the test ran without the extension, counts as a failure: show it as one.
    let counted = run_failure(row.without.map(|r| &r.outcome), row.with.map(|r| &r.outcome));
    if counted.is_some() {
        after = "fail".into();
    }
    let mut head = format!("  {} {}.{}: ", any.codeunit, any.codeunit_name, any.test);
    if before == after {
        head.push_str(&after);
    } else {
        head.push_str(&format!("{before} -> {after}"));
    }
    match &counted {
        Some(failure) => head.push_str(&format!(": {}", output::first_line(failure, 300))),
        None => {
            if let Some(m) = message(row.with) {
                head.push_str(&format!(": {}", output::first_line(m, 300)));
            }
        }
    }
    let mut lines = vec![head];
    // A failure the runner does not trust and that did not run on the "without" side either stays "not run" with the
    // failure at the end of the reason: show it, it is what the extension changed.
    if counted.is_none()
        && let Some(m) = message(row.with)
        && matches!(row.with.map(|r| &r.outcome), Some(Outcome::NotRun(_)))
        && let Some((_, failure)) = m.split_once(UNTRUSTED)
    {
        lines.push(format!("    failure: {}", output::first_line(failure, 300)));
    }
    if row.class == Class::FailsDifferently
        && let Some(m) = message(row.without)
    {
        lines.push(format!("    without: {}", output::first_line(m, 300)));
    }
    if !row.entered.is_empty() {
        let ran: Vec<String> = row
            .entered
            .iter()
            .map(|&(r, calls)| {
                format!(
                    "{} ({calls} call{})",
                    routines.names[r],
                    if calls == 1 { "" } else { "s" }
                )
            })
            .collect();
        lines.push(format!("    ran: {}", ran.join(", ")));
    }
    lines
}

/// The impact report: the counts per class, then the tests of each class but the unchanged unreached ones, the
/// extension procedures the tests ran, and the refusal reasons of the tests that ran on neither side.
pub fn report(rows: &[Row], routines: &Routines, verbose: bool) -> Vec<String> {
    let mut out = Vec::new();
    let count = |c: Class| rows.iter().filter(|r| r.class == c).count();
    let counts: Vec<String> = Class::ALL
        .iter()
        .map(|&c| format!("{} {}", count(c), c.count_label()))
        .collect();
    out.push(style::bold(&format!(
        "Impact: {} tests: {}",
        rows.len(),
        counts.join(", ")
    )));
    for class in Class::ALL {
        if matches!(class, Class::SameNotReached | Class::NotVerified) || count(class) == 0 {
            continue;
        }
        out.push(String::new());
        let head = format!("{} ({})", class.heading(), count(class));
        out.push(if class.breaks() {
            style::red(&head)
        } else {
            style::bold(&head)
        });
        // Unchanged tests can be many (every test that posts runs a posting subscriber): a few unless --verbose.
        let limit = if class == Class::SameReached && !verbose {
            30
        } else {
            usize::MAX
        };
        for row in rows.iter().filter(|r| r.class == class).take(limit) {
            out.extend(row_lines(row, routines));
        }
        if count(class) > limit {
            out.push(format!("  ... {} more (--verbose lists them)", count(class) - limit));
        }
    }
    // Which extension procedures the tests ran, and in how many tests.
    let mut tests_of: BTreeMap<usize, usize> = BTreeMap::new();
    for row in rows {
        for &(r, _) in &row.entered {
            *tests_of.entry(r).or_default() += 1;
        }
    }
    out.push(String::new());
    out.push(style::bold(&format!(
        "extension procedures the tests ran: {} of {}",
        tests_of.len(),
        routines.names.len()
    )));
    for (r, n) in &tests_of {
        out.push(format!(
            "  {} in {n} test{}",
            routines.names[*r],
            if *n == 1 { "" } else { "s" }
        ));
    }
    let not_verified = count(Class::NotVerified);
    if not_verified > 0 {
        let list = refusals::rank(rows.iter().filter(|r| r.class == Class::NotVerified).filter_map(|r| {
            let side = r.with.or(r.without)?;
            Some((
                CaseId {
                    codeunit: side.codeunit,
                    name: side.test.clone(),
                },
                message(Some(side)).unwrap_or("missing from the run").to_string(),
            ))
        }));
        out.push(String::new());
        out.push(format!(
            "not verified ({not_verified}): not run on either side, so the run cannot say whether the extension changes them"
        ));
        out.push(refusals::render_top("not verified tests", &list, 10));
    }
    out
}

/// `alrunner test --impact-of ...`: both runs, then the report. Exit 1 when a test newly fails, fails
/// differently or is newly not run; a run that selected nothing or stopped exits as that run did.
pub fn run(a: &TestArgs) -> Result<u8, String> {
    let mut side = a.clone();
    side.impact_of.clear();
    side.impact = None;
    side.test_apps = Some(a.apps.len());
    side.quiet = !a.verbose;
    // Microsoft's disabled tests are left out of an impact run (ADR-043 owner decision 2).
    side.parity = a.disabled_tests_dir.is_some();
    let names = |paths: &[std::path::PathBuf]| -> String {
        paths
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    };
    println!(
        "impact of {} on the tests of {}: every selected test runs without, then with the extension (--impact all)",
        names(&a.impact_of),
        names(&a.apps)
    );

    println!("\nwithout the extension:");
    let mut without = Collected::default();
    let code = run::test_collect(&side, &mut without)?;
    if !matches!(code, 0 | 1 | 7 | 8) {
        return Ok(code);
    }

    println!("\nwith the extension:");
    let mut with_side = side.clone();
    with_side.apps.extend(a.impact_of.iter().cloned());
    with_side.coverage = true;
    let mut with = Collected::default();
    let code = run::test_collect(&with_side, &mut with)?;
    if !matches!(code, 0 | 1 | 7 | 8) {
        return Ok(code);
    }

    let tests = a.apps.len();
    let routines = Routines::of(&with.statements, |app| app >= tests);
    let rows = diff(&without.results, &with.results, &routines);
    println!();
    for line in report(&rows, &routines, a.verbose) {
        println!("{line}");
    }
    let breaking = rows.iter().filter(|r| r.class.breaks()).count();
    let code = u8::from(breaking > 0);
    let verdict = if breaking > 0 {
        style::red(&format!(
            "Result: {breaking} test(s) changed for the worse with the extension (exit 1)"
        ))
    } else {
        style::green("Result: no test changed for the worse with the extension (exit 0)")
    };
    println!("\n{}", style::bold(&verdict));
    Ok(code)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pass() -> Outcome {
        Outcome::Pass
    }
    fn fail(m: &str) -> Outcome {
        Outcome::Fail(m.into())
    }
    fn not_run(m: &str) -> Outcome {
        Outcome::NotRun(m.into())
    }

    #[test]
    fn classes_follow_the_outcomes_on_both_sides() {
        let c = |w: Option<Outcome>, x: Option<Outcome>, reached| classify(w.as_ref(), x.as_ref(), reached);
        assert_eq!(c(Some(pass()), Some(fail("x")), true), Class::NewlyFailing);
        assert_eq!(c(Some(fail("a")), Some(fail("b")), true), Class::FailsDifferently);
        assert_eq!(c(Some(fail("a")), Some(fail("a")), true), Class::SameReached);
        assert_eq!(c(Some(fail("a")), Some(fail("a")), false), Class::SameNotReached);
        assert_eq!(c(Some(pass()), Some(not_run("r")), false), Class::NewlyRefused);
        assert_eq!(c(Some(fail("a")), None, false), Class::NewlyRefused);
        assert_eq!(c(Some(fail("a")), Some(pass()), true), Class::NewlyPassing);
        assert_eq!(c(Some(not_run("r")), Some(pass()), true), Class::NewlyRuns);
        assert_eq!(c(Some(pass()), Some(pass()), false), Class::SameNotReached);
        assert_eq!(c(Some(not_run("r")), Some(not_run("s")), false), Class::NotVerified);
        assert!(Class::NewlyFailing.breaks() && Class::FailsDifferently.breaks() && Class::NewlyRefused.breaks());
        assert!(!Class::NewlyPassing.breaks() && !Class::SameReached.breaks() && !Class::NotVerified.breaks());
    }

    fn stmt(pos: u32, app: usize, scope: &str, line: u32) -> CoverageStatement {
        CoverageStatement {
            pos,
            app,
            file: "S.al".into(),
            line,
            column: 9,
            end_line: line,
            scope: scope.into(),
            object: "Sub".into(),
        }
    }

    #[test]
    fn a_routine_counts_the_runs_of_its_first_statement_and_only_extension_apps_count() {
        let statements = vec![
            stmt(1, 0, "TestApp", 1),
            stmt(5, 1, "Handler", 11),
            stmt(4, 1, "Handler", 10),
            stmt(6, 1, "Other", 20),
            stmt(7, 1, "Never", 30),
        ];
        let r = Routines::of(&statements, |app| app >= 1);
        assert_eq!(r.names, ["Sub.Handler", "Sub.Other", "Sub.Never"]);
        // Handler: first statement (line 10, pos 4) ran twice, a later one five times (a loop).
        let entered = r.entered(&[(1, 3), (4, 2), (5, 5), (6, 1)]);
        assert_eq!(entered, [(0, 2), (1, 1)]);
        // A test that ran only the test app's statements entered no extension routine.
        assert!(r.entered(&[(1, 1)]).is_empty());
    }

    fn result(test: &str, outcome: Outcome, coverage: Vec<(u32, u32)>) -> TestResult {
        TestResult {
            suite: 0,
            app: 0,
            codeunit: 50510,
            codeunit_name: "Imp Tests".into(),
            test: test.into(),
            outcome,
            call_stack: String::new(),
            assumptions: Vec::new(),
            time: std::time::Duration::ZERO,
            coverage,
            messages: Vec::new(),
            captured: None,
        }
    }

    #[test]
    fn an_untrusted_failure_is_newly_failing_when_the_test_ran_without_the_extension() {
        let statements = vec![stmt(4, 1, "Handler", 10)];
        let routines = Routines::of(&statements, |app| app >= 1);
        let untrusted = not_run(&format!(
            "an earlier test of this codeunit was refused before it started (X) and can write a table this test reads; {UNTRUSTED}Assert.AreEqual failed"
        ));
        let without = vec![result("T", pass(), vec![])];
        let with = vec![result("T", untrusted.clone(), vec![(4, 2)])];
        let rows = diff(&without, &with, &routines);
        assert_eq!(rows[0].class, Class::NewlyFailing);
        let lines = report(&rows, &routines, false).join("\n");
        assert!(lines.contains("T: pass -> fail: Assert.AreEqual failed"), "{lines}");
        assert!(lines.contains("    ran: Sub.Handler (2 calls)"), "{lines}");
        // The same failure on both sides is the same result, not a new one.
        let without = vec![result("T", fail("Assert.AreEqual failed"), vec![])];
        let rows = diff(&without, &with, &routines);
        assert_eq!(rows[0].class, Class::SameReached);
    }

    #[test]
    fn an_untrusted_failure_stays_not_run_when_the_test_did_not_run_without_the_extension() {
        let routines = Routines::of(&[], |_| false);
        let untrusted = not_run(&format!("x; {UNTRUSTED}boom"));
        let without = vec![result("T", not_run("r"), vec![])];
        let with = vec![result("T", untrusted, vec![])];
        let rows = diff(&without, &with, &routines);
        assert_eq!(rows[0].class, Class::NotVerified);
    }
}
