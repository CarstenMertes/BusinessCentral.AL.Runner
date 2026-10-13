# ADR-005: Company and access context on every data operation

- Status: Accepted
- Date: 2026-10-13

## Context
BC tables are per company unless `DataPerCompany = false`. `ChangeCompany`
and cross-company reads exist. Permissions (and their absence) are a large part
of BC behaviour. Retrofitting either into a store API later touches every call.

## Decision
- A table's identity in the store is `(company, table id)`; tables with
  `DataPerCompany = false` use a fixed shared company key.
- Every store operation takes an `AccessContext { user, company, permissions }`.
  The in-memory store ignores user and permissions for now.

## Consequences
Slightly noisier signatures from day one. `ChangeCompany` is a session-level
switch of the company in the context, not a special store path.

## Conformance
`al-data` contract test: the same key inserted under two companies is two rows;
a `DataPerCompany = false` table is shared.
