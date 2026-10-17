//! `alrunner serve` end to end (ADR-028): the v2 protocol on stdin and stdout.

use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

struct Server {
    child: Child,
    stdin: Option<ChildStdin>,
    stdout: BufReader<ChildStdout>,
}

impl Server {
    fn start(extra: &[&str]) -> Server {
        Server::start_with(extra, &[])
    }

    fn start_with(extra: &[&str], env: &[(&str, &str)]) -> Server {
        let mut child = Command::new(env!("CARGO_BIN_EXE_alrunner"))
            .arg("serve")
            .args(extra)
            .envs(env.iter().copied())
            .env("AL_CHECK_THREADS", "2")
            .env("AL_LOWER_THREADS", "2")
            .env("AL_TEST_THREADS", "2")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start alrunner serve");
        let stdin = child.stdin.take();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Server { child, stdin, stdout }
    }

    fn send(&mut self, v: Value) {
        let stdin = self.stdin.as_mut().unwrap();
        writeln!(stdin, "{v}").unwrap();
        stdin.flush().unwrap();
    }

    /// The next line, which must be one JSON object.
    fn line(&mut self) -> Value {
        let mut s = String::new();
        assert!(self.stdout.read_line(&mut s).unwrap() > 0, "the server closed stdout");
        serde_json::from_str(&s).unwrap_or_else(|e| panic!("stdout holds more than protocol: {e}: {s}"))
    }

    /// Lines up to and including the first one of `type` summary or an error.
    fn until_summary(&mut self) -> Vec<Value> {
        let mut out = Vec::new();
        loop {
            let l = self.line();
            let done = l["type"] == "summary" || l.get("error").is_some();
            out.push(l);
            if done {
                return out;
            }
        }
    }
}

fn write(dir: &Path, rel: &str, content: &str) {
    let p = dir.join(rel);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, content).unwrap();
}

fn app(root: &Path, name: &str, al: &str) -> PathBuf {
    let dir = root.join(name);
    write(
        &dir,
        "app.json",
        &format!(
            r#"{{"id":"11111111-0000-0000-0000-0000000000{:02}","name":"{name}","publisher":"Test","version":"1.0.0.0","dependencies":[]}}"#,
            name.len() % 100
        ),
    );
    write(&dir, "src/T.al", al);
    dir
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
}
"#;

#[test]
fn a_run_streams_test_lines_then_a_summary_and_shutdown_ends_the_server() {
    let d = tempfile::tempdir().unwrap();
    let a = app(d.path(), "mixed", MIXED);
    let mut s = Server::start(&["--no-download", "--no-test-data"]);
    assert_eq!(s.line(), json!({"ready": true}));
    // Case-insensitive command names, as in v2.
    s.send(json!({"command": "RunTests", "sourcePaths": [a], "coverage": true, "perTestCoverage": true}));
    let lines = s.until_summary();
    let tests: Vec<&Value> = lines.iter().filter(|l| l["type"] == "test").collect();
    assert_eq!(tests.len(), 2, "{lines:?}");
    let fails = tests.iter().find(|t| t["name"] == "Codeunit50100.Fails").unwrap();
    assert_eq!(
        (fails["status"].as_str(), fails["message"].as_str()),
        (Some("fail"), Some("boom 5"))
    );
    assert!(
        fails["stackFrames"].as_array().is_some_and(|f| !f.is_empty()),
        "{fails}"
    );
    let summary = lines.last().unwrap();
    assert_eq!(summary["type"], "summary");
    assert_eq!(
        (
            summary["exitCode"].as_u64(),
            summary["passed"].as_u64(),
            summary["failed"].as_u64(),
            summary["total"].as_u64()
        ),
        (Some(1), Some(1), Some(1), Some(2))
    );
    assert_eq!(
        (summary["protocolVersion"].as_u64(), summary["cached"].as_bool()),
        (Some(2), Some(false))
    );
    assert!(
        summary["coverage"][0]["file"].as_str().unwrap().ends_with("src/T.al"),
        "{summary}"
    );
    // Only the test that ran a statement has an entry: Passes has an empty body.
    assert_eq!(summary["perTestCoverage"].as_array().unwrap().len(), 1);
    assert_eq!(summary["perTestCoverage"][0]["test"], "Codeunit50100.Fails");
    assert_eq!(summary["cached"], false, "the first request compiles");
    // A second request on the same server: selection and exclusion, on the compile of the first.
    s.send(json!({"command": "runTests", "sourcePaths": [a], "test": "Codeunit50100.Passes", "excludeTests": ["Codeunit50100.Fails"]}));
    let lines = s.until_summary();
    assert_eq!(lines.iter().filter(|l| l["type"] == "test").count(), 1, "{lines:?}");
    assert_eq!(lines.last().unwrap()["exitCode"], 0);
    assert_eq!(
        lines.last().unwrap()["cached"],
        true,
        "nothing changed, nothing is compiled: {lines:?}"
    );
    assert!(lines.last().unwrap().get("coverage").is_none());
    s.send(json!({"command": "shutdown"}));
    assert_eq!(s.line(), json!({"status": "shutting down"}));
    assert!(s.child.wait().unwrap().success());
}

