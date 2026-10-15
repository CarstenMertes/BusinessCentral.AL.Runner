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
        let mut child = Command::new(env!("CARGO_BIN_EXE_alrunner"))
            .arg("serve")
            .args(extra)
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
    // A second request on the same server: selection and exclusion.
    s.send(json!({"command": "runTests", "sourcePaths": [a], "test": "Codeunit50100.Passes", "excludeTests": ["Codeunit50100.Fails"]}));
    let lines = s.until_summary();
    assert_eq!(lines.iter().filter(|l| l["type"] == "test").count(), 1, "{lines:?}");
    assert_eq!(lines.last().unwrap()["exitCode"], 0);
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
            "no result",
        ),
        (json!({"command": "execute", "code": "x"}), "not implemented"),
    ] {
        s.send(request);
        let l = s.line();
        assert!(l["error"].as_str().is_some_and(|e| e.contains(needle)), "{needle}: {l}");
    }
    // cancel with nothing running is a no-op that says so.
    s.send(json!({"command": "cancel"}));
    assert_eq!(s.line(), json!({"type": "ack", "command": "cancel", "noop": true}));
    // A field of another command only earns a warning.
    s.send(json!({"command": "runTests", "sourcePaths": [a], "captureValues": true, "test": "Passes"}));
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
}
"#;

#[test]
fn cancel_stops_a_run_and_the_summary_says_so() {
    let d = tempfile::tempdir().unwrap();
    let a = app(d.path(), "slow", SLOW);
    let mut s = Server::start(&["--no-download", "--no-test-data"]);
    s.line();
    s.send(json!({"command": "runTests", "sourcePaths": [a]}));
    std::thread::sleep(std::time::Duration::from_millis(1500));
    s.send(json!({"command": "cancel"}));
    assert_eq!(s.line(), json!({"type": "ack", "command": "cancel", "noop": false}));
    let summary = s.line();
    assert_eq!(
        (summary["type"].as_str(), summary["cancelled"].as_bool()),
        (Some("summary"), Some(true)),
        "{summary}"
    );
    // The server still answers.
    s.send(json!({"command": "cancel"}));
    assert_eq!(s.line()["noop"], true);
    s.send(json!({"command": "shutdown"}));
    assert_eq!(s.line(), json!({"status": "shutting down"}));
}
