//! Per-phase timings of compiling the corpus, for profiling (issue #1):
//! `cargo run --release -p al-test-cli --example phases [-- ROOT [RUNS]]`.
//!
//! Prints the best of RUNS (default 5) for each phase. "lex only" and "parse
//! only" repeat work that "workspace load" also does, shown to separate the
//! front end's cost; "compile" is the sum of the phases the pipeline runs.

use al_test_cli::runtime;
use std::time::{Duration, Instant};

const NAMES: [&str; 8] = [
    "read files",
    "lex only",
    "parse only",
    "workspace load",
    "members",
    "check",
    "lower",
    "engine new",
];

struct Laps {
    best: [Duration; 8],
    lap: Instant,
}

impl Laps {
    fn mark(&mut self, phase: usize) {
        self.best[phase] = self.best[phase].min(self.lap.elapsed());
        self.lap = Instant::now();
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let root = std::path::PathBuf::from(args.next().unwrap_or_else(|| ".".into()));
    let runs: usize = args.next().map_or(5, |r| r.parse().expect("RUNS is a number"));
    let syms = al_version::corpus_symbols(27);
    let mut laps = Laps {
        best: [Duration::MAX; 8],
        lap: Instant::now(),
    };
    let (mut bytes, mut summary) = (0, String::new());
    // Lowering again without source positions, to show what the positions cost.
    let mut lower_no_pos = Duration::MAX;
    for _ in 0..runs {
        laps.lap = Instant::now();
        let tests = root.join("corpus/tests");
        let apps = vec![
            runtime::load_app(&tests.join("al-language-internals-fixture")).unwrap(),
            runtime::load_app(&tests.join("al-language")).unwrap(),
        ];
        laps.mark(0);
        bytes = apps.iter().flat_map(|a| &a.files).map(|f| f.1.len()).sum();
        for f in apps.iter().flat_map(|a| &a.files) {
            std::hint::black_box(al_syntax::lex(&f.1));
        }
        laps.mark(1);
        let trees: Vec<_> = apps
            .iter()
            .flat_map(|a| &a.files)
            .map(|f| al_syntax::parse(&f.1, &syms))
            .collect();
        laps.mark(2);
        drop(trees);
        laps.lap = Instant::now();
        let ws = al_sema::workspace::Workspace::load(apps, &syms);
        laps.mark(3);
        let members = al_sema::members::build(&ws);
        laps.mark(4);
        let checked = al_sema::check::check(&ws, &members, &al_builtins::catalog::Catalog::load());
        laps.mark(5);
        let lowered = al_sema::lower::lower(&ws, &members, &checked);
        laps.mark(6);
        let t = Instant::now();
        std::hint::black_box(al_sema::lower::lower_with_positions(&ws, &members, &checked, false));
        lower_no_pos = lower_no_pos.min(t.elapsed());
        laps.lap = Instant::now();
        let blocked = lowered
            .unsupported
            .iter()
            .map(|o| o.iter().map(|r| r.first().cloned()).collect())
            .collect();
        let engine = al_vm::Engine::new(lowered.program.clone(), blocked);
        laps.mark(7);
        summary = format!(
            "{} objects, {} source positions, {} diagnostics",
            lowered.program.objects.len(),
            lowered.program.positions.len(),
            checked.diagnostics.len()
        );
        drop(engine);
    }
    println!("corpus: {:.2} MB of .al, {summary}, best of {runs}", bytes as f64 / 1e6);
    for (name, d) in NAMES.iter().zip(laps.best) {
        println!("{name:>15}  {:>8.1} ms", d.as_secs_f64() * 1e3);
    }
    println!(
        "{:>15}  {:>8.1} ms  (the same, without source positions)",
        "lower no pos",
        lower_no_pos.as_secs_f64() * 1e3
    );
    let b = &laps.best;
    let compile = b[0] + b[3] + b[4] + b[5] + b[6];
    println!(
        "{:>15}  {:>8.1} ms  (read + load + members + check + lower)",
        "compile",
        compile.as_secs_f64() * 1e3
    );
}
