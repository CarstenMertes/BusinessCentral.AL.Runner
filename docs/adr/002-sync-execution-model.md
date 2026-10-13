# ADR-002: Synchronous execution model

- Status: Accepted
- Date: 2026-10-13

## Context
AL is single-threaded per session. Background sessions and task scheduler
exist, but each runs as its own session. Async in the core would infect every
signature for no semantic gain.

## Decision
- The VM is synchronous. No `async` in core crates.
- One session per thread. Inside a session, shared mutable values (List,
  Dictionary, Json types, record handles) use `Rc` / `RefCell`.
- `Session` is therefore `!Send`; `Engine` is `Send + Sync` (ADR-001).
- `Error(...)` and `exit(...)` are control-flow signals carried in `Result`,
  never Rust panics. A Rust panic in the VM is a runtime bug.

## Consequences
- Host capabilities that are naturally async (HTTP) block inside the host
  implementation; the core never sees a future.
- Background sessions, if ever needed, are new sessions on new threads.

## Conformance
`al-vm` tests: `Error` inside nested calls unwinds to the nearest `asserterror`
/ `Codeunit.Run` boundary as a value; a test asserts no `catch_unwind` is used
for AL control flow.
