//! Performance budget: cold compile of the pinned corpus (issue #1, docs/architecture.md
//! budget of at most 1 s).
//!
//! "Compile" is everything between the app folders on disk and the lowered
//! program: reading the `.al` files, lex, preprocess, parse, workspace load,
//! member tables, type check, and lowering. It excludes executing the tests.
//!
//! Ignored by default because wall-clock budgets are only meaningful in an
//! optimised build. CI runs it explicitly:
//!
//! ```text
//! cargo test --release -p al-test-cli --test perf_budget -- --ignored
//! ```

use al_test_cli::runtime;
use std::path::{Path, PathBuf};
use std::time::Instant;

/// The compile budget (docs/architecture.md): at most one second.
///
/// The asserted limit is the budget itself, not a looser number. The margin
/// for noisy runners is in the measurement instead: the best of `RUNS`
/// compiles is compared, and the measured time on the dev box is well under
/// half of the budget, so a CI runner that is 2x slower still passes while a
/// regression back to the original 1.8 s fails.
const BUDGET_SECS: f64 = 1.0;
const RUNS: usize = 3;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn compile_corpus() -> usize {
    let tests = root().join("corpus/tests");
    let apps = vec![
        runtime::load_app(&tests.join("al-language-internals-fixture")).unwrap(),
        runtime::load_app(&tests.join("al-language")).unwrap(),
    ];
    let symbols = al_version::corpus_symbols(27);
    let (_ws, lowered) = al_sema::lower::compile(apps, &symbols);
    lowered.program.objects.len()
}

#[test]
#[ignore = "perf budget: run with `cargo test --release -p al-test-cli --test perf_budget -- --ignored`"]
fn cold_compile_of_the_pinned_corpus_stays_within_one_second() {
    if cfg!(debug_assertions) {
        panic!("perf budgets are only meaningful in --release; this is a debug build");
    }
    assert!(
        root().join("corpus/tests/al-language/app.json").exists(),
        "corpus submodule not checked out: run `git submodule update --init`"
    );
    let mut best = f64::MAX;
    let mut objects = 0;
    for _ in 0..RUNS {
        let t = Instant::now();
        objects = compile_corpus();
        best = best.min(t.elapsed().as_secs_f64());
    }
    eprintln!(
        "cold compile: {objects} objects, best of {RUNS} = {:.0} ms, budget {:.0} ms",
        best * 1e3,
        BUDGET_SECS * 1e3
    );
    assert!(
        objects > 1500,
        "compiled only {objects} objects: the corpus was not loaded"
    );
    assert!(
        best <= BUDGET_SECS,
        "cold compile took {best:.2} s, over the {BUDGET_SECS} s budget"
    );
}

/// A full corpus run with Microsoft's apps (Base Application and the rest of `.alpackages`, the
/// test libraries from the artifact cache). Not the owner's target of one second: a guard against
/// the big regressions found by profiling it (a run took 14 to 27 s before; see the commit log of
/// branch perf-1). Wall clock of the whole process, so the limit leaves room for a CI runner with
/// few cores.
const WITH_APPS_BUDGET_SECS: f64 = 12.0;
/// No single test codeunit may take longer: one codeunit that walked 2.9 million computed Date
/// rows took 5 s of the run before.
const SLOWEST_CODEUNIT_SECS: f64 = 2.0;

#[test]
#[ignore = "perf budget: run with `cargo test --release -p al-test-cli --test perf_budget -- --ignored`"]
fn a_full_corpus_run_with_the_microsoft_apps_stays_within_its_budget() {
    if cfg!(debug_assertions) {
        panic!("perf budgets are only meaningful in --release; this is a debug build");
    }
    let started = Instant::now();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_al-test-cli"))
        .arg("--root")
        .arg(root())
        .args(["--bc-version", "29", "--no-ratchet", "--timings"])
        .output()
        .unwrap();
    let wall = started.elapsed().as_secs_f64();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "{err}");
    eprintln!(
        "full run with the apps: {wall:.1} s\n{}",
        err.lines()
            .filter(|l| l.starts_with("timings") || l.starts_with("compile:") || l.starts_with("slowest"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        err.contains("dependency: Base Application"),
        "the Microsoft apps were not loaded:\n{err}"
    );
    let slowest_secs = err
        .lines()
        .find_map(|l| l.strip_prefix("slowest test codeunits: "))
        .and_then(|l| l.split(';').next())
        .and_then(|first| first.trim().strip_suffix(" s"))
        .and_then(|first| first.rsplit(' ').next())
        .and_then(|t| t.parse::<f64>().ok())
        .expect("a slowest codeunit in --timings");
    assert!(
        slowest_secs <= SLOWEST_CODEUNIT_SECS,
        "a test codeunit took {slowest_secs:.1} s, over {SLOWEST_CODEUNIT_SECS} s:\n{err}"
    );
    assert!(
        wall <= WITH_APPS_BUDGET_SECS,
        "the run took {wall:.1} s, over the {WITH_APPS_BUDGET_SECS} s budget:\n{err}"
    );
}
