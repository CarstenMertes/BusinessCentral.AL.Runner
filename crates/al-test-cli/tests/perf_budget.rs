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
/// The longest start of one .NET sidecar process (ADR-014), reported by `--timings` apart from the
/// codeunit times. A start is about 0.05 s on a dev box and 1 s on a cold CI runner; one that
/// takes this long is a broken start, not a slow machine.
const SIDECAR_START_SECS: f64 = 10.0;

/// One entry of the `slowest test codeunits:` line of `--timings`.
#[derive(Debug, PartialEq)]
struct CodeunitLine {
    name: String,
    took: f64,
    sidecar_start: f64,
}

/// The number that ends right before `marker` in `text`: `number_before("a 1.5 s at most", " s at most")`.
fn number_before(text: &str, marker: &str) -> Option<f64> {
    let head = &text[..text.find(marker)?];
    head.rsplit(' ').next()?.parse().ok()
}

/// `69934 Name 7.39 s (sidecar start 7.30 s); 60130 Other 0.50 s`: the name, the time and the part of
/// it spent starting a sidecar (0 when the entry has none).
fn parse_slowest(line: &str) -> Vec<CodeunitLine> {
    line.split("; ")
        .filter_map(|entry| {
            let (entry, start) = match entry.rfind(" (sidecar start ") {
                Some(i) => (&entry[..i], number_before(&entry[i..], " s)")?),
                None => (entry, 0.0),
            };
            let took = entry.trim().strip_suffix(" s")?;
            let (name, took) = took.rsplit_once(' ')?;
            Some(CodeunitLine {
                name: name.to_string(),
                took: took.parse().ok()?,
                sidecar_start: start,
            })
        })
        .collect()
}

#[test]
fn the_slowest_line_is_read_with_and_without_a_sidecar_start() {
    let l =
        parse_slowest("69934 TPF Media Filled Tests 7.39 s (sidecar start 7.30 s); 60130 Test Media Png Import 0.07 s");
    assert_eq!(
        l,
        vec![
            CodeunitLine {
                name: "69934 TPF Media Filled Tests".into(),
                took: 7.39,
                sidecar_start: 7.30
            },
            CodeunitLine {
                name: "60130 Test Media Png Import".into(),
                took: 0.07,
                sidecar_start: 0.0
            },
        ]
    );
    assert_eq!(
        number_before(
            "2 .NET sidecar process(es) started: starting 0.12 s in total, 0.06 s at most; 35 request(s) 0.03 s",
            " s at most"
        ),
        Some(0.06)
    );
}

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
            .filter(|l| l.starts_with("timings")
                || l.starts_with("compile:")
                || l.starts_with("slowest")
                || l.starts_with("dotnet:"))
            .collect::<Vec<_>>()
            .join("\n")
    );
    assert!(
        err.contains("dependency: Base Application"),
        "the Microsoft apps were not loaded:\n{err}"
    );
    // The limit is on the runtime. Starting a .NET helper process is not the runtime and varies
    // with the machine (a cold runner took 1 to 7 s for it), so it is taken off a codeunit's time
    // and checked on its own.
    let line = err
        .lines()
        .find_map(|l| l.strip_prefix("slowest test codeunits: "))
        .expect("a slowest codeunit in --timings");
    let slowest = parse_slowest(line);
    assert!(!slowest.is_empty(), "no codeunit time in: {line}");
    let (name, runtime_secs, start_secs) = slowest
        .iter()
        .map(|c| (c.name.as_str(), c.took - c.sidecar_start, c.sidecar_start))
        .max_by(|a, b| a.1.total_cmp(&b.1))
        .unwrap();
    assert!(
        runtime_secs <= SLOWEST_CODEUNIT_SECS,
        "test codeunit {name} took {runtime_secs:.1} s without its sidecar start ({start_secs:.1} s), \
         over {SLOWEST_CODEUNIT_SECS} s:\n{err}"
    );
    if let Some(d) = err.lines().find_map(|l| l.strip_prefix("dotnet: ")) {
        let max = number_before(d, " s at most").expect("the longest sidecar start in the dotnet line");
        assert!(
            max <= SIDECAR_START_SECS,
            "a .NET sidecar took {max:.1} s to start, over {SIDECAR_START_SECS} s:\n{err}"
        );
    }
    assert!(
        wall <= WITH_APPS_BUDGET_SECS,
        "the run took {wall:.1} s, over the {WITH_APPS_BUDGET_SECS} s budget:\n{err}"
    );
}
