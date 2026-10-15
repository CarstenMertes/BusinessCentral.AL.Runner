//! `alrunner serve` (ADR-028): the v2 server protocol on stdin and stdout, in this process. The compile of the
//! last request is kept (`WarmCtx`), a test line is written as each test finishes, and `cancel` stops the run after the
//! current test.

use crate::args::{Command, TestArgs};
use crate::run::{self, WarmCtx};
use serde_json::{Map, Value, json};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

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

enum Input {
    Line(String),
    Eof,
}

/// A run is in progress, and a `cancel` or the end of input asks it to stop after the current test.
#[derive(Default)]
struct Flags {
    running: AtomicBool,
    cancel: Arc<AtomicBool>,
}

/// Reads stdin on a thread of its own: `cancel` is answered here, at once, also while a run keeps the main thread busy;
/// everything else is queued for the main loop.
fn reader(flags: Arc<Flags>) -> mpsc::Receiver<Input> {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let stdin = std::io::stdin();
        for line in stdin.lock().lines() {
            let Ok(line) = line else { break };
            if line.trim().is_empty() {
                continue;
            }
            let command = serde_json::from_str::<Value>(&line)
                .ok()
                .and_then(|v| v["command"].as_str().map(str::to_ascii_lowercase));
            match command.as_deref() {
                Some("cancel") => {
                    let running = flags.running.load(Ordering::SeqCst);
                    if running {
                        flags.cancel.store(true, Ordering::SeqCst);
                    }
                    out(&json!({ "type": "ack", "command": "cancel", "noop": !running }));
                    continue;
                }
                // The run in progress stops, then the loop sees the request.
                Some("shutdown") => flags.cancel.store(true, Ordering::SeqCst),
                _ => {}
            }
            if tx.send(Input::Line(line)).is_err() {
                return;
            }
        }
        flags.cancel.store(true, Ordering::SeqCst);
        let _ = tx.send(Input::Eof);
    });
    rx
}

pub fn serve(daemon: &[String]) -> Result<(), String> {
    let base = daemon_args(daemon)?;
    let flags = Arc::new(Flags::default());
    let rx = reader(flags.clone());
    let mut ctx = WarmCtx {
        cancel: Some(flags.cancel.clone()),
        on_test: Some(Arc::new(|e: al_testrun::events::TestEvent| {
            let mut row =
                crate::output::json_row(e.codeunit, &e.codeunit_name, &e.test, &e.outcome, &e.call_stack, e.time);
            out(&Value::Object(test_line(&mut row)));
        })),
        ..WarmCtx::default()
    };
    crate::run::keep_backups();
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
        match command.as_str() {
            "shutdown" => {
                out(&json!({ "status": "shutting down" }));
                eprintln!("[server] request {} done", n + 1);
                return Ok(());
            }
            "runtests" => run_tests(&base, fields, &mut ctx, &flags),
            "execute" => error("execute is not implemented yet (ADR-028, step 4)"),
            "" => error("a request needs a `command`"),
            other => error(format!("unknown command `{other}`")),
        }
        n += 1;
        eprintln!("[server] request {n} done");
    }
    Ok(())
}

