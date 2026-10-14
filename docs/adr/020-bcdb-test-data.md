# ADR-020: Test data from a BC database file: `al-bcdb` and a table provider

- Status: Accepted (owner, 2026-10-14: build the loader in point 3)
- Date: 2026-10-14

## Context

AL Runner v2 loads the CRONUS demo company with `--test-data`. It shells out to `bcdb`, the
owner's C# reader for SQL Server backups and BC cloud exports, and rebuilds each table's rows
into its store the first time a run touches the table (`TestDataProvisioner.cs`,
`RecordPatches.TestDataHydration.cs`). Microsoft's Base App test buckets need that data: on
Tests-SMB, switching `--test-data` on more than doubled the passing count
(`running-ms-test-buckets` skill).

The runtime needs the reader in-process, fast, and usable from WebAssembly. A subprocess per
table costs a process start and a page-map build each time, and cannot run in a browser.

## Decision

1. **`al-bcdb` is a core crate** (ADR-006): a port of bcdb 0.2.1 that works on a
   `ByteSource` (a trait with `read_at`/`slice`) and does no OS access. `.bak` and `.bacpac`
   both open through `al_bcdb::open`. Its output is typed (`Value`), and its text form is
   bcdb's text byte for byte.
2. **`al-bcdb-cli` holds the OS access**: a `pread` byte source and the bcdb command line
   (`tables`, `companies`, `describe`, `read`, `serve`, `check`, `validate`, `verify`), plus a
   parallel `export`. It is a development tool and the oracle for parity tests; it is not
   wired into test runs.
3. **The loader is a later step and is not built by this ADR.** It should work like this:
   - A `TableProvider` capability trait in `al-host` (additive): given a company and a table's
     SQL name, it streams typed rows. `al-test-cli` implements it over `al-bcdb`, as it
     implements `Clock` today. The core never sees a file.
   - **On first touch of a table** in a Session, the store asks the provider and fills the
     table. A table nothing touches is never read. This is AL Runner's measured lesson: eager
     loading put 37,710 rows into the install baseline and made every test boundary cost
     105-203 ms.
   - Rows read after the install baseline was captured are added to the baseline as well, so
     the per-test reset (a root-pointer swap, ADR-004) does not drop them.
   - **Names come from the compiled program, not from symbol files.** The program already
     knows each table's fields and extension fields, so a column `Name_` maps to field `Name.`
     by the same normalization (`SqlNames.Normalize`) without loading the 58 MB Base
     Application symbols. `$ext` companions are merged on the companion's own key, as
     `bcdb read --merge-extensions` does (`al_bcdb::query::plan_read_table`).
   - **Refusals are explicit**, one per AL Runner's declared exclusions: same-named tables no
     app id can select, a value type the runtime cannot rebuild, a table the reader fails on
     (reported with the reader's text, never fatal to the other tables), extension columns of
     an app outside the closure (dropped and counted), a column the AL table does not have
     (dropped and counted; refusing the table leaves it empty, which AL reads and believes),
     a row shape sharing no column with the target (refused). The summary line reports each
     count.
   - **The source is a cached Microsoft artifact outside the repository** (hard rule 7):
     `~/.bcartifacts.cache/sandbox/<version>/<country>/BusinessCentral-<country>.bak`. A
     `.bacpac` of the owner's own database is the same call with a different file.
4. **Dependencies added**: `sha2` (the `model.xml` checksum and `--sha256`), `flate2` with the
   `zlib-rs` backend (zip inflate; pure Rust, builds for WebAssembly, faster than
   `miniz_oxide`), `quick-xml`, `serde`, `serde_json` (already in the workspace) in the core;
   `rayon` in the command line only. No dependency touches the OS from the core.
5. **Fixtures**: bcdb's synthetic `typeprobe` database (8.2 MB: a `.bak`, a `.bacpac`, the
   oracle `SELECT` exports, two symbol files) is copied into `crates/al-bcdb/tests/fixtures`.
   It holds no Microsoft data. Fixtures of the demo database (`bc275-*`, `bc281-*`) are not
   copied.

## Consequences

- `--test-data` can be implemented without a .NET process and without a second program on the
  path. Nothing is built on the loader design in point 3 until it is accepted.
- The reader's memory use is bounded by one table's rows in flight; the page map (about 1 MB
  for the 28.1 demo backup) is the only whole-file state.
- Every behaviour of the port is held to the published bcdb by `crates/al-bcdb-cli/tests/differential.rs`
  (see `docs/bcdb.md`). A fix in bcdb needs the same fix here, and the parity tests show a gap.
- `bcdb restore` is not ported.

## Decided without owner review (2026-10-14)

- Two crates (`al-bcdb`, `al-bcdb-cli`) rather than a binary inside the core crate, so the core
  stays free of `std::fs` and threads.
- `pread` rather than a memory map in the command line: 17 ms against 37 ms to open the 28.1
  backup, because a map pays a page fault per touched extent.
- `tables` is sorted by an approximation of the culture-aware order bcdb uses; it matched bcdb on
  all four demo backups and a real `.bacpac`.
- `read` prints nothing when a row cannot be decoded, as bcdb does, by buffering the answer.
- The `typeprobe` fixtures are committed (8.2 MB of binary).

## Conformance

`cargo test -p al-bcdb -p al-bcdb-cli` (hermetic: oracle fixtures, framing bytes, serve and
command-line contracts). `cargo test -p al-bcdb-cli --test differential -- --ignored` compares
with the published bcdb on the fixtures, on every artifact `.bak` in the cache, and on a real
`.bacpac`. `tools/bcdb-bench.sh` measures speed.
