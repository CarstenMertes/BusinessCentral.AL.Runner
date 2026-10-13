//! Performance budget: cold compile of the pinned corpus (issue #1, handoff
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

/// The handoff budget (docs/handoff.md): at most one second.
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
    let symbols = ["BC27PLUS".to_string()].into();
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
