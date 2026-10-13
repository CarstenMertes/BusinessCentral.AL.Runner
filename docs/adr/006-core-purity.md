# ADR-006: Core purity

- Status: Accepted
- Date: 2026-10-13

## Context
A runtime that can run in the browser (WASM) or be embedded in other tools is
worth more than one that assumes a server. Clocks, users, files and network
are also exactly the things tests need to control.

## Decision
- Core crates (`al-syntax`, `al-sema`, `al-builtins`, `al-ir`, `al-vm`,
  `al-data`) do no OS access: no `std::fs`, `std::net`, `std::process`,
  `std::env`, `SystemTime::now`, threads or randomness from the OS.
- Everything environmental goes through capability traits in `al-host`
  (UI, clock, identity, network, blob storage). `al-host` ships a deterministic
  test host.
- Only `al-test-cli` (and future front-ends) touch the OS.

## Consequences
`Today`, `CurrentDateTime`, `UserId`, `CreateGuid` etc. are host calls. Tests
are deterministic by default.

## Conformance
CI builds the core crates for `wasm32-unknown-unknown` (added when the first
core crate has code), and a lint test greps the core crates for forbidden
`std::` paths.
