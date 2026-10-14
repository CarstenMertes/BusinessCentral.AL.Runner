//! End-to-end: the CLI against the real, pinned corpus and the real oracle.
//! These need the `corpus` submodule; a missing checkout fails loudly rather
//! than skipping, because a skipped harness test reads as a green one.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

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

/// A test codeunit with a handful of tests that does not need the Microsoft apps. Runs that only
/// check how a flag is handled select it: a run costs a fixed compile whatever it selects, but
/// only the selected cases are run.
const SMALL_CU: &str = "60184";

/// A test codeunit of the OnPrem app (`corpus/tests/al-language-onprem`), whose cases need the
/// Microsoft apps and are refused in these `--no-deps` runs.
const ONPREM_CU: &str = "61200";

/// The one run over every oracle version that most tests below read their numbers from, so
/// that the corpus is loaded, compiled and run three times in total and not once per assertion.
/// It is `--all-versions` with the three reports written, exactly the CI corpus step minus the
/// Microsoft apps.
struct AllVersions {
    dir: PathBuf,
    code: i32,
    out: String,
    err: String,
}

fn all_versions() -> &'static AllVersions {
    static RUN: OnceLock<AllVersions> = OnceLock::new();
    RUN.get_or_init(|| {
        require_corpus();
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join("corpus_harness_all_versions");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = |f: &str| dir.join(f).to_str().unwrap().to_string();
        let (json, md, junit) = (path("pass-rate.json"), path("pass-rate.md"), path("runtime-junit.xml"));
        let (code, out, err) = cli(&["--all-versions", "--json", &json, "--markdown", &md, "--junit", &junit]);
        AllVersions { dir, code, out, err }
    })
}

impl AllVersions {
    fn file(&self, stem: &str, major: &str, ext: &str) -> PathBuf {
        self.dir.join(format!("{stem}-{major}.{ext}"))
    }

    /// The report of one version (`27`, `28` or `29`).
    fn json(&self, major: &str) -> serde_json::Value {
        assert_eq!(self.code, 0, "BC {major}: {}", self.err);
        let f = self.file("pass-rate", major, "json");
        let text =
            std::fs::read_to_string(&f).unwrap_or_else(|_| panic!("no report for BC {major}; stdout:\n{}", self.out));
        serde_json::from_str(&text).unwrap()
    }
}

