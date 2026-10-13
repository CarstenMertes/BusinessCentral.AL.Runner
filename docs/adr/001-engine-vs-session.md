# ADR-001: Engine vs Session

- Status: Accepted
- Date: 2026-10-13

## Context
A compiled AL program is shared by every session that runs it, while AL has
plenty of per-session mutable state: global variables of codeunits,
`SingleInstance` codeunits, manual event bindings (`BindSubscription`),
`LastErrorText`, collected errors, the transaction. Mixing the two makes
per-test isolation and parallel test execution hard.

## Decision
- `Engine` holds the compiled, linked program and is immutable after build. It is
  shared as `Arc<Engine>`.
- `Session` holds all mutable state: globals, single-instance codeunit instances,
  event bindings, error state, the active transaction and the store handle.
- Nothing reachable from `Engine` has interior mutability (no `Cell`, `RefCell`,
  `Mutex`, atomics, or lazy statics used as caches that change behaviour).

## Consequences
- Running each corpus codeunit in a fresh `Session` against one `Engine` gives
  isolation for free and lets the CLI run codeunits in parallel threads.
- Caches that would like to live on the engine (e.g. resolved method tables)
  are built at link time, not lazily.

## Conformance
`al-vm` contract test: `Engine: Send + Sync` (static assertion) and two sessions
over one engine never observe each other's globals.
