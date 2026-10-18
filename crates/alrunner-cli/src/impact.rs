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
use al_testrun::runtime::{NarrowInput, TestResult};
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
    /// Changed, and neither it nor an earlier test of its codeunit reached an impact point in the index's run: the
    /// index missed something (a runtime bug, ADR-043 section 2.1).
    SelectionMiss,
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
    pub const ALL: [Class; 9] = [
        Class::NewlyFailing,
        Class::FailsDifferently,
        Class::NewlyRefused,
        Class::SelectionMiss,
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
            Class::SelectionMiss => "selection miss",
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
            Class::NewlyFailing | Class::FailsDifferently | Class::NewlyRefused | Class::SelectionMiss
        )
    }

    /// The outcome changed.
    fn changed(self) -> bool {
        !matches!(self, Class::SameReached | Class::SameNotReached | Class::NotVerified)
    }
}

/// The class of one test, from its outcome without and with the extensions (`None`: the test is missing on that
/// side, which counts as not run) and whether it entered extension code.
pub fn classify(without: Option<&Outcome>, with: Option<&Outcome>, reached: bool) -> Class {
    use Outcome::*;
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
    /// The impact points it reached in the index's run (dynamic selection).
    pub points: Vec<String>,
}

/// The rows of both sides, in the order of the "without" run, then any test only the "with" run has.
pub fn diff<'a>(without: &'a [TestResult], with: &'a [TestResult], routines: &Routines) -> Vec<Row<'a>> {
    diff_reached(without, with, routines, None)
}

/// The impact points a test of the "without" side reached in the index's run.
type PointsOf<'f> = dyn Fn(&TestResult) -> Vec<String> + 'f;