/// The `alrunner test` options of the daemon, as the template of every request's.
fn daemon_args(daemon: &[String]) -> Result<TestArgs, String> {
    let mut probe = vec!["test".to_string(), "APP".to_string()];
    probe.extend(daemon.iter().cloned());
    match crate::args::parse(probe)? {
        Command::Test(t) => Ok(*t),
        _ => Err("alrunner serve: options of `alrunner test` only".into()),
    }
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

/// The `alrunner test` arguments for a request, and the warnings the request earns.
fn request_args(base: &TestArgs, fields: &Map<String, Value>) -> Result<(TestArgs, Vec<String>), String> {
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
    let mut a = base.clone();
    a.apps = sources.into_iter().map(PathBuf::from).collect();
    a.package_caches
        .extend(string_list(fields, "packagePaths")?.into_iter().map(PathBuf::from));
    if let Some(t) = fields.get("test").and_then(Value::as_str).filter(|t| !t.is_empty()) {
        a.filters.push(t.into());
    }
    a.excludes.extend(string_list(fields, "excludeTests")?);
    if let Some(i) = fields.get("testIsolation").and_then(Value::as_str) {
        a.isolation = Some(match i.to_ascii_lowercase().as_str() {
            "codeunit" => al_testrun::isolation::TestIsolation::Codeunit,
            "test" => al_testrun::isolation::TestIsolation::Function,
            "disabled" => al_testrun::isolation::TestIsolation::Disabled,
            _ => return Err(format!("`testIsolation` is codeunit, test or disabled, not `{i}`")),
        });
    }
    a.coverage = flag(fields, "coverage")? || flag(fields, "perTestCoverage")?;
    a.quiet = true;
    Ok((a, warnings))
}

fn run_tests(base: &TestArgs, fields: &Map<String, Value>, ctx: &mut WarmCtx, flags: &Flags) {
    let (args, warnings) = match request_args(base, fields) {
        Ok(x) => x,
        Err(e) => return error(e),
    };
    let (want_coverage, want_per_test) = (
        flag(fields, "coverage").unwrap_or(false) || flag(fields, "perTestCoverage").unwrap_or(false),
        flag(fields, "perTestCoverage").unwrap_or(false),
    );
    // A cancel that arrived while nothing ran was answered as a no-op: it does not stop the next run.
    flags.cancel.store(false, Ordering::SeqCst);
    flags.running.store(true, Ordering::SeqCst);
    ctx.report = None;
    ctx.used = None;
    let started = std::time::Instant::now();
    // A panic anywhere in a request ends the request, not the server; what is kept is dropped, as it may be inconsistent.
    let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| run::test_with(&args, Some(ctx))));
    flags.running.store(false, Ordering::SeqCst);
    let cancelled = flags.cancel.swap(false, Ordering::SeqCst);
    match outcome {
        Err(p) => {
            ctx.runtime.clear();
            let msg = p
                .downcast_ref::<String>()
                .cloned()
                .or_else(|| p.downcast_ref::<&str>().map(|s| s.to_string()))
                .unwrap_or_default();
            error(format!("the run panicked (a runtime bug): {msg}"));
        }
        Ok(Err(e)) => error(e),
        Ok(Ok(code)) => {
            let Some(report) = ctx.report.take() else {
                return error("the run produced no report");
            };
            let doc = &report.doc;
            let mut summary = json!({
                "type": "summary",
                "exitCode": code,
                "passed": doc["passed"], "failed": doc["failed"], "errors": doc["errors"],
                "notRun": doc["notRun"], "total": doc["total"],
                "cached": ctx.used == Some(al_testrun::runtime::WarmUse::Reused),
                "protocolVersion": 2,
                "wallSeconds": started.elapsed().as_secs_f64(),
            });
            if cancelled {
                summary["cancelled"] = json!(true);
            }
            if !warnings.is_empty() {
                summary["warnings"] = json!(warnings);
            }
            if let Some(cov) = &report.coverage {
                if want_coverage {
                    summary["coverage"] = cov["coverage"].clone();
                }
                if want_per_test {
                    summary["perTestCoverage"] = cov["perTestCoverage"].clone();
                }
            }
            out(&summary);
        }
    }
}

/// A `test` line of v2: the row of the JSON document, with `stackFrames` split out of the stack text.
fn test_line(row: &mut Map<String, Value>) -> Map<String, Value> {
    let mut line = Map::new();
    line.insert("type".into(), json!("test"));
    for key in ["name", "status", "durationMs", "message", "stackTrace", "notRun"] {
        if let Some(v) = row.remove(key) {
            line.insert(key.into(), v);
        }
    }
    if let Some(stack) = line.get("stackTrace").and_then(Value::as_str) {
        let frames: Vec<&str> = stack.lines().collect();
        line.insert("stackFrames".into(), json!(frames));
    }
    line
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    fn base() -> TestArgs {
        daemon_args(&["--define".into(), "S".into()]).unwrap()
    }

    #[test]
    fn a_run_request_becomes_the_arguments_of_a_test_run() {
        let f = fields(json!({
            "command": "runTests", "sourcePaths": ["/a", "/b"], "packagePaths": ["/p"], "test": "Codeunit1.X",
            "excludeTests": ["Codeunit1.Y"], "testIsolation": "test", "coverage": true,
        }));
        let (a, w) = request_args(&base(), &f).unwrap();
        assert_eq!(a.apps, [PathBuf::from("/a"), PathBuf::from("/b")]);
        assert_eq!(a.package_caches, [PathBuf::from("/p")]);
        assert_eq!(a.defines, ["S"]);
        assert_eq!(
            (a.filters.as_slice(), a.excludes.as_slice()),
            (&["Codeunit1.X".to_string()][..], &["Codeunit1.Y".to_string()][..])
        );
        assert_eq!(a.isolation, Some(al_testrun::isolation::TestIsolation::Function));
        assert!(a.coverage && a.quiet);
        assert!(w.is_empty());
    }

    #[test]
    fn fields_are_checked_deferred_ones_refused_and_the_other_commands_warned_about() {
        let run = |v: Value| request_args(&base(), &fields(v));
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
        let mut row = crate::output::json_row(
            1,
            "T",
            "X",
            &al_testrun::Outcome::Fail("m".into()),
            "a\nb",
            std::time::Duration::from_millis(3),
        );
        let l = test_line(&mut row);
        assert_eq!(l["type"], "test");
        assert_eq!(l["name"], "Codeunit1.X");
        assert_eq!(l["stackFrames"], json!(["a", "b"]));
        assert_eq!(l["durationMs"], 3);
    }
}
