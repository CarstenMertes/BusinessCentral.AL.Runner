//! The lexer over every `.al` file in the corpus. BC compiles all of them, so
//! the lexer must cover each byte exactly once and produce no `Error` token.

use al_syntax::{SyntaxKind, lex};
use std::path::{Path, PathBuf};

fn corpus_files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../corpus/tests");
    assert!(
        root.join("al-language/app.json").exists(),
        "corpus submodule not checked out: run `git submodule update --init`"
    );
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "al") {
                out.push(p);
            }
        }
    }
    out.sort();
    assert!(out.len() > 1300, "expected the whole corpus, found {} files", out.len());
    out
}

#[test]
fn every_corpus_file_round_trips_and_lexes_without_errors() {
    let mut failures = Vec::new();
    for f in corpus_files() {
        let src = std::fs::read_to_string(&f).unwrap();
        let mut pos = 0usize;
        let mut first_error = None;
        for t in lex(&src) {
            let end = pos + t.len as usize;
            if t.kind == SyntaxKind::Error && first_error.is_none() {
                let line = src[..pos].matches('\n').count() + 1;
                first_error = Some(format!("line {line}: {:?}", &src[pos..end.min(pos + 40)]));
            }
            pos = end;
        }
        if pos != src.len() {
            failures.push(format!("{}: covered {pos} of {} bytes", f.display(), src.len()));
        }
        if let Some(e) = first_error {
            failures.push(format!("{}: {e}", f.display()));
        }
    }
    assert!(
        failures.is_empty(),
        "{} file(s) failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}