#[test]
fn every_oracle_case_is_reported_and_attributed_to_an_area() {
    // BC 29: the version a run without `--bc-version` picks.
    let r = all_versions().json("29");
    assert_eq!(r["overall"]["total"], 5147);
    assert_eq!(r["unmapped"].as_array().unwrap().len(), 0, "{}", r["unmapped"]);
    assert_eq!(r["areas"]["json"]["total"], 129);
    assert_eq!(r["areas"]["record"]["total"], 1076);
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
            "6219bf4b41cb9802e1d7fe20efc252778f2e02ef",
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

/// The real oracle with an older corpus commit recorded for BC 29 (every table is on the pin).
fn oracle_with_29_recorded_for(commit: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let t = std::fs::read_to_string(root().join("oracle/oracle.toml")).unwrap();
    let marker = "bc_version = \"29.0\"\n";
    assert!(t.contains(marker), "BC 29 has a table");
    assert!(
        !t.contains("workflow_run = 38306086475"),
        "BC 29 is recorded for the pin"
    );
    let bad = dir.path().join("oracle.toml");
    std::fs::write(
        &bad,
        t.replace(marker, &format!("{marker}corpus_commit = \"{commit}\"\n")),
    )
    .unwrap();
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
    // The ratchet checks the cases of the selection, so selecting the codeunit is enough.
    let (code, _, err) = cli(&["--baseline", base.to_str().unwrap(), "--codeunit", "60002"]);
    assert_eq!(code, 1, "{err}");
    assert!(err.contains("60002 NoSuchProcedure_UsedByTheRegressionTest"), "{err}");
}

#[test]
fn a_failing_case_outside_the_baseline_is_reported_but_does_not_fail_the_run() {
    require_corpus();
    // ADR-007: only a baseline case that stops passing exits 1. The probe tests the corpus adds
    // for behaviour the runtime does not have yet are failures in the report, not in the gate.
    // The run over every version says which codeunit holds a failing case; the ratchet run
    // below selects that codeunit.
    let shared = all_versions();
    assert!(shared.json("27")["overall"]["fail"].as_u64().unwrap() > 0);
    let junit = std::fs::read_to_string(shared.file("runtime-junit", "27", "xml")).unwrap();
    let failing = junit
        .split("<testcase classname=\"Codeunit ")
        .skip(1)
        .find(|c| c.split("</testcase>").next().unwrap().contains("<failure"))
        .expect("BC 27 has a failing case");
    let codeunit = failing.split('"').next().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("passing.txt");
    std::fs::write(&base, "").unwrap();
    let (code, r) = json(&[
        "--bc-version",
        "27",
        "--baseline",
        base.to_str().unwrap(),
        "--codeunit",
        codeunit,
    ]);
    assert_eq!(code, 0);
    assert!(r["overall"]["fail"].as_u64().unwrap() > 0, "{}", r["overall"]);
}

#[test]
fn markdown_summary_is_written() {
    require_corpus();
    // The whole-corpus summary of BC 29 (the default version) ...
    let shared = all_versions();
    assert_eq!(shared.code, 0, "{}", shared.err);
    let md = std::fs::read_to_string(shared.file("pass-rate", "29", "md")).unwrap();
    assert!(md.contains("| **overall** | 5147 |"), "{md}");
    // ... and `--markdown` of a single-version run writes the file it is given.
    let dir = tempfile::tempdir().unwrap();
    let one = dir.path().join("s.md");
    let (code, _, err) = cli(&["--markdown", one.to_str().unwrap(), "--codeunit", SMALL_CU]);
    assert_eq!(code, 0, "{err}");
    assert!(std::fs::read_to_string(one).unwrap().contains("| **overall** |"));
}

#[test]
fn the_first_line_names_the_target() {
    require_corpus();
    // A run over every version starts with the oldest one's target and names the others.
    let shared = all_versions();
    assert_eq!(shared.code, 0, "{}", shared.err);
    assert_eq!(
        shared.out.lines().next(),
        Some("target: BC 27.5, runtime 16.0"),
        "{}",
        shared.out
    );
    for line in ["target: BC 28.5, runtime 16.0", "target: BC 29.0, runtime 16.0"] {
        assert!(shared.out.lines().any(|l| l == line), "{line} in {}", shared.out);
    }
    // `--bc-version` picks one. A selection puts its own line first, so the target line follows it.
    let target_line = |v: &str| {
        let (code, out, err) = cli(&["--bc-version", v, "--codeunit", SMALL_CU]);
        assert_eq!(code, 0, "{err}");
        assert_eq!(
            out.lines().next().map(|l| l.starts_with("selection: ")),
            Some(true),
            "{out}"
        );
        out.lines().nth(1).unwrap_or_default().to_string()
    };
    assert_eq!(target_line("27"), "target: BC 27.5, runtime 16.0");
    assert_eq!(target_line("28"), "target: BC 28.5, runtime 16.0");
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
    // All three are recorded for the pinned corpus commit.
    for (v, total) in [("27", 5147), ("28", 5147), ("29", 5147)] {
        let r = all_versions().json(v);
        assert_eq!(r["overall"]["total"], total, "BC {v}");
        assert_eq!(r["unmapped"].as_array().unwrap().len(), 0, "BC {v}");
    }
}

#[test]
fn the_default_is_the_newest_version_with_an_oracle() {
    require_corpus();
    let (code, out, err) = cli(&["--codeunit", SMALL_CU]);
    assert_eq!(code, 0, "{err}");
    // The selection's own line comes first.
    assert_eq!(out.lines().nth(1), Some("target: BC 29.0, runtime 16.0"), "{out}");
}

#[test]
fn all_versions_runs_each_oracle_writes_per_version_files_and_compares() {
    let AllVersions { dir, code, out, err } = all_versions();
    assert_eq!(*code, 0, "{err}");
    for v in ["27", "28", "29"] {
        let f = dir.join(format!("pass-rate-{v}.json"));
        assert!(f.exists(), "no report for BC {v}; stdout:\n{out}");
    }
    assert!(
        !dir.join("pass-rate.json").exists(),
        "all-versions writes only per-version files"
    );
    assert!(out.contains("Version comparison"), "{out}");
    // Every oracle is recorded for the pin and has the same cases.
    assert!(out.contains("oracle: same cases in every version"), "{out}");
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
    // Without the flag the same machine still runs the corpus (a codeunit of it: the flag decides
    // whether the run starts, not what it selects).
    let (code, _, err) = cli(&[
        "--dotnet-sidecar",
        "/nonexistent/AlDotNetSidecar.dll",
        "--codeunit",
        SMALL_CU,
    ]);
    assert_eq!(code, 0, "{err}");
}

// BC 28 holds the main app and three more oracles (oracle/oracle.toml `[[version.suite]]`): the
// OnPrem app and the isolation probe under its two runners. Their cases stay out of `overall`.
#[test]
fn the_other_test_apps_are_reported_apart_from_the_main_one() {
    let r = all_versions().json("28");
    assert_eq!(r["overall"]["total"], 5147);
    let suites = r["suites"].as_array().unwrap();
    let get = |n: &str| {
        suites
            .iter()
            .find(|s| s["name"] == n)
            .unwrap_or_else(|| panic!("no suite {n}"))
    };
    assert_eq!(get("onprem")["counts"]["total"], 64);
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
    let r = all_versions().json("27");
    let names: Vec<_> = r["suites"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, vec!["onprem"]);
}

// `--baseline FILE` is the main app's ratchet alone. The extra apps keep their own files in
// `oracle.toml`, which may need the dependency apps this `--no-deps` run does not load.
#[test]
fn an_explicit_baseline_does_not_ratchet_the_extra_apps() {
    require_corpus();
    // Not a regression: onprem's own baseline lists cases that need the dependency apps.
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("passing.txt");
    std::fs::write(&base, "").unwrap();
    // Codeunit 61200 is the OnPrem app's: its cases are refused without the dependency apps.
    let (code, _, err) = cli(&[
        "--bc-version",
        "27",
        "--baseline",
        base.to_str().unwrap(),
        "--codeunit",
        ONPREM_CU,
    ]);
    assert_eq!(code, 0, "{err}");
}

#[test]
fn a_suite_baseline_ratchets_that_extra_app_and_no_ratchet_ignores_it() {
    require_corpus();
    let dir = tempfile::tempdir().unwrap();
    let base = dir.path().join("passing.txt");
    std::fs::write(&base, "").unwrap();
    // A case the OnPrem app cannot pass without its platform tables and dependency apps.
    let suite = dir.path().join("onprem.txt");
    std::fs::write(
        &suite,
        "61200 ObjectMetadata_Find_ApplicationDatabaseTableId_ReturnsARow\n",
    )
    .unwrap();
    let args = |extra: &[&str]| {
        let mut a = vec![
            "--bc-version",
            "27",
            "--baseline",
            base.to_str().unwrap(),
            "--codeunit",
            ONPREM_CU,
        ];
        a.extend_from_slice(&["--suite-baseline"]);
        let named = format!("onprem={}", suite.display());
        let mut v: Vec<String> = a.iter().map(|s| s.to_string()).collect();
        v.push(named);
        v.extend(extra.iter().map(|s| s.to_string()));
        v
    };
    let run = |a: Vec<String>| cli(&a.iter().map(String::as_str).collect::<Vec<_>>());
    let (code, _, err) = run(args(&[]));
    assert_eq!(code, 1, "{err}");
    assert!(
        err.contains("27.5 onprem") && err.contains("ObjectMetadata_Find_ApplicationDatabaseTableId"),
        "{err}"
    );
    // --no-ratchet reports only, for every app.
    let mut a = args(&["--no-ratchet"]);
    a.retain(|x| x != base.to_str().unwrap() && x != "--baseline");
    let (code, _, err) = run(a);
    assert_eq!(code, 0, "{err}");
    let (_, _, err) = cli(&["--bc-version", "27", "--suite-baseline", "nope=x"]);
    assert!(err.contains("no suite named `nope`"), "{err}");
}
