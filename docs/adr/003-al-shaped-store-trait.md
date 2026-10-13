# ADR-003: AL-shaped Store trait

- Status: Accepted
- Date: 2026-10-13

## Context
The data layer must eventually be backed by something other than memory
(SQLite, Postgres). If the trait is shaped after the in-memory map, every later
store would have to emulate it. AL's data access is cursor-based: `SetCurrentKey`
+ filters + `Find('-')`/`Next`/`Find('=<>')`.

## Decision
The core `Store` trait exposes:
- **Cursors**: seek by key range and direction (ascending/descending), returning
  rows in key order, with the predicate tree from SetFilter pushed down.
- **Typed outcomes** for writes: `Inserted`, `AlreadyExists`, `Modified`,
  `NotFound`, `Deleted`, `Renamed`, etc. — not booleans.
- **Transaction handles**: begin, commit, rollback.
- An error enum that includes `Conflict`, `Deadlock` and `ChangedByAnotherUser`
  even though the in-memory store never returns them.

Field values, filters and keys are passed in the store's own value types, not
the VM's, so `al-data` does not depend on `al-vm`.

## Consequences
- The in-memory store is not allowed to expose "give me the whole table".
- Error paths for concurrency exist in the VM from day one and are covered by a
  fault-injecting test store.

## Conformance
`al-data` contract suite (`al_data::contract::run_all::<S: Store>()`), run by
every store implementation's own tests.
