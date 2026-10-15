# Where Pageworks and Tests-ERM spend their time

Measured 2026-10-15 on `text-speed-2` merged with main 54d1bb72 (shared copy-on-write text, boxed `AlError`, Integer
fast paths, the workspace install run once). Profile step `profile-suites` of the rework plan. Read-only use of the
owner's Pageworks tree. The machine was shared (load average 28 to 37), so wall times are soft; the shares come from
`perf record -g` on a release build with frame pointers (`RUSTFLAGS="-C force-frame-pointers=yes -C debuginfo=1"`)
and are stable. `AL_PROFILE=<file>` (new, `al-vm/src/profile.rs`) gives the same view per AL routine.

## 1. What was slow, and what was fixed on the way

The first profile of the owner's run did not show interpretation at all. 99.8 % of 395 s of samples sat in two AL
routines, `DecodeBase64Bytes`, called from Pageworks' own install codeunit (`RegisterBaselineFonts`). The runner
re-ran the workspace app's install codeunits in every test codeunit's session (151 times). BC runs them once when it
publishes the app. Now they run once per run, with the baseline.

| Pageworks (1013 tests, 811 run) | wall | CPU |
|---|---|---|
| main before this work | 38 min 33 s | 198 min |
| shared text + append in place | 8 min 01 s | 42 min |
| install once, Integer fast paths, boxed errors | 36 to 70 s (load-dependent) | about 3 to 4 min |

Outcomes were identical in every step (0 differences, `tools/compare_junit.py`).

## 2. Per codeunit, ours against BC (owner's pipeline, `pageworks-bc-times.txt`)

Our time is the sum of the test times in the junit file for tests that ran. BC's is the owner's CI time for the whole
codeunit. Our numbers include only run tests (811 of 1013); BC's include all.

| codeunit | ours s | ran / all | BC s | ratio |
|---|---|---|---|---|
| PageworksQrEncoderTest | 8.02 | 10/10 | 0.41 | 19.7 |
| PageworksBaselineFontTest | 4.76 | 4/5 | 0.67 | 7.1 |
| PageworksQrRenderE2ETest | 3.62 | 8/8 | 0.32 | 11.4 |
| PageworksCorpusRegistryTest | 2.91 | 30/32 | 4.04 | 0.7 |
| PageworksImageTest | 2.14 | 35/38 | 4.04 | 0.5 |
| PageworksBarcodeAutoHookTest | 1.97 | 3/4 | 0.22 | 8.8 |
| PageworksFontSubsetTest | 1.15 | 10/10 | 0.24 | 4.8 |
| PageworksFontEmbedTest | 1.14 | 7/7 | 0.22 | 5.2 |
| PageworksFontUploadFlowTest | 1.10 | 10/10 | 0.85 | 1.3 |
| PageworksEdgeRegionBoxTest | 1.09 | 12/12 | 0.23 | 4.7 |
| total | 40.4 | 811 run | 88.2 | 0.46 |

In sum we are faster than BC's 88 s. Per codeunit, the pure computation ones (QR encoder, font parsing, subsetting)
are 5 to 20 times slower than BC's compiled code, the ones that mostly use records and reports are as fast or faster.
The EndToEnd, LayoutStudio and InvoiceGrade codeunits (BC 20.8, 8.0 and 5.8 s) mostly do not run here yet (report
properties, PDF rendering), so they are not comparable.

## 3. Shares of CPU time

Symbols grouped by what they do (self time, all threads). "Other" in Tests-ERM is mostly unresolved libc addresses,
which are malloc, free and memcpy; they belong to allocation.

| | Pageworks | Tests-ERM |
|---|---|---|
| dispatch: eval, exec_block, call, place read and write, defaults | **45 %** | 7 % |
| Value and Decimal arithmetic, conversion, comparison | 9 % | 4 % |
| text (Txt, search, collation) | 4.5 % | 3 % |
| builtin lookup (hash probe) | 3.7 % | 0.6 % |
| allocation and drop (malloc, free, Value clone and drop, libc) | 12 % | **31 % + 36 % in libc = about 65 %** |
| record store, filters, table access | 0.2 % | 4 % |
| compile and load | 8 % (a one-time cost per run) | 2 % |

Pageworks is an interpreter benchmark: `eval` alone is 23 % self, `exec_block` 10 %, `convert` 4.6 %, `write` 4 %.
The remaining Pageworks time (the 40 s run) is pure AL computation (QR masks, base64, font tables).

Tests-ERM is not. The inclusive profile shows where the allocation comes from:

| inclusive, Tests-ERM | share |
|---|---|
| `Session::record_op` | 77 % |
| `ir::Table::clone` (a whole table schema copied: every field, every string) | **23 %** |
| `drop` of those copies | 17 % |
| `validate_hooks` / `validate_relation` | 38 % / 17 % |
| `default_value` + `initial_fields` (a new record variable) | 10 % / 9 % |
| `MemStore::next` (scans) | 9 % |

The two clones are `relations.rs:147` (`validate_relation` copies the table schema on every Validate of a field
with a relation) and `records.rs:1871`. About 40 % of a Microsoft suite run is copying and freeing schemas. That is
a two-line fix (borrow through the engine's `Arc` instead of cloning); it is not applied here because feature work is
paused. Record variables also rebuild their field defaults per call (`initial_fields`).

## 4. What the numbers say about the executable form (decision D4)

- For code that is AL computation (Pageworks, the Pageworks-like parts of any app) dispatch is the main cost, 45 % in
  `eval` and `exec_block`, plus about 20 % in value handling and allocation that a typed, slot-based form removes.
  That favors **bytecode** (register form, typed operations, no `Result<Value, Signal>` per node).
- For the Microsoft suites (the product's main use) dispatch is 7 %. The cost is allocation from record handling:
  schema clones, per-variable field defaults, row clones. That is ADR-035's ground (record store, keys and rows) and
  a quick fix, and it comes first: it is worth more than any executable form for Tests-ERM.
- So: do the schema-clone fix and ADR-035 first (ADR-034 point 1: "if record operations dominate, ADR-035 goes
  first"), then build the executable form as **bytecode**, because the second profile that remains after that fix will
  look like Pageworks (dispatch and value handling).
- Already measured wins on the current tree-walker, for the 0-difference budget: `Result<Value, Signal>` from 72 to
  40 bytes (Signal 72 to 32), builtin lookup from a linear scan to one hash probe, Integer arithmetic and number
  conversion without `Decimal`, array dimensions read in place. Together about 15 % less CPU on Pageworks.
