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
/// them). No ratchet unless the test brings a baseline: the real one lists cases that need
/// those apps.
fn cli(args: &[&str]) -> (i32, String, String) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_al-test-cli"));
    cmd.arg("--root").arg(root()).arg("--no-deps");
    if !args.contains(&"--baseline") {
        cmd.arg("--no-ratchet");
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
            "cdf315abd1e7115fcbfbcc88c720371e75989b8e",
            "0000000000000000000000000000000000000000",
        ),
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("27")).unwrap();
    std::fs::copy(root().join("oracle/27/junit.xml"), dir.path().join("27/junit.xml")).unwrap();
    let (code, _, err) = cli(&["--oracle", bad.to_str().unwrap()]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("corpus pin"), "{err}");
}

/// The real oracle with BC 29's recorded corpus commit replaced.
fn oracle_with_29_recorded_for(commit: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let t = std::fs::read_to_string(root().join("oracle/oracle.toml")).unwrap();
    assert!(
        t.contains("corpus_commit = \"cf6e546"),
        "BC 29's oracle is recorded for the older commit"
    );
    let bad = dir.path().join("oracle.toml");
    std::fs::write(&bad, t.replace("cf6e546793a92c482499ce86eadf0a0b13d2a742", commit)).unwrap();
    for v in ["27", "28", "29"] {
        std::fs::create_dir(dir.path().join(v)).unwrap();
        std::fs::copy(
            root().join(format!("oracle/{v}/junit.xml")),
            dir.path().join(format!("{v}/junit.xml")),
        )
        .unwrap();
    }
    (dir, bad)
}

#[test]
fn an_oracle_recorded_for_a_corpus_commit_that_changed_existing_tests_is_refused() {
    require_corpus();
    // 9df7c23 .. pin changes tests/al-language/record/TestAutoIncrementExplicitKey.al (ADR-007).
    let (_dir, bad) = oracle_with_29_recorded_for("9df7c23c91095f6747ace1cefaebec7dd882410a");
    let (code, _, err) = cli(&["--bc-version", "29", "--oracle", bad.to_str().unwrap()]);
    assert_eq!(code, 2, "{err}");
    assert!(
        err.contains("TestAutoIncrementExplicitKey.al") && err.contains("29"),
        "{err}"
    );
}

