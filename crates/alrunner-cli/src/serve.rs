//! `alrunner serve` (ADR-028): the v2 server protocol on stdin and stdout. Each `runTests` request is one
//! `alrunner test` child process; the child's JSON becomes the `test` and `summary` lines.

use serde_json::{Map, Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

/// The request fields of `runTests` and what each becomes.
const RUN_TESTS_FIELDS: &[&str] = &[
    "command",
    "sourcePaths",
    "packagePaths",
    "coverage",
    "perTestCoverage",
    "test",
    "excludeTests",
    "testIsolation",
];
/// Fields v2 knows and v3 defers (ADR-016).
const DEFERRED: &[&str] = &["affectedOnly", "includeFailing", "strictEnvironment", "tdd"];
/// Fields v2 reads for another command only: accepted, with a warning.
const OTHER_COMMAND: &[&str] = &["code", "captureValues", "iterationTracking"];

fn out(v: &Value) {
    let mut o = std::io::stdout().lock();
    let _ = writeln!(o, "{v}");
    let _ = o.flush();
}

fn error(message: impl Into<String>) {
    out(&json!({ "error": message.into() }));
}

/// A line of stdin, or the end of it.
enum Input {
    Line(String),
    Eof,
}

fn reader() -> mpsc::Receiver<Input> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            match line {
                Ok(l) if l.trim().is_empty() => {}
                Ok(l) => {
                    if tx.send(Input::Line(l)).is_err() {
                        return;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = tx.send(Input::Eof);
    });
    rx
}

/// What the server does after a request.
enum Next {
    Go,
    Stop,
}

pub fn serve(daemon: &[String]) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| format!("cannot find this executable: {e}"))?;
    let rx = reader();
    out(&json!({ "ready": true }));
    let mut n = 0u64;
    while let Ok(Input::Line(line)) = rx.recv() {
        let request: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                error(format!("not a JSON object: {e}"));
                continue;
            }
        };
        let Some(fields) = request.as_object() else {
            error("a request is a JSON object");
            continue;
        };
        let command = fields
            .get("command")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_ascii_lowercase();
        let next = match command.as_str() {
            "shutdown" => {
                out(&json!({ "status": "shutting down" }));
                Next::Stop
            }
            "cancel" => {
                out(&json!({ "type": "ack", "command": "cancel", "noop": true }));
                continue;
            }
            "runtests" => {
                run_tests(&exe, daemon, fields, &rx);
                Next::Go
            }
            "execute" => {
                error("execute is not implemented yet (ADR-028, step 4)");
                Next::Go
            }
            "" => {
                error("a request needs a `command`");
                Next::Go
            }
            other => {
                error(format!("unknown command `{other}`"));
                Next::Go
            }
        };
        n += 1;
        eprintln!("[server] request {n} done");
        if matches!(next, Next::Stop) {
            return Ok(());
        }
    }
    Ok(())
}

fn string_list(fields: &Map<String, Value>, name: &str) -> Result<Vec<String>, String> {
    match fields.get(name) {
        None | Some(Value::Null) => Ok(Vec::new()),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                v.as_str()
                    .map(String::from)
                    .ok_or_else(|| format!("`{name}` holds only strings"))
            })
            .collect(),
        Some(_) => Err(format!("`{name}` is a list of strings")),
    }
}

fn flag(fields: &Map<String, Value>, name: &str) -> Result<bool, String> {
    match fields.get(name) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(format!("`{name}` is true or false")),
    }
}