#[test]
fn bad_requests_get_an_error_line_and_the_server_goes_on() {
    let d = tempfile::tempdir().unwrap();
    let a = app(d.path(), "mixed", MIXED);
    let mut s = Server::start(&["--no-download"]);
    s.line();
    s.stdin.as_mut().unwrap().write_all(b"this is not json\n").unwrap();
    assert!(s.line()["error"].as_str().unwrap().contains("JSON"));
    for (request, needle) in [
        (json!({"command": "frobnicate"}), "unknown command"),
        (json!({"nocommand": 1}), "needs a `command`"),
        (json!({"command": "runTests"}), "sourcePaths"),
        (
            json!({"command": "runTests", "sourcePaths": [a], "bogus": 1}),
            "unknown field",
        ),
        (
            json!({"command": "runTests", "sourcePaths": [a], "affectedOnly": true}),
            "deferred",
        ),
        (
            json!({"command": "runTests", "sourcePaths": ["/no/such/app"]}),
            "no such file or folder",
        ),
        (
            json!({"command": "execute", "sourcePaths": ["/no/such/app"]}),
            "no such file or folder",
        ),
    ] {
        s.send(request);
        let l = s.line();
        assert!(l["error"].as_str().is_some_and(|e| e.contains(needle)), "{needle}: {l}");
    }
    // cancel with nothing running is a no-op that says so.
    s.send(json!({"command": "cancel"}));
    assert_eq!(s.line(), json!({"type": "ack", "command": "cancel", "noop": true}));
    // A field of another command only earns a warning.
    s.send(json!({"command": "runTests", "sourcePaths": [a], "code": "x", "test": "Passes"}));
    let lines = s.until_summary();
    assert_eq!(
        lines.last().unwrap()["warnings"].as_array().unwrap().len(),
        1,
        "{lines:?}"
    );
    // End of input ends the server.
    drop(s.stdin.take());
    assert!(s.child.wait().unwrap().success());
}

const SLOW: &str = r#"
codeunit 50200 "Slow Tests"
{
    Subtype = Test;

    [Test]
    procedure Spins()
    var
        i: Integer;
    begin
        for i := 1 to 2000000000 do;
    end;

    [Test]
    procedure SpinsToo()
    var
        i: Integer;
    begin
        for i := 1 to 2000000000 do;
    end;
}
"#;