#[test]
fn an_oracle_recorded_for_a_commit_missing_from_the_checkout_is_a_setup_error() {
    require_corpus();
    let (_dir, bad) = oracle_with_29_recorded_for("0123456789012345678901234567890123456789");
    let (code, _, err) = cli(&["--bc-version", "29", "--oracle", bad.to_str().unwrap()]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("fetch"), "{err}");
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
fn a_failing_case_outside_the_baseline_is_reported_but_does_not_fail_the_run() {
    require_corpus();
    // ADR-007: only a baseline case that stops passing exits 1. The probe tests the corpus adds
    // for behaviour the runtime does not have yet are failures in the report, not in the gate.
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("passing.txt");
    std::fs::write(&base, "").unwrap();
    let (code, r) = json(&["--bc-version", "27", "--baseline", base.to_str().unwrap()]);
    assert_eq!(code, 0);
    assert!(r["overall"]["fail"].as_u64().unwrap() > 0, "{}", r["overall"]);
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

#[test]
fn the_first_line_names_the_target() {
    require_corpus();
    let (code, out, err) = cli(&["--bc-version", "27"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.lines().next(), Some("target: BC 27.5, runtime 16.0"), "{out}");
    let (code, out, err) = cli(&["--bc-version", "28"]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.lines().next(), Some("target: BC 28.5, runtime 16.0"), "{out}");
}

#[test]
fn an_unknown_bc_version_exits_2_and_lists_the_known_ones() {
    require_corpus();
    let (code, _, err) = cli(&["--bc-version", "99"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("no oracle for BC 99") && err.contains("27.5"), "{err}");
}

#[test]
fn bc_26_is_not_supported() {
    require_corpus();
    let (code, _, err) = cli(&["--bc-version", "26"]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("27 and later only"), "{err}");
}

#[test]
fn every_bc_major_has_an_oracle_with_the_same_cases() {
    require_corpus();
    // 28 and 29 come from the hybrid runner: its OnRun pseudo-cases must be skipped, or the
    // run stops with exit 2 (ADR-015 1.6).
    // BC 27 and 28 are recorded for the pinned corpus commit (223 probe tests more). BC 29 is
    // recorded for the older commit, which the pin only adds files to (ADR-007), so its cases are
    // the 4836 it had.
    for (v, total) in [("27", 5059), ("28", 5059), ("29", 4836)] {
        let (code, r) = json(&["--bc-version", v]);
        assert_eq!(code, 0, "BC {v}");
        assert_eq!(r["overall"]["total"], total, "BC {v}");
        assert_eq!(r["unmapped"].as_array().unwrap().len(), 0, "BC {v}");
    }
}

#[test]
fn the_default_is_the_newest_version_with_an_oracle() {
    require_corpus();
    let (code, out, err) = cli(&[]);
    assert_eq!(code, 0, "{err}");
    assert_eq!(out.lines().next(), Some("target: BC 29.0, runtime 16.0"), "{out}");
}

#[test]
fn all_versions_runs_each_oracle_writes_per_version_files_and_compares() {
    require_corpus();
    let dir = tempfile::tempdir().unwrap();
    let json = dir.path().join("pass-rate.json");
    let (code, out, err) = cli(&["--all-versions", "--json", json.to_str().unwrap()]);
    assert_eq!(code, 0, "{err}");
    for v in ["27", "28", "29"] {
        let f = dir.path().join(format!("pass-rate-{v}.json"));
        assert!(f.exists(), "no report for BC {v}; stdout:\n{out}");
    }
    assert!(!json.exists(), "all-versions writes only per-version files");
    assert!(out.contains("Version comparison"), "{out}");
    // BC 29's oracle lacks the 223 cases that the pinned corpus added. Re-recording it for the
    // pin makes this line say "same cases in every version" again.
    assert!(out.contains("oracle: 223 case(s) differ"), "{out}");
    // The three known BC 29 differences are gated (ADR-015 3.4), so the runtime passes the same
    // cases on every version, among the cases every oracle has. A new difference would be
    // listed here.
    assert!(out.contains("runtime: same cases in every version"), "{out}");
}

#[test]
fn all_versions_cannot_be_combined_with_one_version() {
    require_corpus();
    let (code, _, err) = cli(&["--all-versions", "--bc-version", "28"]);
    assert_eq!(code, 2, "{err}");
}

#[test]
fn require_dotnet_turns_an_unusable_dotnet_into_a_setup_error() {
    require_corpus();
    // ADR-014 section 11: CI passes --require-dotnet so that "no .NET" is never mistaken for a pass.
    let (code, _, err) = cli(&[
        "--require-dotnet",
        "--dotnet-sidecar",
        "/nonexistent/AlDotNetSidecar.dll",
    ]);
    assert_eq!(code, 2, "{err}");
    assert!(err.contains("--require-dotnet") && err.contains("DotNet"), "{err}");
    // Without the flag the same machine still runs the corpus.
    let (code, _, err) = cli(&["--dotnet-sidecar", "/nonexistent/AlDotNetSidecar.dll"]);
    assert_eq!(code, 0, "{err}");
}

// BC 28 holds the main app and three more oracles (oracle/oracle.toml `[[version.suite]]`): the
// OnPrem app and the isolation probe under its two runners. Their cases stay out of `overall`.
#[test]
fn the_other_test_apps_are_reported_apart_from_the_main_one() {
    require_corpus();
    let (code, r) = json(&["--bc-version", "28"]);
    assert_eq!(code, 0);
    assert_eq!(r["overall"]["total"], 5059);
    let suites = r["suites"].as_array().unwrap();
    let get = |n: &str| {
        suites
            .iter()
            .find(|s| s["name"] == n)
            .unwrap_or_else(|| panic!("no suite {n}"))
    };
    assert_eq!(get("onprem")["counts"]["total"], 61);
    assert_eq!(get("isolation-function")["counts"]["total"], 7);
    assert_eq!(get("isolation-function")["counts"]["pass"], 7);
    // The control: real BC fails T5 under Codeunit isolation, and the runtime fails it too.
    let control = get("isolation-codeunit");
    assert_eq!(control["counts"]["pass"], 6);
    assert_eq!(control["bc_failed"], 1);
    assert_eq!(control["bc_failed_reproduced"], 1);
}

// BC 27 and 29 have no isolation oracle (the probe only ran in the BC 28.4 nightly).
#[test]
fn a_version_without_an_isolation_oracle_lists_only_the_onprem_app() {
    require_corpus();
    let (_, r) = json(&["--bc-version", "27"]);
    let names: Vec<_> = r["suites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["onprem"]);
}