/// The child's command line for a request, and the warnings the request earns.
fn child_args(
    daemon: &[String],
    fields: &Map<String, Value>,
    json_file: &std::path::Path,
    coverage_file: &std::path::Path,
) -> Result<(Vec<String>, Vec<String>), String> {
    let mut warnings = Vec::new();
    for key in fields.keys() {
        if DEFERRED.contains(&key.as_str()) {
            return Err(format!(
                "`{key}` is deferred in v3 (ADR-016): it only hides run cost and waits until it is measured to matter"
            ));
        }
        if OTHER_COMMAND.contains(&key.as_str()) {
            warnings.push(format!("`{key}` is read by `execute` only; `runTests` ignores it"));
        } else if !RUN_TESTS_FIELDS.contains(&key.as_str()) {
            return Err(format!("unknown field `{key}` for runTests"));
        }
    }
    let sources = string_list(fields, "sourcePaths")?;
    if sources.is_empty() {
        return Err("`sourcePaths` needs at least one app folder".into());
    }
    let mut args: Vec<String> = vec!["test".into()];
    args.extend(sources);
    for p in string_list(fields, "packagePaths")? {
        args.push("--package-cache".into());
        args.push(p);
    }
    args.extend(daemon.iter().cloned());
    if let Some(t) = fields.get("test").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        args.push("--test".into());
        args.push(t.into());
    }
    for x in string_list(fields, "excludeTests")? {
        args.push("--exclude-test".into());
        args.push(x);
    }
    if let Some(i) = fields.get("testIsolation").and_then(Value::as_str) {
        if !["codeunit", "test", "disabled"].contains(&i.to_ascii_lowercase().as_str()) {
            return Err(format!("`testIsolation` is codeunit, test or disabled, not `{i}`"));
        }
        args.push("--isolation".into());
        args.push(i.into());
    }
    args.push("--output-json".into());
    args.push(json_file.display().to_string());
    if flag(fields, "coverage")? || flag(fields, "perTestCoverage")? {
        args.push("--coverage-json".into());
        args.push(coverage_file.display().to_string());
    }
    Ok((args, warnings))
}

fn run_tests(exe: &PathBuf, daemon: &[String], fields: &Map<String, Value>, rx: &mpsc::Receiver<Input>) {
    static RUNS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let id = RUNS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("alrunner-serve-{}-{id}", std::process::id()));
    if let Err(e) = std::fs::create_dir_all(&dir) {
        return error(format!("cannot make {}: {e}", dir.display()));
    }
    let (json_file, coverage_file) = (dir.join("run.json"), dir.join("coverage.json"));
    let result = (|| -> Result<(), String> {
        let (args, warnings) = child_args(daemon, fields, &json_file, &coverage_file)?;
        let started = Instant::now();
        let mut child = Command::new(exe)
            .args(&args)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("cannot start {}: {e}", exe.display()))?;
        // The child's stderr is the server's own log; the tail is kept for an error answer.
        let stderr = child.stderr.take().expect("stderr piped");
        let tail = std::thread::spawn(move || {
            let mut lines = Vec::new();
            for l in std::io::BufReader::new(stderr).lines().map_while(Result::ok) {
                eprintln!("{l}");
                lines.push(l);
                if lines.len() > 20 {
                    lines.remove(0);
                }
            }
            lines
        });
        let mut cancelled = false;
        let mut stop_after = false;
        let status = loop {
            if let Some(s) = child.try_wait().map_err(|e| e.to_string())? {
                break s;
            }
            match rx.recv_timeout(Duration::from_millis(40)) {
                Ok(Input::Line(l)) => match serde_json::from_str::<Value>(&l)
                    .ok()
                    .and_then(|v| v["command"].as_str().map(str::to_ascii_lowercase))
                    .as_deref()
                {
                    Some("cancel") => {
                        out(&json!({ "type": "ack", "command": "cancel", "noop": cancelled }));
                        if !cancelled {
                            cancelled = true;
                            let _ = child.kill();
                        }
                    }
                    Some("shutdown") => {
                        cancelled = true;
                        stop_after = true;
                        let _ = child.kill();
                    }
                    _ => error("a run is in progress: only `cancel` and `shutdown` are answered until it ends"),
                },
                Ok(Input::Eof) | Err(mpsc::RecvTimeoutError::Disconnected) => {
                    cancelled = true;
                    stop_after = true;
                    let _ = child.kill();
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
        };
        let tail = tail.join().unwrap_or_default();
        let code = status.code().unwrap_or(-1);
        if cancelled {
            out(&json!({
                "type": "summary", "exitCode": 130, "passed": 0, "failed": 0, "errors": 0, "total": 0,
                "cached": false, "protocolVersion": 2, "cancelled": true,
                "wallSeconds": started.elapsed().as_secs_f64(),
            }));
            if stop_after {
                out(&json!({ "status": "shutting down" }));
                std::process::exit(0);
            }
            return Ok(());
        }
        // The child makes its output file before it starts to work, so an empty one means it stopped early.
        let text = std::fs::read_to_string(&json_file)
            .ok()
            .filter(|t| !t.trim().is_empty())
            .ok_or_else(|| {
                format!(
                    "the run produced no result (exit {code}): {}",
                    tail.iter().rev().take(3).rev().cloned().collect::<Vec<_>>().join(" | ")
                )
            })?;
        let doc: Value = serde_json::from_str(&text).map_err(|e| format!("the run's JSON is unreadable: {e}"))?;
        for t in doc["tests"].as_array().into_iter().flatten() {
            out(&test_line(t));
        }
        let mut summary = json!({
            "type": "summary",
            "exitCode": code,
            "passed": doc["passed"], "failed": doc["failed"], "errors": doc["errors"],
            "notRun": doc["notRun"], "total": doc["total"],
            "cached": false, "protocolVersion": 2,
            "wallSeconds": doc["wallSeconds"],
        });
        if !warnings.is_empty() {
            summary["warnings"] = json!(warnings);
        }
        if let Ok(cov) = std::fs::read_to_string(&coverage_file) {
            if let Ok(cov) = serde_json::from_str::<Value>(&cov) {
                if flag(fields, "coverage")? || flag(fields, "perTestCoverage")? {
                    summary["coverage"] = cov["coverage"].clone();
                }
                if flag(fields, "perTestCoverage")? {
                    summary["perTestCoverage"] = cov["perTestCoverage"].clone();
                }
            }
        }
        out(&summary);
        Ok(())
    })();
    let _ = std::fs::remove_dir_all(&dir);
    if let Err(e) = result {
        error(e);
    }
}

