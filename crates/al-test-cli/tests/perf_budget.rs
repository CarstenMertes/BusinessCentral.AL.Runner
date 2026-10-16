//! Performance budget: cold compile of the pinned corpus (issue #1, docs/architecture.md). The budget is in instructions
//! per object (see below); the wall time is printed for information.
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

#[path = "support/meter.rs"]
mod meter;

use al_test_cli::runtime;
use meter::Meter;
use std::path::{Path, PathBuf};
use std::time::Instant;

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

/// The cold compile is budgeted in instructions per compiled object, not in seconds. On a shared CI runner the same code
/// measured 828 and then 955 ms (and 2.4 and then 3.0 s of CPU) between two runs a day apart; the instruction count of
/// every commit is the same to within 1 % on every machine, and per object it does not break when the corpus grows.
/// The limit is the value of the day plus 5 %. Where the kernel refuses the counter the test falls back to the CPU time of
/// the process per object, with a limit three times as wide, and says so. The wall time is printed for information only.
///
/// ADR-033 step 1 added the link step (every lowered object is made symbolic and linked again, on worker threads): the
/// day's value went from 1.936 to 1.999 million (+3.3 %), so the limit went from 1 990 000 to 2 060 000. The wall time
/// did not grow (the walks run on all cores). Lowering that writes the symbolic form directly saves the first of the two
/// walks.
///
/// ADR-036 step 0 (the side table of expression types, `lower/tys.rs`) costs another 2.3 % (2.059 to 2.106 million), so
/// the limit is 2 120 000 for now. Decision recorded in docs/owner-review.md (2026-10-16). Bound-tree steps 1 and 2 must
/// bring it back below 1 990 000; the limit goes down with them.
const LIMIT_INSTRUCTIONS_PER_OBJECT: u64 = 2_120_000;
const LIMIT_CPU_NS_PER_OBJECT: u64 = 2_000_000;
const RUNS: usize = 3;

#[test]
#[ignore = "perf budget: run with `cargo test --release -p al-test-cli --test perf_budget -- --ignored`"]
fn cold_compile_of_the_pinned_corpus_stays_within_its_instruction_budget() {
    if cfg!(debug_assertions) {
        panic!("perf budgets are only meaningful in --release; this is a debug build");
    }
    assert!(
        root().join("corpus/tests/al-language/app.json").exists(),
        "corpus submodule not checked out: run `git submodule update --init`"
    );
    // The warm-up (file cache, allocator, the thread pools) is not counted.
    let mut objects = compile_corpus();
    // Open before the compile starts its threads: they inherit the counter, so the whole compile is counted.
    let meter = Meter::new(true);
    let counting = matches!(meter, Meter::Instructions(_));
    let mut best = u64::MAX;
    let mut samples = Vec::new();
    for _ in 0..RUNS {
        let t = Instant::now();
        let (n, cost) = meter.measure(compile_corpus);
        objects = n;
        samples.push(format!("{} ({:.0} ms)", cost, t.elapsed().as_secs_f64() * 1e3));
        best = best.min(cost);
    }
    let per_object = best / objects as u64;
    let limit = if counting {
        LIMIT_INSTRUCTIONS_PER_OBJECT
    } else {
        LIMIT_CPU_NS_PER_OBJECT
    };
    eprintln!(
        "cold compile: {objects} objects, {} ({}), least of {RUNS} = {best}, {per_object} per object, limit {limit}; samples: {}",
        meter.name(),
        if counting { "all threads" } else { "process" },
        samples.join(", ")
    );
    assert!(
        objects > 1500,
        "compiled only {objects} objects: the corpus was not loaded"
    );
    assert!(
        limit == 0 || per_object <= limit,
        "the cold compile used {per_object} {} per object, over the limit of {limit}",
        meter.name()
    );
}