#[test]
fn cancel_stops_the_run_after_the_current_test_and_reports_what_ran() {
    let d = tempfile::tempdir().unwrap();
    let a = app(d.path(), "slow", SLOW);
    // The CPU limit ends the first test after 2 s; the second never starts.
    let mut s = Server::start(&["--no-download", "--no-test-data", "--test-timeout", "2"]);
    s.line();
    s.send(json!({"command": "runTests", "sourcePaths": [a], "announceStart": true}));
    // A cancel that arrives while the app still compiles ends the run with no test at all (also correct, but not what
    // this test is about), so wait until the first test starts.
    let started = s.line();
    assert_eq!(started["type"], "started", "{started}");
    s.send(json!({"command": "cancel"}));
    assert_eq!(s.line(), json!({"type": "ack", "command": "cancel", "noop": false}));
    let lines = s.until_summary();
    let tests: Vec<&Value> = lines.iter().filter(|l| l["type"] == "test").collect();
    assert_eq!(
        tests.len(),
        1,
        "the test that was running finished, the next did not start: {lines:?}"
    );
    assert_eq!(tests[0]["name"], "Codeunit50200.Spins");
    assert!(
        tests[0]["message"]
            .as_str()
            .unwrap()
            .starts_with("Test exceeded 2s timeout"),
        "{}",
        tests[0]
    );
    let summary = lines.last().unwrap();
    assert_eq!(
        (summary["cancelled"].as_bool(), summary["total"].as_u64()),
        (Some(true), Some(1)),
        "{summary}"
    );
    // The server still answers, and the next run is not cancelled.
    s.send(json!({"command": "cancel"}));
    assert_eq!(s.line()["noop"], true);
    s.send(json!({"command": "shutdown"}));
    assert_eq!(s.line(), json!({"status": "shutting down"}));
}

const WRITES: &str = r#"
table 50300 "Serve Row" { fields { field(1; Id; Integer) { } } keys { key(PK; Id) { } } }
codeunit 50300 "Serve Writes"
{
    Subtype = Test;

    [Test]
    procedure InsertsAfterCheckingTheTableIsEmpty()
    var
        R: Record "Serve Row";
    begin
        if not R.IsEmpty() then
            Error('the table was not empty: %1 row(s)', R.Count());
        R.Id := 1;
        R.Insert();
        R.Id := 2;
        R.Insert();
        Commit();
    end;
}
"#;

#[test]
fn a_test_that_writes_gives_the_same_answer_in_two_requests() {
    let d = tempfile::tempdir().unwrap();
    let a = app(d.path(), "writes", WRITES);
    let mut s = Server::start(&["--no-download", "--no-test-data"]);
    s.line();
    let mut outcomes = Vec::new();
    for _ in 0..3 {
        s.send(json!({"command": "runTests", "sourcePaths": [a]}));
        let lines = s.until_summary();
        let summary = lines.last().unwrap().clone();
        outcomes.push((
            lines[0]["status"].clone(),
            lines[0]["message"].clone(),
            summary["passed"].clone(),
            summary["cached"].clone(),
        ));
    }
    // Each request starts from the same database: the table is empty every time, though the test commits two rows.
    assert_eq!(outcomes[0].0, "pass", "{outcomes:?}");
    assert!(outcomes.iter().all(|o| o.0 == "pass" && o.2 == 1), "{outcomes:?}");
    // The later requests used the compile of the first.
    assert_eq!(
        (outcomes[0].3.clone(), outcomes[1].3.clone(), outcomes[2].3.clone()),
        (json!(false), json!(true), json!(true))
    );
}

#[test]
fn a_changed_source_is_compiled_again_and_an_unchanged_one_is_not() {
    let d = tempfile::tempdir().unwrap();
    let a = app(d.path(), "changing", MIXED);
    let mut s = Server::start(&["--no-download", "--no-test-data"]);
    s.line();
    let run = |s: &mut Server| {
        s.send(json!({"command": "runTests", "sourcePaths": [a], "test": "Codeunit50100.Fails"}));
        s.until_summary()
    };
    assert_eq!(run(&mut s).last().unwrap()["cached"], false);
    assert_eq!(run(&mut s).last().unwrap()["cached"], true);
    // The test is fixed on disk: the next request sees it.
    write(&a, "src/T.al", &MIXED.replace("Error('boom %1', 5);", ""));
    let lines = run(&mut s);
    assert_eq!(lines.last().unwrap()["cached"], false);
    assert_eq!(lines[0]["status"], "pass", "{lines:?}");
    // A request for a test the first compile did not reach compiles for both.
    s.send(json!({"command": "runTests", "sourcePaths": [a], "test": "Codeunit50100.Passes"}));
    let lines = s.until_summary();
    assert_eq!(lines.last().unwrap()["passed"], 1, "{lines:?}");
}

const TWO_UNITS: &str = r#"
codeunit 50400 "Boom Tests"
{
    Subtype = Test;

    [Test]
    procedure First()
    begin
    end;

    [Test]
    procedure Boom()
    begin
    end;

    [Test]
    procedure Third()
    begin
    end;
}

