//! `alrunner test --impact-of` (ADR-043 step 1: two runs and a diff) end to end on the small fixture apps in
//! `tests/fixtures/impact`. Nothing here is Microsoft's code: `lib` stands in for the Base App (a publisher),
//! `tests` for a Microsoft test app, `ext` for a partner extension (a subscriber), `unrelated` for an app that
//! touches nothing the tests reach.

use std::path::PathBuf;
use std::process::{Command, Output};

fn fixture(name: &str) -> String {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/impact")
        .join(name)
        .to_string_lossy()
        .into_owned()
}

fn alrunner(args: &[&str]) -> Output {
    let cache = tempfile::tempdir().unwrap();
    Command::new(env!("CARGO_BIN_EXE_alrunner"))
        .env("AL_CHECK_THREADS", "2")
        .env("AL_LOWER_THREADS", "2")
        .env("AL_TEST_THREADS", "2")
        .args(args)
        .args(["--no-download", "--cache-dir", cache.path().to_str().unwrap()])
        .output()
        .expect("run alrunner")
}

fn text(o: &Output) -> String {
    format!(
        "{}{}",
        String::from_utf8_lossy(&o.stdout),
        String::from_utf8_lossy(&o.stderr)
    )
}

fn code(o: &Output) -> i32 {
    o.status.code().expect("exit code")
}

/// The lines of the report from the section headed `head` up to the next blank line.
fn section(t: &str, head: &str) -> String {
    let Some(start) = t.find(&format!("\n{head} (")) else {
        return String::new();
    };
    t[start + 1..].split("\n\n").next().unwrap_or_default().to_string()
}

#[test]
fn an_impact_run_classifies_each_test_and_names_the_extension_routines_that_ran() {
    let (tests, ext, lib) = (fixture("tests"), fixture("ext"), fixture("lib"));
    let o = alrunner(&["test", &tests, "--impact-of", &ext, "--impact", "all", "--dep", &lib]);
    let t = text(&o);
    // A test that passed without the extension and fails with it is a breaking change: exit 1.
    assert_eq!(code(&o), 1, "{t}");
    assert!(
        t.contains(
            "Impact: 5 tests: 1 newly failing, 0 fails differently, 0 newly refused, 0 selection miss, 0 newly passing, 0 newly runs, 1 same (reached), 3 same (not reached), 0 not verified"
        ),
        "{t}"
    );
    let failing = section(&t, "newly failing");
    assert!(
        failing.contains("50510 Imp Tests.PostKeepsALargeAmount: pass -> fail: amount 20"),
        "{t}"
    );
    assert!(
        failing.contains("ran: Imp Ext Subscriber.DoubleLargeAmounts (1 call)"),
        "{t}"
    );
    let reached = section(&t, "same, reached");
    assert!(reached.contains("50510 Imp Tests.PostKeepsASmallAmount: pass"), "{t}");
    assert!(
        reached.contains("ran: Imp Ext Subscriber.DoubleLargeAmounts (2 calls)"),
        "{t}"
    );
    // Unchanged tests that ran no extension code are counted, not listed.
    assert!(!t.contains("Imp Tests.PostsNothing"), "{t}");
    // A failure on both sides with the same text is unchanged.
    assert!(!t.contains("fails differently ("), "{t}");
    // The extension's own test codeunits are not part of the run.
    assert!(
        !t.contains("OwnTest") && !t.contains("the extension's own test ran"),
        "{t}"
    );
}

#[test]
fn outcomes_do_not_depend_on_the_selection_or_on_an_unrelated_app_in_the_compile() {
    // ADR-043 section 2.2, condition 1: the same codeunit alone and in a full run, with and without an
    // unrelated app in the compile, gives the same outcomes.
    let (tests, unrelated, lib) = (fixture("tests"), fixture("unrelated"), fixture("lib"));
    let all = alrunner(&[
        "test",
        &tests,
        "--impact-of",
        &unrelated,
        "--impact",
        "all",
        "--dep",
        &lib,
    ]);
    let t = text(&all);
    assert_eq!(code(&all), 0, "{t}");
    assert!(
        t.contains(
            "Impact: 5 tests: 0 newly failing, 0 fails differently, 0 newly refused, 0 selection miss, 0 newly passing, 0 newly runs, 0 same (reached), 5 same (not reached), 0 not verified"
        ),
        "{t}"
    );
    let one = alrunner(&[
        "test",
        &tests,
        "--impact-of",
        &unrelated,
        "--impact",
        "all",
        "--dep",
        &lib,
        "--codeunit",
        "50510",
    ]);
    let t = text(&one);
    assert_eq!(code(&one), 0, "{t}");
    assert!(
        t.contains(
            "Impact: 4 tests: 0 newly failing, 0 fails differently, 0 newly refused, 0 selection miss, 0 newly passing, 0 newly runs, 0 same (reached), 4 same (not reached), 0 not verified"
        ),
        "{t}"
    );
    // The outcomes of codeunit 50510 alone equal its outcomes in the full run (a plain run, no impact).
    let full = text(&alrunner(&["test", &tests, "--dep", &lib, "--verbose"]));
    let alone = text(&alrunner(&[
        "test",
        &tests,
        "--dep",
        &lib,
        "--verbose",
        "--codeunit",
        "50510",
    ]));
    let of_50510 = |t: &str| -> Vec<String> {
        let mut v: Vec<String> = t
            .lines()
            .filter(|l| l.contains(" 50510 Imp Tests."))
            .map(|l| {
                // Drop the time column: `PASS    12 ms 50510 ...`.
                let (kind, rest) = l.split_once(' ').unwrap_or((l, ""));
                format!("{kind} {}", &rest[rest.find("50510").unwrap_or(0)..])
            })
            .collect();
        v.sort();
        v
    };
    assert_eq!(of_50510(&full).len(), 4, "{full}");
    assert_eq!(of_50510(&full), of_50510(&alone), "{full}\n{alone}");
}

