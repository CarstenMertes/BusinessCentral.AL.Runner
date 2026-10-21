# bcdb in Rust: `al-bcdb`

`al-bcdb` reads Business Central data straight out of a SQL Server native backup (`.bak`) or
a cloud export (`.bacpac`): no SQL Server, no restore, no service tier. It is a port of
`bcdb` (BusinessCentral.BakReader, C#, MIT, the owner's tool). AL Runner v2 uses bcdb for
`--test-data`; this runtime uses the port for `al-runner test --test-data`, which restores the CRONUS demo
company into its store table by table, when a test first touches the table. ADR-020 says how it works;
`al-bcdb-std` is the part that opens the file and gives the runtime its rows.

## What is in the repository

| Path | What |
|---|---|
| `crates/al-bcdb` | The reader. Core crate: works on a `ByteSource`, no OS access (ADR-006), `Sync`, usable from WebAssembly |
| `crates/al-bcdb/PROVENANCE.md` | Where each format fact comes from, how the port was validated, how it differs from bcdb |
| `crates/al-bcdb/tests/fixtures` | bcdb's synthetic `typeprobe` database as `.bak` and `.bacpac`, and the `SELECT` output a real SQL Server gave for it |
| `crates/al-bcdb-std` | The OS side: the pread `FileSource` and `BackupProvider`, the `TableProvider` that `al-runner test --test-data` reads a backup through |
| `crates/al-bcdb-cli` | The `al-bcdb` command line (the only code that reads files), the parity tests, `examples/phases.rs` |
| `tools/bcdb-bench.sh` | The speed comparison against the published bcdb |

Module map of the core: `mtf` (backup container), `pagefile` (page map), `catalog` (system
tables), `records` (FixedVar and compressed records), `lob` (off-page values), `types` and
`scsu` (value decoding), `bak` (`.bak` source); `zip`, `model` (`model.xml`), `bcp` (native
BCP rows), `bacpac` (`.bacpac` source); `symbols` (AL names from `.app` files), `query`
(company and app parsing of table names, table resolution, `$ext` merge, describe), `serve`
(the JSON line protocol).

## Using it

```
cargo build --release -p al-bcdb-cli          # binary: target/release/al-bcdb
al-bcdb tables   <file>                        # tables, row counts, compression, company
al-bcdb companies <file>
al-bcdb describe <file> --table Customer --company CRONUS --symbols "<Base Application>.app"
al-bcdb read     <file> --table "G/L Entry" --company CRONUS --select "Entry No.,Amount" --top 5 --format json
al-bcdb serve    <file>                        # JSON requests on stdin, one reply line each
al-bcdb export   <file> --company CRONUS --out DIR --format tsv --jobs 8   # every table, in parallel (new)
al-bcdb check    <file.bak>                    # cross-check the page map
```

The options and the output are bcdb's (bcdb's `README.md` documents them). An option a
command does not accept fails the command. `restore` (which writes into a SQL Server) is not
ported.

Where real files come from: the BC artifact for a country holds the demo backup,
`~/.bcartifacts.cache/sandbox/<version>/<country>/BusinessCentral-<country>.bak` (CRONUS in
the W1 and US artifacts). No artifact holds a `.bacpac`. A CRONUS `.bacpac` can be made from
that backup by restoring it into a throwaway SQL Server container and running `sqlpackage
/Action:Export`. Nothing from Microsoft's files is committed here.

## How the port is checked

1. **Oracle fixtures.** The tests assert whole tables of bcdb's `typeprobe` database against
   `SELECT` output from a real SQL Server, for the `.bak` and the `.bacpac` of the same
   database (`cargo test -p al-bcdb`).
2. **Parity with the published bcdb.** `crates/al-bcdb-cli/tests/differential.rs` runs
   bcdb 0.2.1 (native AOT) and the port on the same input and compares stdout bytes, exit
   code and error text. The tests are `#[ignore]`d because they need the reference binary and
   the artifact files:

   ```
   cargo test --release -p al-bcdb-cli --test differential -- --ignored
   BCDB_BACPAC=<a .bacpac> cargo test --release -p al-bcdb-cli --test differential -- --ignored real_bacpac
   ```

   They cover every table of the fixtures in both formats, 30 option and refusal cases, every
   table with rows of the BC 24.5 (US), 27.5, 28.1 and 29.0 demo backups through `serve`
   (both companies), `describe` and merged reads with the shipped Base Application symbols,
   and every table of a CRONUS `.bacpac` exported by `sqlpackage`.
3. **The model scanner** is held to a tree-based reference by unit tests.

## Speed

`tools/bcdb-bench.sh` builds the port with `cargo --profile bcdb-bench` (opt-level 3, fat LTO,
one codegen unit, `panic=abort`, no `target-cpu`), then times both tools on the same file:
list tables, `describe`, read a small table, read the largest table, and a full export (every
table with rows of one company through `serve`, so both do the same work), warm or cold.
It prints the load average beside every row. Numbers taken while other builds run are not
worth quoting.

Where the time goes in the port, on the BC 28.1 demo backup (`cargo run --profile bcdb-bench
-p al-bcdb-cli --example phases -- <file.bak>`): the page map is 13 ms (one 40-byte read per
extent), the three catalog tables 6-8 ms, and finding one table's columns by descending the
clustered index 1.7 ms. For a `.bacpac`, `model.xml` (111 MB of XML) dominates: inflate,
SHA-256 over the same bytes, and the scan.

### Indicative numbers (2026-10-14, not a quiet machine)

AMD Ryzen 5 5600X, 12 threads, warm page cache, NVMe, minimum of 5 runs after one warm-up,
CRONUS from the BC 28.1 W1 demo backup (894 MB) and a 446 MB `.bacpac` that `sqlpackage` made
from it. Reference: bcdb 0.2.1, native AOT. The load average was 4-12 while these ran (other
agents build all day): the order of magnitude is reliable, single figures are not. CPU time
(user+sys) tracks wall time within a few percent in every row except the parallel export, where
it is 3x wall.

| Operation (wall, ms) | `.bak` bcdb | `.bak` port | `.bacpac` bcdb | `.bacpac` port |
|---|---:|---:|---:|---:|
| list tables | 65 | 30 | 1286 | 788 |
| `describe Customer` (Base Application symbols) | 636 | 256 | 1159 | 541 |
| read a small table (No. Series) | 63 | 24 | 792 | 311 |
| read the largest table by rows (Calendar Entry) | 221 | 54 | 904 | 326 |
| full export, 528 tables, through `serve` (same work for both) | 12,154 | 1,553 | 10,235 | 2,195 |
| full export, port only, `export --jobs 1` | | 867 | | 1,796 |
| full export, port only, `export` (12 threads) | | 331 | | 1,323 |

`tools/bcdb-bench.sh` produced these. The port's `.bak` listing improved from 88 ms to 30 ms after the first run (sort keys
built once), so the "list tables" row for the `.bak` mixes two runs. A second run, with the load
at 10-45, kept every ratio in the port's favour. Run the script yourself on a quiet machine
before quoting a number. Add `--cold` for a cold page cache; cold numbers were not taken.

For the smallest read the port's 24 ms is process start plus the open of the `.bak`: one 40-byte
read per extent for the page map (13 ms) and the catalog (6-8 ms). Opening a `.bacpac` costs the
`model.xml` scan (about 0.3 s of CPU for 111 MB).