codeunit 50401 "Fine Tests"
{
    Subtype = Test;

    [Test]
    procedure Fine()
    begin
    end;
}
"#;

#[test]
fn a_panic_in_a_test_is_reported_as_an_error_and_the_server_goes_on() {
    let d = tempfile::tempdir().unwrap();
    let a = app(d.path(), "boom", TWO_UNITS);
    let mut s = Server::start_with(
        &["--no-download", "--no-test-data"],
        &[("ALRUNNER_DEBUG_PANIC_IN", "Boom")],
    );
    s.line();
    s.send(json!({"command": "runTests", "sourcePaths": [a]}));
    let lines = s.until_summary();
    let status = |name: &str| {
        lines
            .iter()
            .find(|l| l["name"] == name)
            .unwrap_or_else(|| panic!("no {name}: {lines:?}"))
            .clone()
    };
    assert_eq!(status("Codeunit50400.First")["status"], "pass");
    let boom = status("Codeunit50400.Boom");
    assert_eq!(boom["status"], "error");
    assert!(boom["message"].as_str().unwrap().contains("runtime panic"), "{boom}");
    // The rest of that codeunit is not run, with the reason; the other codeunit is.
    assert!(
        status("Codeunit50400.Third")["message"]
            .as_str()
            .unwrap()
            .contains("panicked")
    );
    assert_eq!(status("Codeunit50401.Fine")["status"], "pass");
    assert_eq!(lines.last().unwrap()["total"], 4);
    // The server answers the next request.
    s.send(json!({"command": "runTests", "sourcePaths": [a], "test": "Fine"}));
    assert_eq!(s.until_summary().last().unwrap()["passed"], 1);
}