#[test]
fn an_impact_run_without_an_index_runs_both_sides_and_says_how_to_build_the_index() {
    let (tests, ext, lib) = (fixture("tests"), fixture("ext"), fixture("lib"));
    // No index cached: every selected test runs without and with the extension (ADR-043 owner decision 1, until
    // static selection exists), with a warning before the run and again above the summary.
    let o = alrunner(&["test", &tests, "--impact-of", &ext, "--dep", &lib]);
    let t = text(&o);
    assert_eq!(code(&o), 1, "{t}");
    assert_eq!(t.matches("warning: no impact index").count(), 2, "{t}");
    assert!(t.contains("alrunner impact-index build"), "{t}");
    assert!(t.contains("1 newly failing"), "{t}");
    // Static selection is ADR-043 step 5.
    let o = alrunner(&["test", &tests, "--impact-of", &ext, "--impact", "static", "--dep", &lib]);
    assert_eq!(code(&o), 2, "{}", text(&o));
    assert!(text(&o).contains("not built yet"), "{}", text(&o));
    // `--impact` without `--impact-of` has nothing to compare.
    let o = alrunner(&["test", &tests, "--impact", "all", "--dep", &lib]);
    assert_eq!(code(&o), 2, "{}", text(&o));
    // Both sides must compile the dependencies the same way: from the compile cache, never on demand.
    let o = alrunner(&[
        "test",
        &tests,
        "--impact-of",
        &ext,
        "--impact",
        "all",
        "--dep",
        &lib,
        "--no-compile-cache",
    ]);
    assert_eq!(code(&o), 2, "{}", text(&o));
    assert!(text(&o).contains("--no-compile-cache"), "{}", text(&o));
    // An index is built without an extension.
    let o = alrunner(&["impact-index", "build", &tests, "--impact-of", &ext, "--dep", &lib]);
    assert_eq!(code(&o), 2, "{}", text(&o));
}

fn alrunner_in(cache: &std::path::Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_alrunner"))
        .env("AL_CHECK_THREADS", "2")
        .env("AL_LOWER_THREADS", "2")
        .env("AL_TEST_THREADS", "2")
        .args(args)
        .args(["--no-download", "--cache-dir", cache.to_str().unwrap()])
        .output()
        .expect("run alrunner")
}

#[test]
fn with_an_index_the_run_selects_the_codeunits_that_reach_the_extension_and_runs_them_once() {
    let (tests, ext, lib, unrelated) = (fixture("tests"), fixture("ext"), fixture("lib"), fixture("unrelated"));
    let cache = tempfile::tempdir().unwrap();
    let o = alrunner_in(cache.path(), &["impact-index", "build", &tests, "--dep", &lib]);
    let t = text(&o);
    assert_eq!(code(&o), 0, "{t}");
    assert!(t.contains("impact index: 5 tests in 2 codeunits"), "{t}");

    // The default with an index is dynamic selection: only codeunit 50510 calls the publisher the extension
    // subscribes to. The "without" side comes from the index.
    let o = alrunner_in(cache.path(), &["test", &tests, "--impact-of", &ext, "--dep", &lib]);
    let t = text(&o);
    assert_eq!(code(&o), 1, "{t}");
    assert!(!t.contains("warning: no impact index"), "{t}");
    assert!(t.contains("selected 1 of 2 test codeunits"), "{t}");
    assert!(!t.contains("\nwithout the extension:"), "{t}");
    // The same classes as `--impact all` for the codeunit that ran, and the rest counted as not selected.
    assert!(
        t.contains(
            "Impact: 4 tests: 1 newly failing, 0 fails differently, 0 newly refused, 0 selection miss, 0 newly passing, 0 newly runs, 1 same (reached), 2 same (not reached), 0 not verified"
        ),
        "{t}"
    );
    assert!(t.contains("not selected: 1 test in 1 codeunit"), "{t}");
    let failing = section(&t, "newly failing");
    assert!(
        failing.contains("50510 Imp Tests.PostKeepsALargeAmount: pass -> fail: amount 20"),
        "{t}"
    );
    assert!(failing.contains("reached: subscriber"), "{t}");

    // An extension that reaches nothing the tests reach selects nothing and runs nothing.
    let o = alrunner_in(
        cache.path(),
        &["test", &tests, "--impact-of", &unrelated, "--dep", &lib],
    );
    let t = text(&o);
    assert_eq!(code(&o), 0, "{t}");
    assert!(t.contains("selected 0 of 2 test codeunits"), "{t}");

    // `--impact all` still runs both sides, index or not.
    let o = alrunner_in(
        cache.path(),
        &["test", &tests, "--impact-of", &ext, "--impact", "all", "--dep", &lib],
    );
    let t = text(&o);
    assert_eq!(code(&o), 1, "{t}");
    assert!(t.contains("\nwithout the extension:"), "{t}");
}