/// A `test` line of v2: the child's row, with `stackFrames` split out of the stack text.
fn test_line(t: &Value) -> Value {
    let mut line = Map::new();
    line.insert("type".into(), json!("test"));
    for key in ["name", "status", "durationMs", "message", "stackTrace", "notRun"] {
        if let Some(v) = t.get(key) {
            line.insert(key.into(), v.clone());
        }
    }
    if let Some(stack) = t["stackTrace"].as_str() {
        line.insert("stackFrames".into(), json!(stack.lines().collect::<Vec<_>>()));
    }
    Value::Object(line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn a_run_request_becomes_the_child_command_line() {
        let f = fields(json!({
            "command": "runTests", "sourcePaths": ["/a", "/b"], "packagePaths": ["/p"], "test": "Codeunit1.X",
            "excludeTests": ["Codeunit1.Y"], "testIsolation": "test", "coverage": true,
        }));
        let (a, w) = child_args(
            &["--define".into(), "S".into()],
            &f,
            "/t/run.json".as_ref(),
            "/t/c.json".as_ref(),
        )
        .unwrap();
        assert_eq!(
            a,
            [
                "test",
                "/a",
                "/b",
                "--package-cache",
                "/p",
                "--define",
                "S",
                "--test",
                "Codeunit1.X",
                "--exclude-test",
                "Codeunit1.Y",
                "--isolation",
                "test",
                "--output-json",
                "/t/run.json",
                "--coverage-json",
                "/t/c.json"
            ]
        );
        assert!(w.is_empty());
    }

    #[test]
    fn fields_are_checked_deferred_ones_refused_and_the_other_commands_warned_about() {
        let run = |v: Value| child_args(&[], &fields(v), "r".as_ref(), "c".as_ref());
        assert!(run(json!({"command": "runTests"})).unwrap_err().contains("sourcePaths"));
        assert!(
            run(json!({"sourcePaths": ["/a"], "tdd": true}))
                .unwrap_err()
                .contains("deferred")
        );
        assert!(
            run(json!({"sourcePaths": ["/a"], "frobnicate": 1}))
                .unwrap_err()
                .contains("unknown field")
        );
        assert!(run(json!({"sourcePaths": ["/a"], "testIsolation": "x"})).is_err());
        assert!(run(json!({"sourcePaths": "/a"})).is_err());
        let (_, w) = run(json!({"sourcePaths": ["/a"], "captureValues": true})).unwrap();
        assert_eq!(w.len(), 1);
    }

    #[test]
    fn a_test_line_carries_the_stack_as_frames() {
        let l = test_line(
            &json!({"name": "Codeunit1.X", "status": "fail", "message": "m", "stackTrace": "a\nb", "durationMs": 3}),
        );
        assert_eq!(l["type"], "test");
        assert_eq!(l["stackFrames"], json!(["a", "b"]));
        assert_eq!(l["durationMs"], 3);
    }
}
