# ADR-028: `alrunner serve`: the v2 server protocol on a child process per request

- Status: Proposed
- Date: 2026-10-15

## Context

ADR-016 (owner decision 2026-10-15) puts server mode in scope: v3 replaces v2, and ALchemist and
LethAL start `al-runner --server` and speak JSON lines on stdin and stdout
(`docs/integrations/alchemist-lethal.md`, section 1.1, 3.2). Step 3 of the plan is `alrunner serve`
with `runTests` and `shutdown`. Steps 4 and 5 add `execute`, value capture and warm state.

Two facts shape the design. First, a request recompiles anyway: v3 keeps no warm compiled state
(the compile cache of ADR-022 is a file cache, and LethAL needs a fresh answer after every source
change: it measured a stale verdict on a warm v2 process). Second, `alrunner test` already is the
whole run: loading, dependencies, compile, run, selection, isolation, timeouts, JSON and coverage
output (steps 1 and 2).

## Decision

1. **The wire is protocol-v2**, unchanged: first stdout line `{"ready":true}`; one JSON object per
   line in and out; stdout carries the protocol only, logs go to stderr; command names are case
   insensitive; `runTests` streams `{"type":"test",...}` lines and ends with one
   `{"type":"summary",... "protocolVersion":2}`; `shutdown` answers `{"status":"shutting down"}` and
   exits 0, EOF on stdin exits too; a bad request answers `{"error":"..."}` and the server goes on;
   after every request except `cancel` it writes `[server] request <n> done` to stderr.
2. **Each `runTests` request runs `alrunner test` in a child process** (this executable, with the
   daemon's options and the request's), reads the JSON the child wrote (`--output-json FILE`,
   `--coverage-json FILE`) and translates it into the `test` and `summary` lines. A request is
   isolated from the server and from the next request by construction: a panic, a hang or a leak
   cannot reach the daemon, and nothing is warm, so nothing can be stale.
3. **Daemon options** are the `alrunner test` options that do not depend on a request:
   `--package-cache`, `--dep`, `--define`, `--preprocessor-symbols`, `--test-timeout`,
   `--test-timeout-secs`, `--bc-version`, `--cache-dir`, `--no-download`, `--no-test-data`,
   `--test-data`, `--allow-not-run`, `--protocol 2` (the only value). They are checked at start-up
   with the `alrunner test` parser, and passed on to every child.
4. **Request fields.** `sourcePaths` (required: the app folders), `packagePaths` (each becomes a
   `--package-cache`), `coverage`, `perTestCoverage`, `test` (a `--test`), `excludeTests` (one
   `--exclude-test` each), `testIsolation` (`codeunit`, `test`, `disabled`). A field the command
   knows and v3 defers (`affectedOnly`, `includeFailing`, `strictEnvironment`, `tdd`) answers
   `{"error":...}` naming the deferral. A field v2 declares for another command (`code`,
   `captureValues`, `iterationTracking`) runs with a `warnings` entry in the summary. Any other
   field answers `{"error":"unknown field ..."}`.
5. **Translation.** `status` is `pass`, `fail` or `error` (a test that did not run is `error`, as
   in `--output-json`, with `message` `NotRun: out-of-scope: <reason>`); `stackFrames` is the AL call
   stack split by line, `stackTrace` the same text; `errorKind` is not sent. The summary has
   `exitCode` (the child's), `passed`, `failed`, `errors`, `total`, `cached` (always false),
   `protocolVersion` 2, `wallSeconds`, plus v3's `notRun`. `coverage` and `perTestCoverage` are the
   arrays of `--coverage-json`.
6. **Test lines come when the run ends.** The runtime reports a codeunit's results when the run
   finishes, so the lines are written together. A client that shows progress sees it at the end.
   (Streaming per codeunit needs a callback in `al-testrun`; it is a later step with warm state.)
7. **`cancel`** answers at once, also during a run: `{"type":"ack","command":"cancel","noop":bool}`.
   During a run it kills the child, and the summary has `cancelled:true`, `exitCode` 130 and no
   test lines (v2 finished the running test and reported those before it; v3 loses them).
8. **A child that cannot start** (exit 2: a bad path, an app that does not load) answers
   `{"error":"<first lines of the child's stderr>"}`; a child that dies by a signal answers an error too.
9. **Not in this step:** `execute` (step 4), value capture, warm state and per-codeunit streaming.
   `execute` answers `{"error":"execute is not implemented yet"}`.

## Consequences

- Smallest possible server: the new code is the wire (about 300 lines), no change to the run.
  Every improvement of `alrunner test` reaches `serve` the same day.
- Cost per request is a process start (milliseconds) plus the run. The compile cache keeps its
  value across requests because it is on disk.
- No warm state means no speed-up for editor loops; the owner's idea of a warm `serve`
  (`docs/ideas.md`) needs an in-process run. This ADR does not forbid it: the wire stays, the child
  becomes a thread.
- Cancel loses the partial results of the cancelled request.

## Conformance

`crates/alrunner-cli/tests/serve.rs` starts the daemon and checks the ready line, a run with test
and summary lines, field handling, `cancel`, `shutdown`, EOF, and that stdout holds protocol only.
