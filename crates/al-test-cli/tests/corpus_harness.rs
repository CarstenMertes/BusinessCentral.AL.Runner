//! End-to-end: the CLI against the real, pinned corpus and the real oracle.
//! These need the `corpus` submodule; a missing checkout fails loudly rather
//! than skipping, because a skipped harness test reads as a green one.

use std::path::{Path, PathBuf};
use std::process::Command;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn require_corpus() {
    assert!(
        root().join("corpus/tests/al-language/app.json").exists(),
        "corpus submodule not checked out: run `git submodule update --init`"
    );
}

/// Runs the CLI over the corpus on its own: without the Microsoft apps, which would make
/// each run in this unoptimised test build take minutes (the release CI step runs with
/// them). A baseline of its own, because the real one lists cases that need those apps.
fn cli(args: &[&str]) -> (i32, String, String) {
    let dir = tempfile::tempdir().unwrap();
    let empty = dir.path().join("empty-baseline.txt");
    std::fs::write(&empty, "").unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_al-test-cli"));
    cmd.arg("--root").arg(root()).arg("--no-deps");
    if !args.contains(&"--baseline") {
        cmd.arg("--baseline").arg(&empty);
    }
    let out = cmd.args(args).output().unwrap();
    (
        out.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&out.stdout).into(),
        String::from_utf8_lossy(&out.stderr).into(),
    )
}

fn json(args: &[&str]) -> (i32, serde_json::Value) {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("r.json");
    let mut a = vec!["--json", f.to_str().unwrap()];
    a.extend_from_slice(args);
    let (code, _, err) = cli(&a);
    let v = std::fs::read_to_string(&f)
        .map(|s| serde_json::from_str(&s).unwrap())
        .unwrap_or_else(|_| panic!("no report written; stderr:\n{err}"));
    (code, v)
}

#[test]
fn every_oracle_case_is_reported_and_attributed_to_an_area() {
    require_corpus();
    let (code, r) = json(&[]);
    assert_eq!(code, 0);
    assert_eq!(r["overall"]["total"], 4836);
    assert_eq!(r["unmapped"].as_array().unwrap().len(), 0, "{}", r["unmapped"]);
    assert_eq!(r["areas"]["json"]["total"], 117);
    assert_eq!(r["areas"]["record"]["total"], 1018);
}

#[test]
fn oracle_pinned_to_a_different_corpus_commit_is_refused() {
    require_corpus();
    let dir = tempfile::tempdir().unwrap();
    let t = std::fs::read_to_string(root().join("oracle/oracle.toml")).unwrap();
    let bad = dir.path().join("oracle.toml");
    std::fs::write(
        &bad,
        t.replace(
            "cf6e546793a92c482499ce86eadf0a0b13d2a742",
            "0000000000000000000000000000000000000000",
        ),
    )
    .unwrap();
    std::fs::copy(root().join("oracle/junit-27.5.xml"), dir.path().join("junit-27.5.xml")).unwrap();
    let (code, _, err) = cli(&["--oracle", bad.to_str().unwrap()]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("corpus pin"), "{err}");
}

#[test]
fn a_baseline_case_that_does_not_pass_fails_the_run() {
    require_corpus();
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("passing.txt");
    std::fs::write(&base, "60002 NoSuchProcedure_UsedByTheRegressionTest\n").unwrap();
    let (code, _, err) = cli(&["--baseline", base.to_str().unwrap()]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("60002 NoSuchProcedure_UsedByTheRegressionTest"), "{err}");
}

#[test]
fn markdown_summary_is_written() {
    require_corpus();
    let dir = tempfile::tempdir().unwrap();
    let md = dir.path().join("s.md");
    let (code, _, _) = cli(&["--markdown", md.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(std::fs::read_to_string(md).unwrap().contains("| **overall** | 4836 |"));
}
