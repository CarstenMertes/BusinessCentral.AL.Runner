# Provenance of al-bcdb

`al-bcdb` is a Rust port of `bcdb` (BusinessCentral.BakReader / BusinessCentral.DbReader,
C#, MIT, same owner), version 0.2.1. The port adds no format knowledge of its own. Every
structural fact in it was established by bcdb and is recorded, with its evidence, in that
repository's `PROVENANCE.md`. This file records three things: where each fact lives there,
how the port was checked against it, and what the port does differently.

## Clean room

bcdb's rule carries over unchanged: no other MDF-, BAK- or database-file-reading project's
code was read, fetched or consulted, GPL or not. The port was written from bcdb's own source
and from the files. If something cannot be derived from Microsoft's documentation, `DBCC
PAGE`, the files themselves or the oracle, stop and record the gap. Do not look at someone
else's code. A new fact goes into this file with its evidence, in the style below.

## Where each fact comes from

| Rust source | bcdb source | bcdb `PROVENANCE.md` section |
|---|---|---|
| `mtf.rs` | `Mtf.cs` | Backup container (MTF) |
| `pagefile.rs` | `PageFile.cs` | Data-copy layout; File size is a lower bound; Why self-identification was wrong; PFS pages; Log region; Page header offsets |
| `catalog.rs` (tables) | `Catalog.cs` | Boot page; System catalog base tables; Physical rowset layout (sysrscols) |
| `catalog.rs` (descent) | `ClusteredSeek.cs` | Clustered index descent |
| `bak.rs` | `TableReader.cs`, `BakSource` | IAM pages; Heaps and empty slots |
| `records.rs` | `Records.cs` | Compressed (CD) records; Ghost records in compressed pages |
| `types.rs`, `scsu.rs` | `Values.cs`, `Scsu.cs` | Type encodings; Unicode compression is SCSU |
| `lob.rs` | `Lob.cs` | Off-row storage |
| `zip.rs`, `bacpac.rs`, `bcp.rs` | `Bacpac.cs`, `Bcp.cs` | The `.bacpac` container; bacpac: native BCP row framing |
| `symbols.rs` | `Symbols.cs` | AL meaning layer |
| `query.rs`, `serve.rs` | `Program.cs` | Extension companion join key; the request-surface issues |

## How the port was validated

Verify against an oracle, never against yourself (bcdb's rule). The oracle is a value that a
real SQL Server returned, or the published bcdb, which was validated against one.

1. **bcdb's own oracle fixtures.** `tests/fixtures/typeprobe.bak`, `typeprobe.bacpac` and
   the `typeprobe-*.tsv` files are copied unchanged from bcdb's `fixtures/`. They are a
   scratch database built by `tools/typeprobe.sql` in bcdb (synthetic values, no
   Microsoft data), its backup, a `sqlpackage` export of it, and `SELECT` output from a real
   SQL Server 2022. The port's tests assert full tables against them for both containers.
   The `bc275-*.tsv` and `bc281-*.tsv` fixtures hold rows of Microsoft's demo database and
   were not copied.
2. **Differential tests against the published bcdb 0.2.1 (native AOT).**
   `crates/al-bcdb-cli/tests/differential.rs`. Same command, same file: same stdout bytes,
   same exit code, and the same error text. Run on 2026-10-14:
   * every table of the typeprobe fixtures, tsv and json, with and without symbols; 30
     option and refusal cases; `verify`; `check`;
   * the BC 27.5, 28.1 and 29.0 W1 demo backups and the BC 24.5 US demo backup: `tables`
     output, and every table with rows through `serve` (all companies), reply by reply;
   * the BC 28.1 backup: `describe` and `read --merge-extensions` against the shipped Base
     Application symbols, seven tables, both companies, plus `tables --symbols`;
   * a real CRONUS `.bacpac` made with `sqlpackage` from the 28.1 demo backup (3,955
     tables, 565 with data): `tables`, `companies` and every table through `serve`.
3. **Mutation checks of the `.bak` tests.** The `.bak` code was written as a port before its
   tests (the tests are ports of bcdb's). To show the tests discriminate, three rules were
   broken one at a time and the suite had to go red: not skipping ghost compressed records
   (1 test fails), ignoring the PFS per-page allocation bit (15 fail), treating the internal
   change-tracking column as a user column (1 fails). Each was reverted.

## Where the port differs, and why

* **`tables` order.** bcdb sorts with the current culture (ICU). `query::collate_cmp`
  approximates that for BC identifiers (punctuation first in CLDR order, then digits, then
  letters with case ignored at the first level, lower case first at the third). It is exact
  for ASCII names and matched the published bcdb on all four demo backups and the bacpac. A
  non-ASCII name sorts by an approximation (Latin-1 accents fold to the base letter).
* **Mapped file versus pread.** The command line reads with `pread`, not a memory map. A
  page map touches one page per extent, and a map pays a page fault for each: opening the
  28.1 backup took 37 ms mapped and 17 ms with pread (measured with `examples/phases.rs`).
* **`read` is all or nothing**, as in bcdb: it decodes every row before printing the first,
  so a row that cannot be decoded leaves stdout empty. The port buffers the output to match.
* **serve `"merge-extensions": false`** merges in bcdb (the key is present). The port treats
  an explicit `false` as no merge. Nothing in AL Runner sends `false`.
* **Inflate** uses `flate2` with the `zlib-rs` backend. It is pure Rust, so it builds for
  WebAssembly, and it was measurably faster than `miniz_oxide` on the 463 MB of deflated
  data in a production-shaped `.bacpac`.
* **No prefetch.** bcdb's `--prefetch` and its parallel page warming exist because its page
  reads were serial and cold. The port accepts `--prefetch` and ignores it.
* **Typed values.** `Value` carries the decoded type (decimal as an exact integer and scale,
  dates as day counts, a GUID as 16 bytes). Its `Display` is bcdb's text. A store loader
  therefore never parses text.
* **Parallelism lives in the command line.** The core is `Sync` and uses no threads (ADR-006).
  `al-bcdb export` reads tables in parallel with rayon.

## Not ported

* `bcdb restore` (writes into a SQL Server; the port neither needs nor talks to one).
* Nothing else of the read surface is missing: `tables`, `companies`, `describe`, `read`
  (`--company`, `--app`, `--top`, `--select`, `--sha256`, `--merge-extensions`, `--format`),
  `serve`, `check`, `validate`, `verify` and `--version` are all there.

## Still not determined (refused loudly, as in bcdb)

`money`, `smallmoney`, `smalldatetime`, `datetimeoffset`, `sql_variant`, `xml` and sparse
columns; backups taken `WITH COMPRESSION` or encrypted; more than one data file; differential
and log backups; a `.bacpac` with a Data stream version other than 2.0.0.0 and chunked MAX
values in a bacpac stream. The transaction log of a `.bak` is not replayed.
