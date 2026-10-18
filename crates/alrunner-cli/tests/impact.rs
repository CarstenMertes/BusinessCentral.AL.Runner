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
            "Impact: 5 tests: 1 newly failing, 0 fails differently, 0 newly refused, 0 newly passing, 0 newly runs, 1 same (reached), 3 same (not reached), 0 not verified"
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
            "Impact: 5 tests: 0 newly failing, 0 fails differently, 0 newly refused, 0 newly passing, 0 newly runs, 0 same (reached), 5 same (not reached), 0 not verified"
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
            "Impact: 4 tests: 0 newly failing, 0 fails differently, 0 newly refused, 0 newly passing, 0 newly runs, 0 same (reached), 4 same (not reached), 0 not verified"
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
fn an_impact_run_needs_impact_all_and_the_compile_cache() {
    let (tests, ext, lib) = (fixture("tests"), fixture("ext"), fixture("lib"));
    // No index and no static reach yet (ADR-043 steps 2 and 5): without `--impact all` the run stops and says
    // what to pass.
    let o = alrunner(&["test", &tests, "--impact-of", &ext, "--dep", &lib]);
    assert_eq!(code(&o), 2, "{}", text(&o));
    assert!(text(&o).contains("--impact all"), "{}", text(&o));
    let o = alrunner(&[
        "test",
        &tests,
        "--impact-of",
        &ext,
        "--impact",
        "dynamic",
        "--dep",
        &lib,
    ]);
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
}