/// A full corpus run with Microsoft's apps (Base Application and the rest of `.alpackages`, the
/// test libraries from the artifact cache). Not the owner's target of one second: a guard against
/// the big regressions found by profiling it (a run took 14 to 27 s before; see the commit log of
/// branch perf-1). It is on the runtime phases of `--timings` (compile, install baseline, run tests), not on
/// the whole process: "load apps and dependencies" reads the artifact cache and the network, and took 9.9 s of a
/// 14.7 s run on a runner while it takes about 1 s otherwise (issue 16). The load time is printed and
/// not budgeted.
///
/// The budget is in instructions of the whole process tree (137.1 billion on 2026-10-16) (the meter is opened before the run starts, so the worker
/// threads and a .NET helper count), not in seconds: the seconds moved with the load of the machine (12 s was
/// exceeded at 21.5 s on a machine with a load of 25 and the same code), the instructions do not. The limit is the
/// value of the day plus 5 %. Where the kernel refuses the counter, the old limit on the runtime phases in seconds
/// applies. The seconds are printed for information. A run over the limit is repeated, at most `WITH_APPS_RUNS` times in
/// all, and the best counts: a real regression is over the limit every time.
const WITH_APPS_LIMIT_INSTRUCTIONS: u64 = 144_000_000_000;
const WITH_APPS_BUDGET_SECS: f64 = 12.0;
const WITH_APPS_RUNS: usize = 3;
/// No single test codeunit may take longer: one codeunit that walked 2.9 million computed Date
/// rows took 5 s of the run before.
const SLOWEST_CODEUNIT_SECS: f64 = 2.0;
/// The longest start of one .NET sidecar process (ADR-014), reported by `--timings` apart from the
/// codeunit times. A start is about 0.05 s on a dev box and 1 s on a cold CI runner; one that
/// takes this long is a broken start, not a slow machine.
const SIDECAR_START_SECS: f64 = 10.0;

/// Compile, install baseline and run tests of the `timings:` line of `--timings`, in seconds: the runtime,
/// without the load of the apps and dependencies (artifact cache and network).
fn runtime_phases_secs(err: &str) -> Option<f64> {
    let line = err.lines().find(|l| l.starts_with("timings: "))?;
    let part = |label: &str| -> Option<f64> {
        let at = line.find(label)? + label.len();
        line[at..].split(' ').next()?.parse().ok()
    };
    Some(part("compile ")? + part("install baseline ")? + part("run tests ")?)
}

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
fn the_runtime_phases_leave_out_the_load_of_the_apps() {
    // Run 38598436035 of CI: 14.7 s in all, 9.9 s of it loading.
    let line = "timings: BC 29.0: load apps and dependencies 9.90 s, compile 3.36 s, install baseline 0.30 s, run tests 1.04 s, total 14.62 s";
    let secs = runtime_phases_secs(line).unwrap();
    assert!((secs - 4.70).abs() < 1e-9, "{secs}");
    assert_eq!(runtime_phases_secs("nothing"), None);
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
    let meter = Meter::new(true);
    let counting = matches!(meter, Meter::Instructions(_));
    let mut runs: Vec<(f64, f64, String)> = Vec::new();
    let mut costs: Vec<u64> = Vec::new();
    for _ in 0..WITH_APPS_RUNS {
        let started = Instant::now();
        let (out, cost) = meter.measure(|| {
            std::process::Command::new(env!("CARGO_BIN_EXE_al-test-cli"))
                .arg("--root")
                .arg(root())
                .args(["--bc-version", "29", "--no-ratchet", "--timings"])
                .output()
                .unwrap()
        });
        costs.push(cost);
        let wall = started.elapsed().as_secs_f64();
        let err = String::from_utf8_lossy(&out.stderr).into_owned();
        assert!(out.status.success(), "{err}");
        let runtime = runtime_phases_secs(&err).expect("a `timings:` line in --timings");
        runs.push((runtime, wall, err));
        let within = if counting {
            cost <= WITH_APPS_LIMIT_INSTRUCTIONS
        } else {
            runtime <= WITH_APPS_BUDGET_SECS
        };
        if within {
            break;
        }
    }
    let best = costs.iter().copied().min().unwrap();
    eprintln!(
        "full run with the apps: {best} {} (limit {WITH_APPS_LIMIT_INSTRUCTIONS}), samples: {costs:?}",
        meter.name()
    );
    let (runtime, wall, err) = runs
        .iter()
        .min_by(|a, b| a.0.total_cmp(&b.0))
        .map(|(r, w, e)| (*r, *w, e.clone()))
        .unwrap();
    eprintln!(
        "full run with the apps: runtime phases {runtime:.1} s (whole process {wall:.1} s), samples: {}\n{}",
        runs.iter()
            .map(|(r, w, _)| format!("{r:.1} s ({w:.1} s)"))
            .collect::<Vec<_>>()
            .join(", "),
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
    if counting {
        assert!(
            best <= WITH_APPS_LIMIT_INSTRUCTIONS,
            "the run used {best} instructions at best of {WITH_APPS_RUNS}, over the limit of {WITH_APPS_LIMIT_INSTRUCTIONS} \
             (runtime phases {runtime:.1} s, for information):\n{err}"
        );
    } else {
        assert!(
            runtime <= WITH_APPS_BUDGET_SECS,
            "the runtime phases took {runtime:.1} s at best of {WITH_APPS_RUNS}, over the {WITH_APPS_BUDGET_SECS} s budget:\n{err}"
        );
    }
}
