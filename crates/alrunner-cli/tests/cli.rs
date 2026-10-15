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
fn version_prints_the_crate_version_and_the_commit() {
    let o = alrunner(&["--version"]);
    let t = text(&o);
    let want = format!("alrunner {} (", env!("CARGO_PKG_VERSION"));
    assert!(t.starts_with(&want) && t.trim_end().ends_with(')'), "{t}");
    assert!(env!("CARGO_PKG_VERSION").starts_with("3.0.0-alpha."), "{t}");
}

#[test]
fn a_bad_invocation_exits_2_and_says_why() {
    for (args, why) in [
        (vec!["test"], "at least one APP"),
        (vec!["test", ".", "--nope"], "unknown option"),
        (vec!["test", ".", "--test-data", "--no-test-data"], "cannot be combined"),
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

#[test]
fn warm_builds_the_index_of_a_backup_so_that_the_first_run_does_not() {
    let backup = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../al-bcdb/tests/fixtures/typeprobe.bak");
    let cache = tempfile::tempdir().unwrap();
    let args = [
        "warm",
        backup.to_str().unwrap(),
        "--test-data-company",
        "TP",
        "--cache-dir",
        cache.path().to_str().unwrap(),
    ];
    let o = alrunner(&args);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(text(&o).contains("built and saved"), "{}", text(&o));
    let dir = cache.path().join("backup-index");
    let files: Vec<_> = std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().path()).collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let o = alrunner(&args);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(text(&o).contains("kept map and catalog used"), "{}", text(&o));
}

#[test]
fn warm_without_a_backup_or_a_build_says_what_it_needs() {
    let o = alrunner(&["warm"]);
    assert_eq!(code(&o), 2);
    assert!(
        text(&o).contains("PATH.bak") && text(&o).contains("--bc-version"),
        "{}",
        text(&o)
    );
}

// ---- the interface the v2 tools use (docs/integrations/alchemist-lethal.md, step 1) ----

fn json_of(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| panic!("stdout is not one JSON document: {e}\n{}", text(o)))
}

fn row<'a>(doc: &'a serde_json::Value, name: &str) -> &'a serde_json::Value {
    doc["tests"]
        .as_array()
        .unwrap()
        .iter()
        .find(|t| t["name"] == name)
        .unwrap_or_else(|| panic!("no row {name} in {doc}"))
}