fn rss_mb(pid: u32) -> u64 {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).unwrap();
    let kb: u64 = status
        .lines()
        .find_map(|l| l.strip_prefix("VmRSS:"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .parse()
        .unwrap();
    kb / 1024
}

#[test]
#[cfg(target_os = "linux")]
fn memory_stays_bounded_over_fifty_requests_that_change_the_source_every_time() {
    let d = tempfile::tempdir().unwrap();
    let a = app(d.path(), "many", MIXED);
    let mut s = Server::start(&["--no-download", "--no-test-data"]);
    s.line();
    let pid = s.child.id();
    let mut at_10 = 0;
    for i in 0..50 {
        // A changed file every time: the compile is dropped and made again 50 times.
        write(&a, "src/T.al", &format!("{MIXED}\n// request {i}\n"));
        s.send(json!({"command": "runTests", "sourcePaths": [a]}));
        let lines = s.until_summary();
        assert_eq!(lines.last().unwrap()["total"], 2, "{lines:?}");
        if i == 9 {
            at_10 = rss_mb(pid);
        }
    }
    let at_50 = rss_mb(pid);
    assert!(
        at_50 < at_10 + 60,
        "resident memory grew from {at_10} MB after 10 requests to {at_50} MB after 50"
    );
    s.send(json!({"command": "shutdown"}));
    s.line();
}

#[test]
fn a_request_sent_during_a_run_waits_for_it() {
    let d = tempfile::tempdir().unwrap();
    let a = app(d.path(), "slow2", SLOW);
    let b = app(d.path(), "mixed", MIXED);
    let mut s = Server::start(&["--no-download", "--no-test-data", "--test-timeout", "1"]);
    s.line();
    s.send(json!({"command": "runTests", "sourcePaths": [a], "test": "Codeunit50200.SpinsToo"}));
    s.send(json!({"command": "runTests", "sourcePaths": [b], "test": "Codeunit50100.Passes"}));
    let first = s.until_summary();
    assert_eq!(first.last().unwrap()["total"], 1);
    assert_eq!(first[0]["name"], "Codeunit50200.SpinsToo");
    let second = s.until_summary();
    assert_eq!(second[0]["name"], "Codeunit50100.Passes", "{second:?}");
}

// ---- execute ----

const SCRATCH: &str = r#"
codeunit 50200 "Later"
{
    trigger OnRun()
    begin
        Message('later');
    end;
}

codeunit 50150 "Scratch"
{
    trigger OnRun()
    var
        N: Integer;
    begin
        N := 40;
        Message('answer %1', N + 2);
        Message('second');
    end;
}

codeunit 50160 "Helper"
{
    procedure P()
    begin
    end;
}
"#;

fn started() -> Server {
    let mut s = Server::start(&["--no-download", "--no-test-data"]);
    assert_eq!(s.line(), json!({"ready": true}));
    s
}

#[test]
fn execute_runs_the_on_run_of_the_lowest_codeunit_of_an_app_and_returns_its_messages() {
    let d = tempfile::tempdir().unwrap();
    let a = app(d.path(), "scratch", SCRATCH);
    let mut s = started();
    s.send(json!({"command": "execute", "sourcePaths": [a]}));
    let r = s.line();
    // One line, no `type`, as v2 answers.
    assert!(r.get("type").is_none(), "{r}");
    assert_eq!(r["exitCode"], 0, "{r}");
    let t = &r["tests"][0];
    assert_eq!(t["name"], "Codeunit50150.OnRun", "{r}");
    assert_eq!(t["status"], "pass");
    assert_eq!(t["messages"], json!(["answer 42", "second"]));
    assert_eq!(r["tests"].as_array().unwrap().len(), 1);
    // A change in the code runs again with the new code (the warm compile takes it in).
    write(&a, "src/T.al", &SCRATCH.replace("N := 40;", "N := 50;"));
    s.send(json!({"command": "execute", "sourcePaths": [a]}));
    let r = s.line();
    assert_eq!(r["tests"][0]["messages"], json!(["answer 52", "second"]), "{r}");
    s.send(json!({"command": "shutdown"}));
    s.line();
}

#[test]
fn execute_runs_inline_code_a_statement_list_or_a_whole_object() {
    let mut s = started();
    s.send(json!({"command": "execute", "code": "Message('hi');"}));
    let r = s.line();
    assert_eq!(r["exitCode"], 0, "{r}");
    assert_eq!(r["tests"][0]["name"], "AL Runner Inline Execute.OnRun", "{r}");
    assert_eq!(r["tests"][0]["messages"], json!(["hi"]));
    // A comment first, then an object: used as it is.
    s.send(json!({"command": "execute", "code": "// scratch\ncodeunit 50100 X { trigger OnRun() begin Message('from x'); Error('boom %1', 3); end; }"}));
    let r = s.line();
    assert_eq!(r["exitCode"], 1, "{r}");
    let t = &r["tests"][0];
    assert_eq!(
        (t["name"].as_str(), t["status"].as_str()),
        (Some("X.OnRun"), Some("fail")),
        "{r}"
    );
    assert_eq!(t["message"], "boom 3");
    assert_eq!(t["messages"], json!(["from x"]));
    // The next statement list works after a failure.
    s.send(json!({"command": "execute", "code": "Message(Format(6 * 7));"}));
    assert_eq!(s.line()["tests"][0]["messages"], json!(["42"]));
    s.send(json!({"command": "shutdown"}));
    s.line();
}

#[test]
fn execute_checks_its_request_and_says_what_it_does_not_do_yet() {
    let d = tempfile::tempdir().unwrap();
    let a = app(d.path(), "scratch", SCRATCH);
    let none = app(d.path(), "none", "codeunit 50100 NoRun { procedure P() begin end; }");
    let mut s = started();
    let mut ask = |v: Value| {
        s.send(v);
        s.line()
    };
    assert!(
        ask(json!({"command": "execute"}))["error"]
            .as_str()
            .unwrap()
            .contains("sourcePaths")
    );
    assert!(
        ask(json!({"command": "execute", "code": "x", "sourcePaths": [a]}))["error"]
            .as_str()
            .unwrap()
            .contains("mutually exclusive")
    );
    assert!(
        ask(json!({"command": "execute", "code": "x", "bogus": 1}))["error"]
            .as_str()
            .unwrap()
            .contains("bogus")
    );
    assert!(
        ask(json!({"command": "execute", "code": "x", "tdd": true}))["error"]
            .as_str()
            .unwrap()
            .contains("deferred")
    );
    let r =
        ask(json!({"command": "execute", "code": "Message('a');", "captureValues": true, "iterationTracking": true}));
    assert_eq!(r["tests"][0]["messages"], json!(["a"]), "{r}");
    assert!(r.get("warnings").is_none(), "{r}");
    // A field of runTests: a warning, not an error.
    let r = ask(json!({"command": "execute", "code": "Message('a');", "test": "X"}));
    assert!(r["warnings"][0].as_str().unwrap().contains("runTests"), "{r}");
    // An app without an OnRun: nothing ran.
    let r = ask(json!({"command": "execute", "sourcePaths": [none]}));
    assert_eq!(r["tests"], json!([]), "{r}");
    assert_eq!(r["exitCode"], 6, "{r}");
    assert!(r["warnings"][0].as_str().unwrap().contains("OnRun"), "{r}");
    s.send(json!({"command": "shutdown"}));
    s.line();
}

#[test]
fn execute_and_runtests_share_the_server() {
    let d = tempfile::tempdir().unwrap();
    let a = app(d.path(), "mixed", MIXED);
    let mut s = started();
    s.send(json!({"command": "execute", "code": "Message('x');"}));
    assert_eq!(s.line()["exitCode"], 0);
    s.send(json!({"command": "runTests", "sourcePaths": [a]}));
    let lines = s.until_summary();
    assert_eq!(lines.last().unwrap()["total"], 2, "{lines:?}");
    s.send(json!({"command": "execute", "code": "Message('y');"}));
    assert_eq!(s.line()["tests"][0]["messages"], json!(["y"]));
    s.send(json!({"command": "shutdown"}));
    s.line();
}

#[test]
fn cancel_stops_an_execute_that_does_not_end() {
    let mut s = started();
    s.send(json!({"command": "execute", "code": "Message('start'); while true do ;", "announceStart": true}));
    // A cancel during the compile gives cancelled, exit code 6 and no error: also correct, but not what this test is
    // about. Wait until the code starts to run.
    let started = s.line();
    assert_eq!(started["type"], "started", "{started}");
    s.send(json!({"command": "cancel"}));
    // The acknowledgement comes at once, and then the answer of the stopped run.
    let ack = s.line();
    assert_eq!(ack, json!({"type": "ack", "command": "cancel", "noop": false}));
    let r = s.line();
    assert_eq!(r["cancelled"], true, "{r}");
    assert_eq!(r["tests"][0]["status"], "error", "{r}");
    assert!(r["tests"][0]["message"].as_str().unwrap().contains("cancelled"), "{r}");
    // The server goes on.
    s.send(json!({"command": "execute", "code": "Message('after');"}));
    assert_eq!(s.line()["tests"][0]["messages"], json!(["after"]));
    s.send(json!({"command": "shutdown"}));
    s.line();
}

// ---- captureValues and iterationTracking ----

#[test]
fn execute_returns_the_values_and_the_loops_as_v2_does() {
    let mut s = started();
    s.send(json!({
        "command": "execute", "captureValues": true, "iterationTracking": true, "coverage": true,
        "code": "codeunit 50102 X3 { trigger OnRun() var i: Integer; total: Integer; begin total := 0; for i := 1 to 3 do begin total := total + i; Message(Format(total)); end; end; }"
    }));
    let r = s.line();
    assert_eq!(r["exitCode"], 0, "{r}");
    let t = &r["tests"][0];
    assert_eq!(t["name"], "X3.OnRun");
    let cv = t["capturedValues"].as_array().unwrap();
    let shape: Vec<(String, i64, Option<i64>, Option<i64>)> = cv
        .iter()
        .map(|v| {
            (
                v["variableName"].as_str().unwrap().to_string(),
                v["value"].as_i64().unwrap(),
                v["loop"].as_i64(),
                v["iteration"].as_i64(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        vec![
            ("total".to_string(), 0, None, None),
            ("i".to_string(), 1, Some(0), Some(1)),
            ("total".to_string(), 1, Some(0), Some(1)),
            ("i".to_string(), 2, Some(0), Some(2)),
            ("total".to_string(), 3, Some(0), Some(2)),
            ("i".to_string(), 3, Some(0), Some(3)),
            ("total".to_string(), 6, Some(0), Some(3)),
        ]
    );
    // The loop: where it is, how many passes, how it ended, and the statements of each pass.
    let l = &t["loops"][0];
    assert_eq!(
        (l["id"].as_i64(), l["scope"].as_str(), l["iterationCount"].as_i64()),
        (Some(0), Some("OnRun"), Some(3))
    );
    assert_eq!(l["closedBy"], "exit");
    assert!(l["file"].as_str().unwrap().ends_with("/Inline.al"), "{l}");
    assert_eq!(l["iterations"].as_array().unwrap().len(), 3);
    assert_eq!(l["iterations"][0]["index"], 1);
    // The ids of statements are the ids of the coverage table (same scope).
    let ids: Vec<i64> = cv.iter().map(|v| v["statementId"].as_i64().unwrap()).collect();
    let covered: Vec<i64> = r["coverage"][0]["statements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|st| st["id"].as_i64().unwrap())
        .collect();
    assert!(ids.iter().all(|i| covered.contains(i)), "{ids:?} not in {covered:?}");
    // The tagged messages are the response's.
    let m = r["messages"].as_array().unwrap();
    assert_eq!(m.len(), 3, "{r}");
    assert_eq!(
        (m[1]["text"].as_str(), m[1]["loop"].as_i64(), m[1]["iteration"].as_i64()),
        (Some("3"), Some(0), Some(2))
    );
    // Without the flags there is nothing of it.
    s.send(json!({"command": "execute", "code": "Message('x');"}));
    let r = s.line();
    assert!(
        r["tests"][0].get("capturedValues").is_none() && r["tests"][0].get("loops").is_none(),
        "{r}"
    );
    assert!(r.get("messages").is_none(), "{r}");
    s.send(json!({"command": "shutdown"}));
    s.line();
}

const CAPTURE_TESTS: &str = r#"
codeunit 50300 "Capture Tests"
{
    Subtype = Test;

    [Test]
    procedure Loops()
    var
        i: Integer;
        Sum: Integer;
    begin
        for i := 1 to 2 do
            Sum += i;
    end;
}
"#;

#[test]
fn run_tests_puts_the_capture_on_the_test_line() {
    let d = tempfile::tempdir().unwrap();
    let a = app(d.path(), "cap", CAPTURE_TESTS);
    let mut s = started();
    s.send(json!({"command": "runTests", "sourcePaths": [a], "captureValues": true, "iterationTracking": true}));
    let lines = s.until_summary();
    let t = lines.iter().find(|l| l["type"] == "test").unwrap();
    assert_eq!(t["status"], "pass", "{t}");
    let values: Vec<(String, i64)> = t["capturedValues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| {
            (
                v["variableName"].as_str().unwrap().to_string(),
                v["value"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        values,
        vec![
            ("i".to_string(), 1),
            ("Sum".to_string(), 1),
            ("i".to_string(), 2),
            ("Sum".to_string(), 3)
        ]
    );
    assert_eq!(t["loops"][0]["iterationCount"], 2);
    // Another request without the flags: the lines carry nothing of it.
    s.send(json!({"command": "runTests", "sourcePaths": [a]}));
    let lines = s.until_summary();
    let t = lines.iter().find(|l| l["type"] == "test").unwrap();
    assert!(t.get("capturedValues").is_none() && t.get("loops").is_none(), "{t}");
    s.send(json!({"command": "shutdown"}));
    s.line();
}

#[test]
fn announce_start_writes_one_started_line_before_the_first_test() {
    let d = tempfile::tempdir().unwrap();
    let a = app(d.path(), "mixed", MIXED);
    let mut s = started();
    // Without the field no `started` line is written.
    s.send(json!({"command": "runTests", "sourcePaths": [a]}));
    let lines = s.until_summary();
    assert!(lines.iter().all(|l| l["type"] != "started"), "{lines:?}");
    s.send(json!({"command": "runTests", "sourcePaths": [a], "announceStart": true}));
    let lines = s.until_summary();
    let started: Vec<usize> = (0..lines.len()).filter(|&i| lines[i]["type"] == "started").collect();
    assert_eq!(started, vec![0], "{lines:?}");
    s.send(json!({"command": "shutdown"}));
    s.line();
}
