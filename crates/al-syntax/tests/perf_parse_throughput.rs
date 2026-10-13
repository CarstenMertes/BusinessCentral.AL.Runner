//! Performance budget: parser throughput over the pinned corpus (issue #1).
//!
//! Ignored by default because wall-clock budgets are only meaningful in an
//! optimised build. CI runs it explicitly:
//!
//! ```text
//! cargo test --release -p al-syntax --test perf_parse_throughput -- --ignored
//! ```

use std::path::{Path, PathBuf};
use std::time::Instant;

/// Floor, in MB/s, for lex + preprocess + build the lossless tree, one thread.
///
/// The target set in issue #1 is 50 MB/s. Measured on the 2-core dev box the
/// parser is well above that, so the floor leaves a wide margin for a noisy,
/// slower CI runner; it exists to catch a return of quadratic work (the
/// original lexer re-validated the rest of the file for every character and
/// ran at about 7 MB/s), not to track small drifts.
const FLOOR_MB_PER_S: f64 = 50.0;

/// Best of this many passes over the corpus: the minimum filters out scheduler
/// noise, which can only ever make a pass slower.
const PASSES: usize = 5;

fn corpus_sources() -> Vec<String> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/tests");
    assert!(
        root.join("al-language/app.json").exists(),
        "corpus submodule not checked out: run `git submodule update --init`"
    );
    let mut paths: Vec<PathBuf> = Vec::new();
    let mut stack = vec![root];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "al") {
                paths.push(p);
            }
        }
    }
    paths.sort();
    assert!(
        paths.len() > 1300,
        "expected the whole corpus, found {} files",
        paths.len()
    );
    paths.iter().map(|p| std::fs::read_to_string(p).unwrap()).collect()
}

#[test]
#[ignore = "perf budget: run with `cargo test --release -p al-syntax --test perf_parse_throughput -- --ignored`"]
fn parser_throughput_over_the_corpus_meets_the_floor() {
    if cfg!(debug_assertions) {
        panic!("perf budgets are only meaningful in --release; this is a debug build");
    }
    let sources = corpus_sources();
    let bytes: usize = sources.iter().map(String::len).sum();
    let syms = ["BC27PLUS".to_string()].into();
    let mut best = f64::MAX;
    for _ in 0..PASSES {
        let t = Instant::now();
        for s in &sources {
            std::hint::black_box(al_syntax::parse(s, &syms));
        }
        best = best.min(t.elapsed().as_secs_f64());
    }
    let mb_per_s = bytes as f64 / 1e6 / best;
    eprintln!(
        "parser: {:.1} MB in {:.0} ms (best of {PASSES}) = {mb_per_s:.1} MB/s, floor {FLOOR_MB_PER_S}",
        bytes as f64 / 1e6,
        best * 1e3
    );
    assert!(
        mb_per_s >= FLOOR_MB_PER_S,
        "parser throughput {mb_per_s:.1} MB/s is below the {FLOOR_MB_PER_S} MB/s floor"
    );
}
