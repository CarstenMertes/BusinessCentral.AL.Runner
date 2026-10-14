//! `alrunner test` end to end on small fixture apps. Nothing here is Microsoft's code.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn alrunner(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_alrunner"))
        // Few threads: the machine is shared, and the fixtures are tiny.
        .env("AL_CHECK_THREADS", "2")
        .env("AL_LOWER_THREADS", "2")
        .env("AL_TEST_THREADS", "2")
        .args(args)
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

fn write(dir: &Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn app_json(id: &str, name: &str, deps: &[(&str, &str)]) -> String {
    let deps: Vec<String> = deps
        .iter()
        .map(|(id, name)| format!(r#"{{"id":"{id}","name":"{name}","publisher":"Test","version":"1.0.0.0"}}"#))
        .collect();
    format!(
        r#"{{"id":"{id}","name":"{name}","publisher":"Test","version":"1.0.0.0","dependencies":[{}]}}"#,
        deps.join(",")
    )
}

const MIXED: &str = r#"
codeunit 50100 "Mixed Tests"
{
    Subtype = Test;

    [Test]
    procedure Passes()
    begin
    end;

    [Test]
    procedure Fails()
    begin
        Error('boom %1', 5);
    end;

    [Test]
    procedure Refused()
    begin
        Codeunit.Run(Codeunit::"Not There");
    end;
}

codeunit 50101 "Other Tests"
{
    Subtype = Test;

    [Test]
    procedure AlsoPasses()
    begin
    end;
}
"#;

fn mixed_app(root: &Path) -> PathBuf {
    let dir = root.join("mixed");
    write(
        &dir,
        "app.json",
        &app_json("11111111-0000-0000-0000-000000000001", "Mixed", &[]),
    );
    write(&dir, "src/Tests.al", MIXED);
    dir
}

/// A stored (uncompressed) ZIP with CRC-32s, enough for `al-deps`.
fn zip(entries: &[(&str, &str)]) -> Vec<u8> {
    fn crc32(data: &[u8]) -> u32 {
        let mut c = !0u32;
        for &b in data {
            c ^= u32::from(b);
            for _ in 0..8 {
                c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
            }
        }
        !c
    }
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in entries {
        let data = data.as_bytes();
        let crc = crc32(data);
        let offset = out.len() as u32;
        out.extend(0x0403_4b50u32.to_le_bytes());
        out.extend([20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        out.extend(crc.to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((data.len() as u32).to_le_bytes());
        out.extend((name.len() as u16).to_le_bytes());
        out.extend([0, 0]);
        out.extend(name.as_bytes());
        out.extend(data);
        central.extend(0x0201_4b50u32.to_le_bytes());
        central.extend([20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        central.extend(crc.to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((data.len() as u32).to_le_bytes());
        central.extend((name.len() as u16).to_le_bytes());
        central.extend([0; 12]);
        central.extend(offset.to_le_bytes());
        central.extend(name.as_bytes());
    }
    let start = out.len() as u32;
    let size = central.len() as u32;
    out.extend(central);
    out.extend(0x0605_4b50u32.to_le_bytes());
    out.extend([0, 0, 0, 0]);
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend((entries.len() as u16).to_le_bytes());
    out.extend(size.to_le_bytes());
    out.extend(start.to_le_bytes());
    out.extend([0, 0]);
    out
}

#[test]
fn help_and_version_exit_0_and_say_what_the_command_does() {
    let o = alrunner(&["--help"]);
    assert_eq!(code(&o), 0);
    let t = text(&o);
    assert!(t.contains("alrunner test [OPTIONS] APP..."), "{t}");
    assert!(t.contains("--dep PATH") && t.contains("--package-cache DIR"), "{t}");
    assert!(t.contains("7  some tests were not run"), "{t}");
    assert_eq!(code(&alrunner(&["--version"])), 0);
}

#[test]
fn a_bad_invocation_exits_2_and_says_why() {
    for (args, why) in [
        (vec!["test"], "at least one APP"),
        (vec!["test", ".", "--nope"], "unknown option"),
        (vec!["test", ".", "--test-data-company", "X"], "needs --test-data"),
        (vec!["test", ".", "--test-data", "/no/such/Demo.bak"], "Demo.bak"),
        (vec!["serve"], "not implemented yet"),
    ] {
        let o = alrunner(&args);
        assert_eq!(code(&o), 2, "{args:?}");
        assert!(text(&o).contains(why), "{args:?}: {}", text(&o));
    }
    let o = alrunner(&["test", "/no/such/app"]);
    assert_eq!(code(&o), 2);
    assert!(text(&o).contains("no such file or folder"), "{}", text(&o));
}

#[test]
fn a_run_reports_pass_fail_and_not_run_and_exits_1_on_a_failure() {
    let d = tempfile::tempdir().unwrap();
    let app = mixed_app(d.path());
    let o = alrunner(&["test", app.to_str().unwrap(), "--no-download", "--verbose"]);
    let t = text(&o);
    assert_eq!(code(&o), 1, "{t}");
    assert!(t.contains("PASS    50100 Mixed Tests.Passes"), "{t}");
    assert!(t.contains("PASS    50101 Other Tests.AlsoPasses"), "{t}");
    assert!(t.contains("FAIL    50100 Mixed Tests.Fails: boom 5"), "{t}");
    assert!(t.contains("NOT RUN 50100 Mixed Tests.Refused: "), "{t}");
    assert!(t.contains("Tests: 4 total, 2 passed, 1 failed, 1 not run"), "{t}");
    // The summary says failures are not to be trusted without test data.
    assert!(t.contains("no test data"), "{t}");
    assert!(t.contains("Top refusals"), "{t}");
    assert!(!t.contains("Top failures"), "failures are ranked only on request: {t}");
    assert!(t.contains("Result: FAILED (exit 1)"), "{t}");
    assert!(t.contains("threads: check 2 (AL_CHECK_THREADS)"), "{t}");
}

#[test]
fn not_run_tests_exit_7_unless_allowed() {
    let d = tempfile::tempdir().unwrap();
    let app = d.path().join("refused");
    write(
        &app,
        "app.json",
        &app_json("11111111-0000-0000-0000-000000000002", "Refused", &[]),
    );
    write(
        &app,
        "T.al",
        "codeunit 50200 \"R\" { Subtype = Test; [Test] procedure Ok() begin end; [Test] procedure No() begin Codeunit.Run(Codeunit::\"Missing\"); end; }",
    );
    let o = alrunner(&["test", app.to_str().unwrap(), "--no-download"]);
    assert_eq!(code(&o), 7, "{}", text(&o));
    assert!(text(&o).contains("Result: TESTS NOT RUN (exit 7)"), "{}", text(&o));
    let o = alrunner(&["test", app.to_str().unwrap(), "--no-download", "--allow-not-run"]);
    assert_eq!(code(&o), 0, "{}", text(&o));
}

#[test]
fn selection_by_codeunit_and_filter_runs_codeunits_whole_and_reports_the_selected_tests() {
    let d = tempfile::tempdir().unwrap();
    let app = mixed_app(d.path());
    let a = app.to_str().unwrap();
    let o = alrunner(&["test", a, "--no-download", "--codeunit", "50101", "--verbose"]);
    let t = text(&o);
    assert_eq!(code(&o), 0, "{t}");
    assert!(t.contains("Tests: 1 total, 1 passed"), "{t}");
    assert!(t.contains("selection: 1 test codeunits of 2"), "{t}");
    // A filter picks tests by name, any case; the other tests of the codeunit are not reported.
    let o = alrunner(&["test", a, "--no-download", "--filter", "ALSO", "--filter", "fails"]);
    let t = text(&o);
    assert_eq!(code(&o), 1, "{t}");
    assert!(t.contains("Tests: 2 total, 1 passed, 1 failed, 0 not run"), "{t}");
    // Nothing matches: exit 6.
    let o = alrunner(&["test", a, "--no-download", "--filter", "no such test"]);
    assert_eq!(code(&o), 6, "{}", text(&o));
}

#[test]
fn junit_has_a_suite_per_codeunit_and_skipped_with_the_reason() {
    let d = tempfile::tempdir().unwrap();
    let app = mixed_app(d.path());
    let out = d.path().join("reports/deep/junit.xml");
    let o = alrunner(&[
        "test",
        app.to_str().unwrap(),
        "--no-download",
        "--junit",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 1, "{}", text(&o));
    let x = std::fs::read_to_string(&out).unwrap();
    assert!(
        x.contains("<testsuites tests=\"4\" failures=\"1\" errors=\"0\" skipped=\"1\""),
        "{x}"
    );
    assert!(x.contains("<testsuite name=\"Mixed Tests\" tests=\"3\""), "{x}");
    assert!(x.contains("<testsuite name=\"Other Tests\" tests=\"1\""), "{x}");
    assert!(x.contains("<failure message=\"boom 5\""), "{x}");
    assert!(x.contains("<skipped message=\""), "{x}");
}

#[test]
fn an_unwritable_junit_path_fails_before_the_run() {
    let d = tempfile::tempdir().unwrap();
    let app = mixed_app(d.path());
    let blocker = d.path().join("file");
    std::fs::write(&blocker, "x").unwrap();
    let bad = blocker.join("junit.xml");
    let o = alrunner(&[
        "test",
        app.to_str().unwrap(),
        "--no-download",
        "--junit",
        bad.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 2, "{}", text(&o));
    assert!(!text(&o).contains("Tests:"), "{}", text(&o));
}

/// A test app in a source archive, and a library it depends on in another.
fn archives(root: &Path) -> (PathBuf, PathBuf) {
    let lib_id = "22222222-0000-0000-0000-000000000001";
    let lib = root.join("Lib.Source.zip");
    std::fs::write(
        &lib,
        zip(&[
            ("app.json", &app_json(lib_id, "Lib", &[])),
            (
                "src/Helper.Codeunit.al",
                "codeunit 50300 \"Lib Helper\" { procedure Answer(): Integer begin exit(42); end; }",
            ),
        ]),
    )
    .unwrap();
    let app = root.join("Tests-Lib.Source.zip");
    std::fs::write(
        &app,
        zip(&[
            ("app.json", &app_json("22222222-0000-0000-0000-000000000002", "Tests-Lib", &[(lib_id, "Lib")])),
            (
                "T.Codeunit.al",
                "codeunit 50301 \"Lib Tests\" { Subtype = Test; [Test] procedure UsesTheLibrary() var H: Codeunit \"Lib Helper\"; begin if H.Answer() <> 42 then Error('wrong'); end; }",
            ),
        ]),
    )
    .unwrap();
    (app, lib)
}

#[test]
fn a_source_archive_runs_with_its_dependency_given_by_dep() {
    let d = tempfile::tempdir().unwrap();
    let (app, lib) = archives(d.path());
    let o = alrunner(&[
        "test",
        app.to_str().unwrap(),
        "--dep",
        lib.to_str().unwrap(),
        "--no-download",
    ]);
    let t = text(&o);
    assert_eq!(code(&o), 0, "{t}");
    assert!(t.contains("Tests: 1 total, 1 passed"), "{t}");
    assert!(t.contains("dependencies: 1 apps"), "{t}");
    // The same, with the library found in a folder of packages.
    let o = alrunner(&[
        "test",
        app.to_str().unwrap(),
        "--package-cache",
        d.path().to_str().unwrap(),
        "--no-download",
    ]);
    assert_eq!(code(&o), 0, "{}", text(&o));
}

#[test]
fn a_missing_dependency_refuses_the_tests_that_need_it_instead_of_failing_the_run() {
    let d = tempfile::tempdir().unwrap();
    let (app, _lib) = archives(d.path());
    let o = alrunner(&["test", app.to_str().unwrap(), "--no-download"]);
    let t = text(&o);
    assert_eq!(code(&o), 7, "{t}");
    assert!(t.contains("1 not run"), "{t}");
    assert!(t.contains("is outside the workspace"), "{t}");
}

#[test]
fn an_app_folder_may_be_given_as_a_dependency_and_a_build_fills_microsoft_variables() {
    let d = tempfile::tempdir().unwrap();
    // The library is a folder; its version is one of Microsoft's build variables.
    let lib = d.path().join("lib");
    write(
        &lib,
        "app.json",
        r#"{"id":"22222222-0000-0000-0000-000000000009","name":"Lib","publisher":"Test","version":"$(app_currentVersion)","dependencies":[]}"#,
    );
    write(
        &lib,
        "H.al",
        "codeunit 50400 \"Lib Helper\" { procedure Answer(): Integer begin exit(7); end; }",
    );
    let app = d.path().join("app");
    write(
        &app,
        "app.json",
        &app_json(
            "22222222-0000-0000-0000-00000000000a",
            "App",
            &[("22222222-0000-0000-0000-000000000009", "Lib")],
        ),
    );
    write(
        &app,
        "T.al",
        "codeunit 50401 \"T\" { Subtype = Test; [Test] procedure Uses() var H: Codeunit \"Lib Helper\"; begin if H.Answer() <> 7 then Error('wrong'); end; }",
    );
    // Without a build the variable cannot be filled: said, not guessed.
    let o = alrunner(&[
        "test",
        app.to_str().unwrap(),
        "--dep",
        lib.to_str().unwrap(),
        "--no-download",
    ]);
    assert!(text(&o).contains("$(app_"), "{}", text(&o));
    // An exact build fills it.
    let o = alrunner(&[
        "test",
        app.to_str().unwrap(),
        "--dep",
        lib.to_str().unwrap(),
        "--no-download",
        "--bc-version",
        "27.5.46862.48827",
    ]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(text(&o).contains("Tests: 1 total, 1 passed"), "{}", text(&o));
}

const TP_TESTS: &str = r#"
table 50100 "exttest" { fields { field(1; id; Integer) { } field(2; own; Text[50]) { } } keys { key(PK; id) { } } }
table 50101 "stray" { fields { field(1; id; Integer) { } } keys { key(PK; id) { } } }
codeunit 50100 "Data Tests" { Subtype = Test;
  [Test] procedure RestoredRows() var E: Record "exttest"; begin
    if E.Count() <> 3 then Error('count %1', E.Count());
    E.Get(2);
    if E.own <> 'base-two' then Error(E.own);
    if CompanyName() <> 'TP' then Error(CompanyName());
  end;
  [Test] procedure AnEmptyTable() var S: Record "stray"; begin if not S.IsEmpty() then Error('rows'); end; }
"#;

fn typeprobe() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../al-bcdb/tests/fixtures/typeprobe.bak")
}

#[test]
fn test_data_restores_a_company_from_a_backup_and_the_summary_says_what_was_loaded() {
    let d = tempfile::tempdir().unwrap();
    let dir = d.path().join("tp");
    write(
        &dir,
        "app.json",
        &app_json("11111111-0000-0000-0000-000000000002", "TP", &[]),
    );
    write(&dir, "src/Tests.al", TP_TESTS);
    let bak = typeprobe();
    let o = alrunner(&[
        "test",
        dir.to_str().unwrap(),
        "--no-download",
        "--test-data",
        bak.to_str().unwrap(),
        "--test-data-company",
        "TP",
    ]);
    let t = text(&o);
    assert_eq!(code(&o), 0, "{t}");
    assert!(t.contains("Tests: 2 total, 2 passed, 0 failed, 0 not run"), "{t}");
    assert!(
        !t.contains("no test data"),
        "a run with test data carries no such warning: {t}"
    );
    assert!(
        t.contains("Test data: 3 rows loaded from 1 tables of company 'TP' in typeprobe.bak"),
        "{t}"
    );
    assert!(
        t.contains("test data: ") && t.contains("typeprobe.bak (company 'TP'"),
        "the header names the source: {t}"
    );
}

#[test]
fn a_company_the_backup_does_not_hold_is_a_start_up_error_that_lists_the_companies() {
    let d = tempfile::tempdir().unwrap();
    let app = mixed_app(d.path());
    let bak = typeprobe();
    let o = alrunner(&[
        "test",
        app.to_str().unwrap(),
        "--no-download",
        "--test-data",
        bak.to_str().unwrap(),
        "--test-data-company",
        "Nope",
    ]);
    assert_eq!(code(&o), 2);
    assert!(
        text(&o).contains("'ProbeCo'") && text(&o).contains("'TP'"),
        "{}",
        text(&o)
    );
}

#[test]
fn top_failures_ranks_the_failure_messages_with_numbers_folded() {
    let d = tempfile::tempdir().unwrap();
    let app = mixed_app(d.path());
    let o = alrunner(&["test", app.to_str().unwrap(), "--no-download", "--top-failures", "5"]);
    let t = text(&o);
    assert!(
        t.contains("Top failures, failed tests: 1 failed, 1 distinct reasons"),
        "{t}"
    );
    assert!(t.contains("boom N"), "{t}");
}

#[test]
fn explain_shows_where_a_failing_test_raised_its_error() {
    let d = tempfile::tempdir().unwrap();
    let app = mixed_app(d.path());
    let o = alrunner(&["test", app.to_str().unwrap(), "--no-download", "--explain", "Fails"]);
    let t = text(&o);
    assert!(t.contains("runtime: FAIL"), "{t}");
    assert!(t.contains("error: boom 5"), "{t}");
    assert!(t.contains("raised in (innermost first):"), "{t}");
    assert!(t.contains("Mixed Tests") && t.contains("Fails"), "{t}");
    assert!(t.contains("src/Tests.al:"), "the source position is named: {t}");
    assert_eq!(code(&o), 1, "{t}");
}

#[test]
fn explain_shows_where_a_refused_test_was_refused() {
    let d = tempfile::tempdir().unwrap();
    let app = mixed_app(d.path());
    let o = alrunner(&["test", app.to_str().unwrap(), "--no-download", "--explain", "Refused"]);
    let t = text(&o);
    assert!(t.contains("runtime: NOT RUN (refused)"), "{t}");
    assert!(t.contains("reason: "), "{t}");
    assert_eq!(code(&o), 7, "{t}");
}

#[test]
fn explain_needs_exactly_one_test() {
    let d = tempfile::tempdir().unwrap();
    let app = mixed_app(d.path());
    let o = alrunner(&["test", app.to_str().unwrap(), "--no-download", "--explain", "Pass"]);
    let t = text(&o);
    assert!(t.contains("matches 2 tests"), "{t}");
    assert_eq!(code(&o), 2, "{t}");
    let o = alrunner(&[
        "test",
        app.to_str().unwrap(),
        "--no-download",
        "--explain",
        "NoSuchTest",
    ]);
    assert_eq!(code(&o), 6, "{}", text(&o));
}

const DEEP: &str = r#"
codeunit 50200 "Deep Tests"
{
    Subtype = Test;

    local procedure Inner()
    begin
        Error('deep boom');
    end;

    local procedure Middle()
    begin
        Inner();
    end;

    [Test]
    procedure DeepFails()
    begin
        Middle();
    end;
}
"#;

fn deep_app(root: &Path) -> PathBuf {
    let dir = root.join("deep");
    write(
        &dir,
        "app.json",
        &app_json("11111111-0000-0000-0000-000000000002", "Deep", &[]),
    );
    write(&dir, "src/Deep.al", DEEP);
    dir
}

#[test]
fn a_failure_shows_the_al_call_stack_in_the_console_and_in_junit() {
    let d = tempfile::tempdir().unwrap();
    let app = deep_app(d.path());
    let junit = d.path().join("junit.xml");
    let o = alrunner(&[
        "test",
        app.to_str().unwrap(),
        "--no-download",
        "--junit",
        junit.to_str().unwrap(),
    ]);
    let t = text(&o);
    assert_eq!(code(&o), 1, "{t}");
    // Innermost frame first: Inner, Middle, then the test itself.
    let frames: Vec<&str> = t
        .lines()
        .filter(|l| l.trim_start().starts_with("\"Deep Tests\"(CodeUnit 50200)"))
        .collect();
    assert_eq!(frames.len(), 3, "{t}");
    assert!(frames[0].contains(".Inner line 2"), "{t}");
    assert!(frames[1].contains(".Middle line 2"), "{t}");
    assert!(frames[2].contains(".DeepFails line 2"), "{t}");
    assert!(
        t.lines().any(|l| l.starts_with("FAIL") && l.contains("deep boom")),
        "{t}"
    );
    let x = std::fs::read_to_string(&junit).unwrap();
    assert!(x.contains("<failure message=\"deep boom\">"), "{x}");
    assert!(x.contains(".Inner line 2"), "{x}");
    assert!(x.contains("</failure>"), "{x}");
}

#[test]
fn quiet_leaves_the_call_stack_out_of_the_console() {
    let d = tempfile::tempdir().unwrap();
    let app = deep_app(d.path());
    let o = alrunner(&["test", app.to_str().unwrap(), "--no-download", "--quiet"]);
    assert!(!text(&o).contains("CodeUnit 50200"), "{}", text(&o));
}