#[test]
fn output_json_without_a_file_is_the_only_thing_on_stdout_and_has_a_row_per_test() {
    let d = tempfile::tempdir().unwrap();
    let app = mixed_app(d.path());
    let o = alrunner(&["test", app.to_str().unwrap(), "--no-download", "--output-json"]);
    assert_eq!(code(&o), 1, "{}", text(&o));
    // The report is on stderr, the document alone on stdout, starting with `{` in column 0.
    assert!(String::from_utf8_lossy(&o.stdout).starts_with("{\n"));
    assert!(String::from_utf8_lossy(&o.stderr).contains("Result: FAILED (exit 1)"));
    let v = json_of(&o);
    assert_eq!(
        (
            v["total"].as_u64(),
            v["passed"].as_u64(),
            v["failed"].as_u64(),
            v["errors"].as_u64()
        ),
        (Some(4), Some(2), Some(1), Some(1))
    );
    assert_eq!((v["skipped"].as_u64(), v["exitCode"].as_u64()), (Some(0), Some(1)));
    assert_eq!(row(&v, "Codeunit50100.Passes")["status"], "pass");
    let fails = row(&v, "Codeunit50100.Fails");
    assert_eq!(
        (fails["status"].as_str(), fails["message"].as_str()),
        (Some("fail"), Some("boom 5"))
    );
    assert!(
        fails["stackTrace"].as_str().is_some_and(|s| s.contains("Fails")),
        "{fails}"
    );
    let refused = row(&v, "Codeunit50100.Refused");
    assert_eq!(refused["status"], "error");
    assert!(
        refused["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("NotRun: out-of-scope: ")),
        "{refused}"
    );
    assert_eq!(refused["notRun"], true);
}

#[test]
fn output_json_with_a_file_keeps_the_report_on_stdout() {
    let d = tempfile::tempdir().unwrap();
    let app = mixed_app(d.path());
    let out = d.path().join("r/out.json");
    let o = alrunner(&[
        "test",
        app.to_str().unwrap(),
        "--no-download",
        "--output-json",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(String::from_utf8_lossy(&o.stdout).contains("Result: FAILED (exit 1)"));
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(v["total"], 4);
}

#[test]
fn a_qualified_test_name_selects_one_test_and_exclude_test_removes_tests() {
    let d = tempfile::tempdir().unwrap();
    let app = mixed_app(d.path());
    let a = app.to_str().unwrap();
    let run = |extra: &[&str]| {
        let mut args = vec!["test", a, "--no-download", "--output-json"];
        args.extend_from_slice(extra);
        alrunner(&args)
    };
    let o = run(&["--test", "Codeunit50100.Passes"]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let v = json_of(&o);
    assert_eq!(v["total"], 1);
    assert_eq!(row(&v, "Codeunit50100.Passes")["status"], "pass");
    // The qualified name is matched in any case; a sibling of another codeunit with the same method name is not picked.
    let o = run(&["--test", "codeunit50101.alsopasses"]);
    assert_eq!(json_of(&o)["total"], 1);
    // Nothing matches: exit 6.
    assert_eq!(code(&run(&["--test", "Codeunit50100.AlsoPasses"])), 6);
    // --test-exact.
    let v = json_of(&run(&["--test-exact", "Passes"]));
    assert_eq!(v["total"], 1);
    // --exclude-test leaves out whole qualified names and wins over the selection.
    let o = run(&[
        "--exclude-test",
        "Codeunit50100.Fails",
        "--exclude-test",
        "Codeunit50100.Refused",
    ]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert_eq!(json_of(&o)["total"], 2);
    assert_eq!(
        code(&run(&[
            "--test",
            "Codeunit50100.Passes",
            "--exclude-test",
            "Codeunit50100.Passes"
        ])),
        6
    );
}

const ISO: &str = r#"
table 50300 "Iso Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
codeunit 50300 "Iso Tests"
{
    Subtype = Test;

    [Test]
    procedure A_Inserts()
    var
        R: Record "Iso Row";
    begin
        R.Id := 1;
        R.Insert();
    end;

    [Test]
    procedure B_SeesTheRow()
    var
        R: Record "Iso Row";
    begin
        if R.Count() <> 0 then
            Error('kept');
    end;
}
"#;

#[test]
fn isolation_decides_whether_the_database_is_rolled_back_between_the_tests_of_a_codeunit() {
    let d = tempfile::tempdir().unwrap();
    let app = d.path().join("iso");
    write(
        &app,
        "app.json",
        &app_json("11111111-0000-0000-0000-000000000003", "Iso", &[]),
    );
    write(&app, "T.al", ISO);
    let a = app.to_str().unwrap();
    let run = |extra: &[&str]| {
        let mut args = vec!["test", a, "--no-download", "--no-test-data", "--output-json"];
        args.extend_from_slice(extra);
        alrunner(&args)
    };
    // The default, Codeunit isolation: the second test sees the first one's row.
    let o = run(&[]);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert_eq!(row(&json_of(&o), "Codeunit50300.B_SeesTheRow")["message"], "kept");
    // test (and its v2 and BC spellings): the database goes back after every test.
    for mode in ["test", "function", "method"] {
        let o = run(&["--isolation", mode]);
        assert_eq!(code(&o), 0, "{mode}: {}", text(&o));
        assert_eq!(json_of(&o)["passed"], 2);
    }
    // disabled is not pinned: every test is reported as not run.
    let o = run(&["--isolation", "disabled"]);
    assert_eq!(code(&o), 7, "{}", text(&o));
    assert_eq!(json_of(&o)["notRun"], 2);
    assert_eq!(code(&run(&["--isolation", "nope"])), 2);
}

#[test]
fn define_adds_a_preprocessor_symbol_and_the_v2_flags_are_accepted() {
    let d = tempfile::tempdir().unwrap();
    let app = d.path().join("defs");
    write(
        &app,
        "app.json",
        &app_json("11111111-0000-0000-0000-000000000004", "Defs", &[]),
    );
    write(
        &app,
        "T.al",
        "codeunit 50400 \"Defs\" { Subtype = Test; [Test] procedure Symbol() begin\n#if LETHAL_MUTANT\n Error('mutant');\n#endif\n end; }",
    );
    let a = app.to_str().unwrap();
    assert_eq!(code(&alrunner(&["test", a, "--no-download", "--no-test-data"])), 0);
    let o = alrunner(&[
        "test",
        a,
        "--no-download",
        "--no-test-data",
        "--define",
        "LETHAL_MUTANT",
        "--auto-provision",
        "--test-timeout",
        "30",
    ]);
    assert_eq!(code(&o), 1, "{}", text(&o));
    assert!(text(&o).contains("mutant"), "{}", text(&o));
    // The v2 variable is read; a value that is no number is an error.
    let o = Command::new(env!("CARGO_BIN_EXE_alrunner"))
        .env("AL_RUNNER_TEST_TIMEOUT_SEC", "soon")
        .args(["test", a, "--no-download", "--no-test-data"])
        .output()
        .unwrap();
    assert_eq!(code(&o), 2, "{}", text(&o));
    assert!(text(&o).contains("AL_RUNNER_TEST_TIMEOUT_SEC"), "{}", text(&o));
}

// ---- coverage (docs/integrations/alchemist-lethal.md, step 2) ----

const COVERED: &str = r#"
codeunit 50500 "Covered Tests"
{
    Subtype = Test;

    [Test]
    procedure TakesTheThenBranch()
    var
        N: Integer;
    begin
        N := 1;
        if N = 1 then
            N := 2
        else
            N := 3;
    end;

    [Test]
    procedure TakesTheElseBranch()
    var
        N: Integer;
    begin
        N := 5;
        if N = 1 then
            N := 2
        else
            N := 3;
    end;
}
"#;

fn covered_app(root: &Path) -> PathBuf {
    let dir = root.join("covered");
    write(
        &dir,
        "app.json",
        &app_json("11111111-0000-0000-0000-000000000005", "Covered", &[]),
    );
    write(&dir, "src/Covered.al", COVERED);
    dir
}

#[test]
fn coverage_writes_cobertura_with_the_runs_of_each_line_and_json_with_each_tests_statements() {
    let d = tempfile::tempdir().unwrap();
    let app = covered_app(d.path());
    let xml = d.path().join("out/c.xml");
    let js = d.path().join("out/c.json");
    let o = alrunner(&[
        "test",
        app.to_str().unwrap(),
        "--no-download",
        "--no-test-data",
        "--coverage-out",
        xml.to_str().unwrap(),
        "--coverage-json",
        js.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(text(&o).contains("coverage: "), "{}", text(&o));
    // Cobertura: a class for the file with a line per statement line. Test 1: `N := 1;` (11), the `if` (12), the then
    // branch (13) run, the else branch (15) does not. Test 2: `N := 5;` (23), the `if` (24), the else branch (27) run,
    // the then branch (25) does not.
    let x = std::fs::read_to_string(&xml).unwrap();
    assert!(x.contains("filename=\"") && x.contains("Covered.al\""), "{x}");
    assert!(x.contains("<line number=\"11\" hits=\"1\""), "{x}");
    assert!(x.contains("<line number=\"12\" hits=\"1\""), "{x}");
    let line_hits = |n: u32| -> u64 {
        let tag = format!("<line number=\"{n}\" hits=\"");
        let at = x.find(&tag).unwrap_or_else(|| panic!("no line {n} in {x}")) + tag.len();
        x[at..].split('"').next().unwrap().parse().unwrap()
    };
    assert_eq!(
        (line_hits(13), line_hits(15), line_hits(25), line_hits(27)),
        (1, 0, 0, 1)
    );
    // JSON: every statement, and per test the statements it ran.
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&js).unwrap()).unwrap();
    let file = &v["coverage"][0];
    assert!(file["file"].as_str().unwrap().ends_with("/src/Covered.al"), "{file}");
    let statements = file["statements"].as_array().unwrap();
    assert!(
        statements
            .iter()
            .any(|s| s["scope"] == "TakesTheThenBranch" && s["line"] == 13 && s["hits"] == 1)
    );
    let per_test = v["perTestCoverage"].as_array().unwrap();
    let then = per_test
        .iter()
        .find(|t| t["test"] == "Codeunit50500.TakesTheThenBranch")
        .unwrap();
    let ran: Vec<u64> = then["coverage"][0]["statements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| s["line"].as_u64().unwrap())
        .collect();
    assert!(ran.contains(&13) && !ran.contains(&15), "{ran:?}");
}

#[test]
fn coverage_off_writes_nothing_and_a_selection_counts_only_the_selected_tests() {
    let d = tempfile::tempdir().unwrap();
    let app = covered_app(d.path());
    let xml = d.path().join("c.xml");
    let o = alrunner(&["test", app.to_str().unwrap(), "--no-download", "--no-test-data"]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    assert!(!text(&o).contains("coverage:"), "{}", text(&o));
    let o = alrunner(&[
        "test",
        app.to_str().unwrap(),
        "--no-download",
        "--no-test-data",
        "--test",
        "Codeunit50500.TakesTheElseBranch",
        "--coverage-out",
        xml.to_str().unwrap(),
    ]);
    assert_eq!(code(&o), 0, "{}", text(&o));
    let x = std::fs::read_to_string(&xml).unwrap();
    // Only the second test counts: its else branch ran, the first test's then branch did not.
    assert!(x.contains("<line number=\"27\" hits=\"1\""), "{x}");
    assert!(x.contains("<line number=\"13\" hits=\"0\""), "{x}");
}