/// [`diff`] for dynamic selection: `points_of` names the impact points each test of the "without" side reached in the
/// index's run. A test that reached one counts as reached, and a changed test that did not, after no test of its
/// codeunit that did, is a selection miss.
pub fn diff_reached<'a>(
    without: &'a [TestResult],
    with: &'a [TestResult],
    routines: &Routines,
    points_of: Option<&PointsOf<'_>>,
) -> Vec<Row<'a>> {
    let key = |r: &TestResult| (r.app, r.codeunit, r.test.clone());
    let with_by: HashMap<_, &TestResult> = with.iter().map(|r| (key(r), r)).collect();
    let mut seen = std::collections::HashSet::new();
    let mut rows = Vec::new();
    let row = |w: Option<&'a TestResult>, x: Option<&'a TestResult>| {
        let entered = x.map(|x| routines.entered(&x.coverage)).unwrap_or_default();
        let points = match (points_of, w) {
            (Some(f), Some(w)) => f(w),
            _ => Vec::new(),
        };
        Row {
            class: classify(
                w.map(|r| &r.outcome),
                x.map(|r| &r.outcome),
                !entered.is_empty() || !points.is_empty(),
            ),
            without: w,
            with: x,
            entered,
            points,
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
    if points_of.is_some() {
        // Tests of one codeunit share its state: a change after a test that reached an impact point is no miss.
        let mut reached_codeunits = std::collections::HashSet::new();
        for r in &mut rows {
            let side = r.with.or(r.without).expect("a row has a side");
            let unit = (side.app, side.codeunit);
            if r.points.is_empty() && r.class.changed() && !reached_codeunits.contains(&unit) {
                r.class = Class::SelectionMiss;
            }
            if !r.points.is_empty() {
                reached_codeunits.insert(unit);
            }
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
    let (before, after) = (outcome_word(row.without), outcome_word(row.with));
    let mut head = format!("  {} {}.{}: ", any.codeunit, any.codeunit_name, any.test);
    if before == after {
        head.push_str(&after);
    } else {
        head.push_str(&format!("{before} -> {after}"));
    }
    if let Some(m) = message(row.with) {
        head.push_str(&format!(": {}", output::first_line(m, 300)));
    }
    let mut lines = vec![head];
    // A failure the runner does not trust (an earlier test of the codeunit did not run) is reported as not run, with
    // the failure at the end of the reason: show it, it is what the extension changed.
    if let Some(m) = message(row.with)
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
    if !row.points.is_empty() {
        let mut shown: Vec<&str> = row.points.iter().take(3).map(String::as_str).collect();
        let more = row.points.len().saturating_sub(3);
        let tail = format!("and {more} more");
        if more > 0 {
            shown.push(&tail);
        }
        lines.push(format!("    reached: {}", shown.join(", ")));
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
    report_with(rows, routines, verbose, None)
}

fn plural(n: usize, word: &str) -> String {
    format!("{n} {word}{}", if n == 1 { "" } else { "s" })
}

/// [`report`], with the tests dynamic selection left out: (tests, codeunits).
pub fn report_with(
    rows: &[Row],
    routines: &Routines,
    verbose: bool,
    not_selected: Option<(usize, usize)>,
) -> Vec<String> {
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
    if let Some((tests, codeunits)) = not_selected {
        out.push(format!(
            "not selected: {} in {}: they reached no impact point of the extension in the index's run, so it cannot change them",
            plural(tests, "test"),
            plural(codeunits, "codeunit")
        ));
    }
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

/// The names of the APPs, for the first line of the run.
fn names(paths: &[std::path::PathBuf]) -> String {
    paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// The options both sides of an impact run share.
fn side_args(a: &TestArgs) -> TestArgs {
    let mut side = a.clone();
    side.impact_of.clear();
    side.impact = None;
    side.test_apps = Some(a.apps.len());
    side.quiet = !a.verbose;
    // Microsoft's disabled tests are left out of an impact run (ADR-043 owner decision 2).
    side.parity = a.disabled_tests_dir.is_some();
    side
}

/// The command that builds the index this run lacks: the run's own arguments, as `impact-index build`, without the
/// extension (ADR-043 section 2.3: it repeats the test apps and flags exactly).
fn build_command(args: &[String]) -> String {
    let mut out = vec!["alrunner".to_string(), "impact-index".into(), "build".into()];
    let mut it = args.iter().skip_while(|a| *a != "test").skip(1);
    while let Some(a) = it.next() {
        if a == "--impact-of" || a == "--impact" {
            it.next();
            continue;
        }
        out.push(
            if a.is_empty() || a.contains(|c: char| c.is_whitespace() || c == '\'') {
                format!("'{}'", a.replace('\'', "'\\''"))
            } else {
                a.clone()
            },
        );
    }
    out.join(" ")
}

/// The warning of a run with no cached index (ADR-043 section 2.3, owner decision 1).
fn no_index_warning(a: &TestArgs) -> String {
    let args: Vec<String> = std::env::args().collect();
    format!(
        "warning: no impact index for these test apps and options (country {}, this runtime build): every selected test runs without and with the extension, which costs two runs of the test apps. With an index, only the test codeunits that reach one of the extension's impact points run, once. Static selection, the fallback ADR-043 plans for a run without an index, is not built yet. Build the index once with: {}",
        a.country(),
        build_command(&args)
    )
}

/// `alrunner test --impact-of ...`: with a cached index (and not `--impact all`) dynamic selection, else both runs and
/// the diff. Exit 1 when a test newly fails, fails differently, is newly not run or is a selection miss; a run that
/// selected nothing or stopped exits as that run did.
pub fn run(a: &TestArgs) -> Result<u8, String> {
    if a.impact.as_deref() == Some("all") {
        return run_all(a, None);
    }
    match crate::impact_index::lookup(&side_args(a))? {
        Some((index, path)) => run_dynamic(a, index, &path),
        None => run_all(a, Some(no_index_warning(a))),
    }
}

/// Both sides run every selected test (`--impact all`, or no index).
fn run_all(a: &TestArgs, warning: Option<String>) -> Result<u8, String> {
    let side = side_args(a);
    println!(
        "impact of {} on the tests of {}: every selected test runs without, then with the extension{}",
        names(&a.impact_of),
        names(&a.apps),
        if warning.is_some() { "" } else { " (--impact all)" }
    );
    if let Some(w) = &warning {
        println!("{}", style::yellow(w));
    }

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
    if let Some(w) = &warning {
        println!("{}\n", style::yellow(w));
    }
    for line in report(&rows, &routines, a.verbose) {
        println!("{line}");
    }
    Ok(verdict(&rows))
}

fn verdict(rows: &[Row]) -> u8 {
    let breaking = rows.iter().filter(|r| r.class.breaks()).count();
    let verdict = if breaking > 0 {
        style::red(&format!(
            "Result: {breaking} test(s) changed for the worse with the extension (exit 1)"
        ))
    } else {
        style::green("Result: no test changed for the worse with the extension (exit 0)")
    };
    println!("\n{}", style::bold(&verdict));
    u8::from(breaking > 0)
}

/// What dynamic selection decided once the "with" side was compiled.
enum Plan {
    /// Run these test codeunits (caller's app index, id).
    Select(Vec<(usize, u32)>),
    /// The index cannot select for some impact points: run every test codeunit; the reasons say why.
    Everything(Vec<String>),
    /// The index was recorded on other Microsoft apps: it cannot be used.
    Stale(String),
}

struct Decision {
    plan: Plan,
    /// The test codeunits the run would run without selection.
    would: Vec<(usize, u32)>,
    /// Per test of the index (app, codeunit, test): the impact points it reached.
    reached: std::collections::HashMap<(usize, u32, String), Vec<String>>,
}

/// Dynamic selection over `index` for the compiled "with" side (ADR-043 section 2.2).
fn decide(index: &crate::impact_index::Index, input: &NarrowInput) -> Decision {
    use al_testrun::impact_trace::{Matcher, impact_points};
    let program = input.program;
    let mut would: Vec<(usize, u32)> = match input.only {
        Some(o) => o.to_vec(),
        None => index.tests.iter().map(|t| (t.app, t.codeunit)).collect(),
    };
    would.sort_unstable();
    would.dedup();
    let mut d = Decision {
        plan: Plan::Select(Vec::new()),
        would,
        reached: Default::default(),
    };
    // The Microsoft apps must be the ones the index was recorded on.
    for (id, name, version) in &index.apps {
        match program.apps.iter().find(|p| &p.id == id) {
            Some(p) if &p.version == version => {}
            Some(p) => {
                d.plan = Plan::Stale(format!("{name} is {} here and {version} in the index", p.version));
                return d;
            }
            None => {
                d.plan = Plan::Stale(format!("{name} {version} of the index is not in this run"));
                return d;
            }
        }
    }
    // The extensions: every app of the compile the index's run did not have.
    let known: std::collections::HashSet<&str> = index.apps.iter().map(|(id, _, _)| id.as_str()).collect();
    let extension: Vec<bool> = program.apps.iter().map(|p| !known.contains(p.id.as_str())).collect();
    let points = impact_points(program, |oi| {
        extension
            .get(program.objects[oi].app as usize)
            .copied()
            .unwrap_or(false)
    });
    let mut reasons: Vec<String> = points
        .iter()
        .filter_map(|p| match &p.matcher {
            Matcher::Unsupported(why) => Some(why.clone()),
            _ => None,
        })
        .collect();
    // An impact point the install code reached changes the data every test starts from.
    if let Some(baseline) = index.baseline_keys() {
        let set: std::collections::HashSet<&str> = baseline.iter().map(String::as_str).collect();
        for p in points
            .iter()
            .filter(|p| p.runs_code && !matches!(p.matcher, Matcher::Unsupported(_)))
        {
            if p.reached_by(&baseline, |k| set.contains(k)) {
                reasons.push(format!(
                    "{}: the install code reached it in the index's run, so it changes the data every test starts from",
                    p.what
                ));
            }
        }
    }
    let mut per_point = vec![0usize; points.len()];
    let mut selected = std::collections::BTreeSet::new();
    for t in &index.tests {
        let keys = index.keys_of(t);
        let set: std::collections::HashSet<&str> = keys.iter().map(String::as_str).collect();
        let mut hit = Vec::new();
        for (i, p) in points.iter().enumerate() {
            if !matches!(p.matcher, Matcher::Unsupported(_)) && p.reached_by(&keys, |k| set.contains(k)) {
                per_point[i] += 1;
                hit.push(p.what.clone());
            }
        }
        if !hit.is_empty() {
            selected.insert((t.app, t.codeunit));
            d.reached.insert((t.app, t.codeunit, t.test.to_lowercase()), hit);
        }
    }
    println!("dynamic selection: {} impact point(s) of the extension", points.len());
    for (p, n) in points.iter().zip(&per_point) {
        match &p.matcher {
            Matcher::Unsupported(_) => println!("  {}: not covered by the index", p.what),
            _ => println!("  {}: reached by {} of {} tests", p.what, n, index.tests.len()),
        }
    }
    d.plan = if reasons.is_empty() {
        Plan::Select(d.would.iter().copied().filter(|u| selected.contains(u)).collect())
    } else {
        Plan::Everything(reasons)
    };
    d
}

/// Dynamic selection: the codeunits that reach an impact point run once, with the extension; the outcomes without it
/// come from the index.
fn run_dynamic(a: &TestArgs, index: crate::impact_index::Index, path: &std::path::Path) -> Result<u8, String> {
    use std::sync::{Arc, Mutex};
    println!(
        "impact of {} on the tests of {}: dynamic selection with the impact index {}",
        names(&a.impact_of),
        names(&a.apps),
        path.display()
    );
    let index = Arc::new(index);
    let decision: Arc<Mutex<Option<Decision>>> = Arc::new(Mutex::new(None));
    let (ix, dec) = (index.clone(), decision.clone());
    let narrow = al_testrun::runtime::Narrow(Arc::new(move |input: &NarrowInput| {
        let d = decide(&ix, input);
        let only = match &d.plan {
            Plan::Select(units) => {
                println!("selected {} of {} test codeunits", units.len(), d.would.len());
                Some(units.clone())
            }
            Plan::Everything(reasons) => {
                println!(
                    "{}",
                    style::yellow(&format!(
                        "warning: the index cannot select for every impact point of the extension, so all {} test codeunits run with it:",
                        d.would.len()
                    ))
                );
                for r in reasons {
                    println!("  {r}");
                }
                None
            }
            Plan::Stale(_) => Some(Vec::new()),
        };
        *dec.lock().expect("the decision") = Some(d);
        only
    }));

    println!("\nwith the extension:");
    let mut with_side = side_args(a);
    with_side.apps.extend(a.impact_of.iter().cloned());
    with_side.coverage = true;
    let mut with = Collected {
        narrow: Some(narrow),
        ..Default::default()
    };
    let code = run::test_collect(&with_side, &mut with)?;
    let Some(d) = decision.lock().expect("the decision").take() else {
        // The run stopped before its compile was done.
        return Ok(code);
    };
    if let Plan::Stale(why) = &d.plan {
        let w = format!(
            "warning: the impact index {} was recorded on other Microsoft apps ({why}); rebuild it with: {}",
            path.display(),
            build_command(&std::env::args().collect::<Vec<_>>())
        );
        println!("{}", style::yellow(&w));
        return run_all(a, Some(w));
    }
    if !matches!(code, 0 | 1 | 6 | 7 | 8) {
        return Ok(code);
    }
    let ran: std::collections::HashSet<(usize, u32)> = match &d.plan {
        Plan::Select(units) => units.iter().copied().collect(),
        _ => d.would.iter().copied().collect(),
    };
    let present: std::collections::HashSet<(usize, u32, &str)> = with
        .results
        .iter()
        .map(|r| (r.app, r.codeunit, r.test.as_str()))
        .collect();
    let without: Vec<TestResult> = index
        .tests
        .iter()
        .filter(|t| present.contains(&(t.app, t.codeunit, t.test.as_str())))
        .map(|t| index.result(t))
        .collect();
    let would: std::collections::HashSet<&(usize, u32)> = d.would.iter().collect();
    let left_out: Vec<&crate::impact_index::IndexedTest> = index
        .tests
        .iter()
        .filter(|t| would.contains(&(t.app, t.codeunit)) && !ran.contains(&(t.app, t.codeunit)))
        .collect();
    let left_out_units: std::collections::HashSet<(usize, u32)> =
        left_out.iter().map(|t| (t.app, t.codeunit)).collect();

    let tests = a.apps.len();
    let routines = Routines::of(&with.statements, |app| app >= tests);
    let points_of = |r: &TestResult| -> Vec<String> {
        d.reached
            .get(&(r.app, r.codeunit, r.test.to_lowercase()))
            .cloned()
            .unwrap_or_default()
    };
    let rows = diff_reached(&without, &with.results, &routines, Some(&points_of));
    println!();
    for line in report_with(
        &rows,
        &routines,
        a.verbose,
        Some((left_out.len(), left_out_units.len())),
    ) {
        println!("{line}");
    }
    Ok(verdict(&rows))
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
            impact: Vec::new(),
        }
    }

    #[test]
    fn a_failure_the_runner_does_not_trust_shows_its_text_under_newly_refused() {
        let statements = vec![stmt(4, 1, "Handler", 10)];
        let routines = Routines::of(&statements, |app| app >= 1);
        let without = vec![result("T", pass(), vec![])];
        let with = vec![result(
            "T",
            not_run(&format!(
                "a test before this one did not pass (X) and this test draws random numbers; {UNTRUSTED}Assert.AreEqual failed"
            )),
            vec![(4, 2)],
        )];
        let rows = diff(&without, &with, &routines);
        assert_eq!(rows[0].class, Class::NewlyRefused);
        let lines = report(&rows, &routines, false).join("\n");
        assert!(lines.contains("    failure: Assert.AreEqual failed"), "{lines}");
        assert!(lines.contains("    ran: Sub.Handler (2 calls)"), "{lines}");
    }
}
