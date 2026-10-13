# ADR-004: Snapshots are a separate trait

- Status: Accepted
- Date: 2026-10-13

## Context
Per-test reset, rollback-on-error and `Codeunit.Run` transaction boundaries are
O(1) for a persistent (copy-on-write) in-memory store: a snapshot is a root
pointer. A SQL store cannot do that cheaply, and should not be forced to.

## Decision
`SnapshotStore: Store` is a separate trait with `snapshot() -> Snapshot` and
`restore(Snapshot)`. Only the in-memory store implements it. The VM's
transaction semantics are written against `Store` transactions; the test CLI
uses `SnapshotStore` for per-test isolation.

## Consequences
The VM never requires `SnapshotStore`. A SQL store can run the corpus, using
transactions for isolation, without implementing snapshots.

## Conformance
`al-data` contract suite has a separate `snapshot` section run only for
`SnapshotStore` implementations.
