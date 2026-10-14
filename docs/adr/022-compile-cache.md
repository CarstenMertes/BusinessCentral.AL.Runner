# ADR-022: Compile cache: one entry per dependency set

- Status: Accepted (owner, 2026-10-14; the measurement below amended decisions 2 to 4 and the owner accepted the amendments)
- Date: 2026-10-14
- Amends: ADR-013 (decision 6: its "persistent cache only if the warm start needs it" becomes a design; its
  "lowered IR is not cached" is lifted for dependency apps), ADR-016 (the deferral of persistent dependency
  caches, PERF-2)
- Related: ADR-004 (snapshots), ADR-006 (core purity), ADR-009 (what may differ from BC), ADR-011 (IR),
  ADR-015 (version gates)

## Context

Today every run compiles the dependency apps again, demand-driven (ADR-013 decision 6): parse, check and lower
only what the tests reach. A full corpus run with Microsoft's apps takes seconds. The owner's performance goal
(owner-review, 2026-10-14) puts caching last, after the work itself is cheaper. This ADR is that last step, designed
now so the measurements can decide whether to build it. No code before the measurement section below is done and the
owner accepts.

Two facts shape the design:

- **Microsoft ships hotfixes, sometimes several per day.** An entry is valid for one exact app version only. A
  cache that is keyed by the whole stack is rebuilt after every hotfix, and Base Application is the large part.
- **AL Runner v2 restored stale rows silently.** Its install-baseline cache key missed an input (the company
  normalization rule set), so a baseline captured under old rules was restored into a run that asked for new
  ones. Its remedy is a manual rule: bump `RuleSetVersion` (`running-ms-test-buckets` skill). Here the key must
  be unable to miss an input by construction (decision 4).

How BC loads, for contrast: the NST loads the metadata of all objects and syncs the SQL schema when it starts. This
runtime has no SQL schema step, so a cache can be opened by reading an index and decoding only what a test touches.

## Decision

1. **Caching is on by default.**
   - On: the runner compiles every Microsoft dependency app completely (eager, no demand pruning) once per app
     version, stores the result, and every later run loads it.
   - Off (`--no-compile-cache`, `ALRUNNER_NO_COMPILE_CACHE=1`; `--no-cache` implies it): the demand-driven partial
     compile of ADR-013 stays as it is. It is the reference path and the fallback.
   - Only dependency apps are cached. The workspace apps (the code under test) compile on every run.

2. **One entry per dependency set** (owner, 2026-10-14, open question 5; design A of the measurement). The set is every
   non-workspace app of a run, in dependency order. Workspace apps are ordered after them in the program, so the
   position of every dependency object and every base field is the same in every run with that set and needs no
   remap. (Per-app entries were measured and do not work: the IR refers to objects and fields by position. See
   "Can lowering be done per app?".) A customer extension never invalidates the entry. A Base Application hotfix
   rebuilds the whole entry, which is 79 % of the objects under either unit. An entry holds:
   - a header: magic, format integer, the key (decision 4), a checksum of the index;
   - an object index: object type + id -> offset and length of its block;
   - the event-subscriber index, precomputed (publisher -> subscribers), by name;
   - the member and type tables the workspace is checked against (fields, enum values, procedure signatures,
     extension fields the app adds to other apps' tables);
   - one shared zstd dictionary (level 3, about 512 KB) for all blocks;
   - **one block per object**: its lowered IR (serde + postcard), compressed on its own with zstd level 3 and the
     shared dictionary, with its own checksum. The block lists the objects whose extension or implementer set it
     embeds (the "whole-program facts" of the measurement: `InterfaceCall::targets`, `InterfaceTest::objects`,
     `EnumImpl::by_ordinal`, table and page extension lists, merged page controls). **A block whose listed object is
     extended or implemented by a workspace object is not loaded; it is lowered again from source with the demand
     path.** About 4 % of the objects are affected;
   - per routine, the refusal reason if lowering refused it (decision 7).
   - No string table (the dictionary reaches nearly the same size with no format change). No call graph in the
     blocks (three quarters of the raw bytes; derived from the IR where `baseline.rs` and the explanations need it).
     No level 19 (8 % smaller, not worth the time). No source positions (dependency IR has none).
   - The entry is about 16 to 19 MB per BC build.

3. **Loading costs as little as possible.** (The first implementation keeps three parts of this for later; see
   "Not done" below: the event index and the member tables are not stored, the sources of the dependency apps are
   still read, and the blocks are decoded at the start, not on first touch.)
   - Open = read the header and the index. Nothing else.
   - The event index and the member tables are used as stored. They are not rebuilt from objects.
   - The objects the install baseline reaches are decoded in parallel at start (4 threads: 70 to 90 ms), because every
     run reaches about 3 100 of them. A block is otherwise read, checked and decoded the first time a test touches its
     object. An object no test touches is never decoded. Per-routine laziness is not needed. `Program` therefore gets lazily filled object slots (`OnceLock`, because test codeunits run on worker
     threads). This changes how the engine reaches `Program::objects` and needs an accessor (ADR-011, additive).
   - The format code is a core crate over bytes (ADR-006). The file access (read, rename, directory) sits next to
     the existing download cache in `al-deps/src/cache.rs`.

4. **The key** has five inputs. Every one must be in it, and a test proves each one misses when it changes:
   1. app id and 2. app version, for every app of the set, in order;
   3. a hash of each app's source. For an artifact the hash is made once when the artifact is stored (ADR-013
      decision 13 keeps CRC-32 and size per entry). For `.alpackages` and `--deps` files it is a hash of the zip
      central directory (name, size and CRC-32 of every entry), which needs no data read;
   4. the target BC version, with the runtime version (`Target`, ADR-015: gates change lowering);
   5. the runtime build id. Any compiler change must invalidate. Decided here: the hash of the running executable
      (it cannot miss a crate; a hash of source files can). Measured at 6 to 8 ms; there is no build-script fallback.

   The key of the entry is the ordered list of these per-app inputs plus inputs 4 and 5. Any hotfix of any app in the
   set is a miss and rebuilds the whole entry. The format integer is part of the key as well; a mismatch is a miss and rebuilds, never an error (ADR-016).

   **No input can be forgotten.** The key is one struct. The hash function destructures it field by field without
   `..`, so adding a field that is not hashed does not compile. The install-baseline key (if built) takes the key of
   the entry plus the test-data and normalization inputs, built the same way.

5. **Writing is safe and local.**
   - Write to a temporary file in the cache folder, then rename. A reader sees the whole old file or the whole new
     file. Two parallel runs may both build the same entry; the last rename wins, and the bytes are the same
     (the build is deterministic, a test checks it).
   - A damaged file is refused and rebuilt, never used: the header and index checksum are checked at open, a block
     checksum at first touch. A failed check (named reason on stderr) ignores the entry, the run
     continues from a rebuild (or the demand path), and the damaged file is replaced. A damaged cache never changes an
     outcome and never fails a run. `alrunner cache verify` reads every block, for CI.
   - **Licensing (ADR-013, "cache only"):** the cache is a local file made on the user's machine from source the
     user downloaded. Nothing compiled from Microsoft code is committed, released, uploaded as a release asset or
     shared outside the user's own CI cache.
   - Location: `<cache root>/compile/` (owner decision 3 below).

6. **CI** caches the cache files between pipeline runs with `actions/cache` (owner, 2026-10-14: short-lived key per BC build; a
   separate step builds or verifies the cache before the measured run, so the measured run is always a hit; the job
   summary reports the times with the cache on and the cache build time separately).
   - The `actions/cache` key is built from the same inputs as decision 4: the output of `--print-builds`
     (the app versions), the runtime build id and the format integer. No `restore-keys`: an entry of another version
     is never usable.
   - Entries are short-lived (hotfixes), so before saving, `alrunner cache prune` deletes every entry the current
     stack does not use. GitHub evicts caches unused for 7 days and caps a repository at 10 GB.
   - **Step order** in the pipeline:
     1. restore the cache;
     2. `alrunner cache build` builds missing entries and verifies existing ones, and prints its own time;
     3. the measured run, with a flag that turns any cache miss into an error (`--require-compile-cache`), so the
        reported run is always a cache hit;
     4. the outcome comparison against `--no-compile-cache` (decision 7);
     5. the job summary: run time with caching on, and the cache build time as a separate line;
     6. prune and save the cache.

7. **Refusals found by the eager compile** are stored per routine in the entry. They only matter when a test reaches
   that routine; then the test is refused with the stored reason, as today. A refusal in code nothing reaches does not
   change any outcome. The evidence that a full compile and a partial compile give the same outcomes is the
   existing demand-versus-eager comparison (ADR-013, "The permanent check": `tools/compare_junit.py` over a second run
   with `AL_EAGER_DEPENDENCIES=1`). That comparison stays, and gains a third leg: cached against uncached.

## Measurement (spike done 2026-10-14)

A throwaway branch (`cache-spike`, not merged) serialized the lowered program of an eager compile of the whole
dependency stack and read it back. Nothing here is a design; it is what the numbers are. Method notes:

- **Stacks.** BC 29.0.54011.56262: System, System Application, Business Foundation, Base Application,
  Application (the corpus `.alpackages`). 9 683 dependency objects, 148 860 routines, 4 201 of them refused.
  The Tests-ERM closure is BC 27.5.46862.48827 (the only artifact here with the `Tests-*` sources): 9 919
  dependency objects, 153 326 routines.
- **Machine.** 12 cores shared with other agents. Check, lowering and test threads: 4 each. Load average is
  noted per number. Run time depends strongly on load (the same all-32-buckets run took 31.5 s at load 20 and
  12.7 s at load 7), so only numbers from one session are compared. CPU time (`user`) is the steadier figure.
- **Codec.** `postcard` for the IR. Round trip checked: 0 of 9 683 objects differ after decode.

### 1. Size (BC 29 stack, dependency objects only)

| What | Size |
|---|---:|
| postcard, raw | 52.4 MB |
| bincode 1.3, raw | 113.2 MB |
| postcard, per-object zstd level 3 | 15.6 MB |
| postcard, per-object zstd level 19 | 14.3 MB |
| postcard, per-object LZ4 | 21.4 MB |
| postcard, whole stream zstd 3 (no per-object split) | 9.3 MB |
| per-object zstd 3 with one shared 512 KB dictionary | 12.3 MB (+0.25 MB dictionary) |
| call graph (`Lowered::calls`), raw / per-object zstd 3 | 151.2 MB / 6.3 MB |
| object index (`ObjectSym` of all dependency objects, with 3 375 event subscribers) | 3.09 MB raw, 0.49 MB zstd |
| member tables (44 254 fields, 97 549 procedures) | 13.4 MB raw, 2.35 MB zstd |

- **The call graph is three quarters of the raw bytes and does not belong in the blocks.** It is derivable from the
  IR. Its consumers are `baseline.rs` (static reach of the install roots) and the refusal explanations. Without it
  the blocks are 52 MB raw instead of 204 MB. The numbers above leave it out unless stated.
- **Level 19 saves 8 % over level 3 and is not worth it.** LZ4 is 37 % bigger than zstd 3.
- **Per-object compression costs 67 % against one stream** (median object 2 KB, p90 13 KB, largest 246 KB). A
  shared dictionary takes back most of it. The cost is an index entry per block (0.52 MB for 9 683 blocks).
- **Strings are two thirds of the raw bytes.** 1.94 million strings, 35.2 MB, in the blocks; 252 469 distinct
  strings, 10.5 MB (2.0 MB zstd 3). A shared string table would give about 20 MB of blocks plus the 10.5 MB table
  before compression. It would not make decode faster (decode time is allocation, see 2). The dictionary reaches
  nearly the same size with no format change. Recommendation: dictionary, no string table.
- **Dependency IR carries no source positions** (0 of 53 376 positions in the program belong to a dependency
  object). `Program::positions` and `sources` are workspace-only and stay out of the entries.
- **One entry (blocks with dictionary, object index, member tables, block index) is about 16 to 19 MB per BC build.**
  The Tests-ERM closure is the same size (15.9 MB blocks).

### 2. Decode time (one thread, warm file cache, cold process)

| What decoded | Blocks | Postcard bytes | Time | Of which zstd | Of which postcard decode |
|---|---:|---:|---:|---:|---:|
| Open: block index (9 683 entries) | | 0.52 MB | 1.2 to 1.3 ms | | |
| Corpus run: objects the demand compile activates | 3 601 | 28.9 MB | 269 to 283 ms | 62 to 65 ms | 201 to 213 ms |
| One corpus test file (`--file`) | 3 176 | 26.3 MB | 239 to 248 ms | 54 to 56 ms | 181 to 187 ms |
| Small app, one test (install baseline reach) | 3 105 | 26.1 MB | 239 ms | 54 ms | 180 ms |
| Tests-ERM (27.5) | 5 415 | 39.0 MB | 361 ms | 89 ms | 264 ms |
| Everything, one thread | 9 683 | 52.4 MB | 471 to 488 ms | 129 to 133 ms | 330 to 343 ms |
| Everything, 4 threads | 9 683 | 52.4 MB | 188 to 190 ms | | |

- The earlier estimate (load 50 to 150 ms for about 35 000 routines) was low by a factor of two to three on one
  thread. The 35 298 routines of a corpus run sit in 3 601 objects, and an object decodes whole.
- **Decode is allocation work, not decompression.** zstd is a quarter of the time. About 28.9 MB of postcard becomes
  280 MB of heap (the uncached corpus run peaks at 1 394 MB in total).
- **A single test of a small app reaches 3 105 objects**, the same as a whole corpus run, because the install
  baseline (`Company-Initialize` and the install codeunits) is a root of every run. So the cost of a small run is
  the baseline's reach: 0.24 s decode on one thread, then 0.2 s to run the baseline.
- The "reached" objects are the demand compile's active set (`SCHEMA`, `READ` and `FULL` levels). A cached run
  that decodes at first touch decodes at most this many.
- Object index decode: 15 to 22 ms; rebuilding `by_name`, `by_id` and `subscribers_of` from it: 6 to 12 ms.
  Member tables decode: 42 to 69 ms for all objects, against 400 to 785 ms to compute them today (measured
  warm, after the files were parsed; cold it includes the parse). The event index is a by-name list per object
  (0.21 MB); the global map is rebuilt in a few milliseconds and is not stored.

### 3. Open time

Index 1.3 ms, object index 15 to 22 ms plus maps 6 to 12 ms, member tables 42 to 69 ms (or lazily per object):
**25 to 35 ms with the member tables lazy, 70 to 100 ms with everything decoded.** Today a run with dependencies
reads and inflates the dependency sources ("load apps and dependencies", 0.11 to 0.18 s) and scans 108 MB of source
(62 to 180 ms wall, 9 746 files). A cache hit skips both.
The member tables and the event index can be stored, but not "as is": `FieldInfo::declared_in`, `ProcSig::declared_in`
and `EnumImpl::declared_in` are object indices, and `on_validate`, `on_lookup` and `ProcSig::offset` are byte offsets
into source files. Types (`Ty`) refer to objects by name, so they store as they are. The offsets are used only
when an object is lowered from source.

### 4. Eager compile (the one-time cost)

| Run | Compile wall | Peak memory | Load |
|---|---:|---:|---:|
| Corpus stack, BC 29, 4 threads | 8.1 to 8.8 s | 3.3 GB | 8 |
| Corpus stack, BC 29, 1 thread (`AL_CHECK_THREADS=1 AL_LOWER_THREADS=1 RAYON_NUM_THREADS=1`) | 9.5 s | 3.2 GB | 6 |
| Corpus stack, BC 29, 4 threads | 13.4 s | 3.3 GB | 18 |
| Tests-ERM closure, 27.5, 4 threads | 9.6 s | 3.8 GB | 11 |
| Tests-SMB closure, 27.5, 4 threads | 9.2 s | 3.4 GB | about 8 |

The estimate was 10 to 30 s per Base Application version; it is **about 9 s on a quiet machine and 13 s on a busy
one**, and it is almost the same on one thread (the eager check is serial; lowering is parallel). Serializing and
compressing the blocks adds roughly 2 s (the spike's pass, which does five codecs, takes 3.3 s on 4 threads). Memory:
3.3 to 3.8 GB, so the builder needs a cap of at least 5 GB.

### 5. Peak memory of a cached run

Not measured as one run (no cached run exists). From the parts: decoded IR of the objects a corpus run reaches
280 MB (measured in the reader), plus the workspace compile (197 MB for the corpus alone, measured with `--no-deps`),
plus the object index and member tables (about 50 MB decoded). About 0.5 to 0.6 GB against 1.4 GB uncached
(corpus), 1.2 GB for the small app. Estimate, not a measurement.

### 6. Hash costs

| Hash | Time |
|---|---:|
| sha256 of the running executable (14.8 MB, file cache warm) | 6.4 to 8.4 ms (read 2.6 to 12 ms) |
| blake3 of the same | 2.5 to 5.1 ms |
| central directory of `Base Application.app` (38.4 MB file, 8 350 entries, 0.83 MB directory): read, parse, sha256 | 3.7 ms + 1.1 ms |

Decision 4.5 stands: **hash the executable; the build-script fallback is not needed.** Total key cost for the
five apps is under 20 ms.

### 7. Can lowering be done per app? (the go / no-go question)

**No, not as decision 2 is written.** The IR refers to other objects by position in the one global
`Program::objects` list, and to table fields by position in a list that other apps' extensions grow. It also holds
facts about the whole program. Findings, each measured or shown by a test:

1. **Object references are positions in `Program::objects`.** `Expr::Call::object`, `InterfaceCall::targets`,
   `RtType::{Record, Codeunit, Page, TestPage, Query, Interface, Enum}`, `Const::Enum`, `Table::extensions`,
   `Field::on_validate` and friends, `Page::extensions`, `Control::owner`, `Control::part_page`,
   `RunObject::object`, `SrcPos::object`, report, query and xmlport `table`/`base`/`extension` fields: about 35 places
   in `al-ir`. The workspace apps come first in the list today, so **every dependency object moves by the size of the
   workspace**: Tests-ERM against Tests-SMB on the same artifact moves all 9 919 dependency objects by 245, and only
   1 628 of them (16 %: enums, permission sets, profiles, `object` placeholders) have byte-identical IR. A table's
   IR differed in 936 places for Customer (table 18), a codeunit's in 4 717 (Sales-Post). In the 26 objects
   diffed field by field (codeunits, tables, pages, reports, queries, an xmlport), **every difference was a shift by
   245**; nothing else changed.
2. **Field references are positions in the table's merged field list.** `Place::Field(place, usize)`,
   `Const::FieldPos`, `Table::keys`, relation arms, `DataItemLink`, page control bindings. The order is: the table's
   own fields, the six system fields, then the fields of extensions in workspace object order
   (`members.rs`, `merged_members`). A test on the spike branch compiles app B (a table extension that adds `BF`,
   plus a codeunit that assigns `Rec.BF`) alone and next to app X, which extends the same table first: `BF` is
   field 7 in one and field 8 in the other, and the codeunit's IR differs
   (`Assign(Field(Local(0), 7), ..)` against `Field(Local(0), 8)`). `Place::Field` does not name the table, so a remap
   has to type the place from the slot it starts at.
3. **Other apps' contributions are merged into an object's IR**, so an object block is not a function of its own
   app: `Table::fields` and `keys` (extension fields), `Table::extensions`, `Field::ext_before_validate` and
   `ext_after_validate`, `Page::controls` (page extension controls are inserted into the base page's list, which
   shifts the control indices that `Expr::Control` uses), `Page::extensions` and `ext_triggers`, enum values,
   report extensions. And some **dependency routines embed the fan-out over all apps**: `InterfaceCall::targets`
   (every implementer), `InterfaceTest::objects`, `EnumImpl::by_ordinal` (every enum value, extensions included).
4. **How much of this exists in the stack.** In the Tests-ERM closure there are 67 extension objects that cross an
   app boundary (11 page and 4 table extensions of Base Application on Business Foundation objects, 17 page, 3 table,
   10 enum and 2 permission set extensions on System Application, plus the test apps), and 315 extensions inside their
   own app (these are fine). Dependency IR contains 537 `InterfaceCall`, 13 `InterfaceTest` and 156 `EnumImpl` sites in
   about 200 objects, and 210 tables and pages list extensions. **About 400 of 9 919 objects (4 %) carry whole-program
   facts.** Every other block depends only on object positions and on field positions of base fields.

What it would take:

| Design | What changes | Estimate (mine) |
|---|---|---|
| **A. One entry per dependency set** (all non-workspace apps of a run, in dependency order). Workspace apps are ordered after them. | Dependency object indices and base field positions are then the same in every run with that set, so **no remap**. Needs the app order changed (workspace last; `runs[0].app = Some(1)` and the app index logic move), blocks that list the objects whose extension or implementer set they embed, and a load step that re-lowers (demand path, from source) the blocks whose listed objects the workspace extends or implements. Fallback to lazy parse of the source for the few objects the workspace checker reads directly. | 2 weeks |
| **B. One entry per app** (decision 2 as written). | All of A, plus: stable references (a typed remap visitor over about 80 IR enum variants for object indices and field positions, with a field-id table per table), a canonical extension order, and a merge step for fields, controls, enum values and subscribers contributed by other entries. | 4 to 5 weeks, with a risk of subtle wrong-field bugs that the corpus would catch only partly |

**Per app buys little here.** Microsoft's apps move together per BC build. Base Application is 7 853 of 9 919
objects (79 %), System Application 1 244 (13 %). A hotfix of Base Application rebuilds 79 % of the set under A as
well. The only gain of B is keeping System Application and Business Foundation (14 %) across a Base Application hotfix.

### 8. Projected run times

"Today" is origin/main (3c35b4e) in one session at load 6 to 7, two runs each, `Time:` line of
`--timings`; the spike's cached figures are built from measured parts as shown. Cached = workspace compile
(measured as the same run without dependencies, or the workspace share of check and lowering) + open + decode +
install baseline + tests. The workspace share is the measured share of check and lowering thread-seconds spent on
workspace objects (corpus: not separable, taken from `--no-deps`; Tests-ERM: 24 %; 32 buckets: 53 %).

| Run | Today (total, user CPU) | Cached, projected | Factor |
|---|---|---|---:|
| Small app, one test (`alrunner test`, BC 29) | 2.10, 2.12 s; 6.0 s CPU | 0.35 to 0.55 s: open 0.03 + decode 0.07 (4 threads) to 0.24 (1 thread) + baseline 0.20 + test | 4 to 6 |
| Corpus, BC 29, with Microsoft's apps | 2.92, 3.88 s; 9.3 and 11.9 s CPU | 1.7 to 2.1 s: workspace compile 0.35 to 0.5 + open 0.03 + decode 0.1 to 0.25 + baseline 0.2 + tests 0.86 to 1.09 | 1.4 to 2.3 |
| Tests-ERM (27.5) | 6.13, 5.01 s; 17.6 and 14.3 s CPU | 2.3 to 2.8 s | about 2 |
| All 32 buckets, one process | 13.17, 12.69 s; 38.3 and 34.8 s CPU; 5.9 GB | 8.3 to 9.5 s (test apps are 91 MB of source and stay) | 1.4 |

The earlier estimates (corpus 1 s, single test 0.3 s, 32 buckets 6 to 8 s) are not reached. The floors are the
tests that run only with Microsoft's apps (0.86 to 1.09 s in the corpus), the workspace compile, and the baseline run.
The corpus without Microsoft's apps takes 0.45 to 0.53 s in total. The break-even is 6 to 12 runs: the build is
about 12 s, the saving 1 to 2 s per corpus run or about 1.6 s per small run.

Uncached numbers will drop when `perf-3` merges (-18 % instructions at 4 threads, -29 % at 12), which narrows the
corpus gap more than the small-app gap, because the small-app cost is the dependency work perf-3 does not remove.

### Go / no-go

The rule was: build only if a warm cached run is clearly faster than the best uncached run, on the corpus and one
bucket. Read against the numbers:

- **Clearly faster:** a small app with one test (4 to 6 times) and Tests-ERM (about 2 times).
- **Faster, less clearly:** the corpus (1.4 to 2.3 times, the two runs differ by 1 s; closer after perf-3 merges) and all 32 buckets (1.4 times).
- **Per-app entries (decision 2): no-go as written.** The unit has to be the dependency set (design A) or the work
  grows to design B.
- Not tried here: making the uncached path cheaper for a small app. Its cost is the install baseline's reach (3 105
  objects, about 6 CPU-seconds), so a cheaper baseline would help the uncached path and the cached one alike.

### How to reproduce

Branch `cache-spike` (commit "SPIKE (do not merge)", worktree `wt-cache-spike`). Environment variables, all read by
`al-testrun/src/runtime.rs` and `spike.rs`: `AL_EAGER_DEPENDENCIES=1` with `AL_SPIKE_WRITE=FILE` runs the eager
compile, prints every size above, writes the blocks, and exits; `AL_SPIKE_DEMAND=FILE` writes the demand compile's
active objects of a normal run; `AL_SPIKE_NS=1` prints thread-seconds of check and lowering split by workspace and
dependency; `AL_SPIKE_LOADSPLIT=1` times the workspace load per app kind; `AL_SPIKE_HASHES=FILE` writes a hash per
dependency object (to compare two closures). `cargo run --release -p al-test-cli --example cache_spike_read -- FILE
DEMAND.json...` opens a written file and times the decode. The corpus run is
`al-test-cli --root . --bc-version 29`; the bucket runs are the recipe in `docs/alrunner-buckets.md`.

## Amendments adopted (owner, 2026-10-14)

The owner accepted all seven amendments the measurement proposed. They are folded into the decisions above:
(1) the entry is per dependency set; (2) blocks list the objects whose extension or implementer set they embed and are
lowered again when the workspace extends or implements one; (3) no call graph in the blocks; (4) per-object zstd
level 3 with one shared dictionary, no string table, no level 19; (5) the objects the baseline reaches are decoded in
parallel at start; (6) the executable is hashed, no build-script fallback; (7) the first-run cost is 9 to 13 s and
5 GB of memory in the builder, and an entry is 16 to 19 MB per BC build.

## Open questions (all answered, see "Owner decisions")

Each was one decision. The recommendation came first.

1. **How are blocks serialized?**
   - Recommended: **serde + postcard (or bincode) + per-object zstd.** The IR already derives serde. Small change,
     small risk, format versioned by the integer. Zero-copy gives nothing here: a compressed block must be
     decompressed into memory before it can be read.
   - Alternative: rkyv-style zero-copy. Needs derives and alignment on every IR type, and works only for
     uncompressed data (index and tables, not the blocks). Gain is a faster open of the tables. Pick it only if the
     measurement shows the tables dominate.
2. **Is the install-baseline snapshot in version 1?**
   - Recommended: **no, later.** Version 1 caches compiled code. The baseline is the part where the v2 failure
     happened, and it also depends on test data and normalization inputs. Add it after the code cache is proven,
     with its own key test.
   - Alternative: yes. Saves the install run (`Company-Initialize` and install triggers) on each process start,
     at the price of the wider key now.
3. **Where does the cache live?**
   - Recommended: **`<cache root>/compile/` with the root the download cache already uses** (`--cache-dir`,
     `ALRUNNER_CACHE`, default `~/.cache/al-runtime`). No new variable; one folder to delete or save in CI.
   - Alternative: its own variable (`ALRUNNER_COMPILE_CACHE`). Lets CI keep it apart from the artifacts. Costs a
     second setting to explain.
4. **WebAssembly: cache or none?**
   - Recommended: **none in version 1.** The zstd crate needs a C compiler and is not a clean WebAssembly build;
     the host has no agreed storage. WebAssembly keeps the demand-driven compile.
   - Alternative: the host hands the format code a byte source (`ByteSource`, as in `al-bcdb`), with a pure Rust
     decoder (`ruzstd`, slower, or LZ4). The format code is already core and OS-free, so this stays possible.

5. **What is the unit of an entry?** (added after the measurement)
   - Recommended: **the dependency set** (all non-workspace apps of a run, in dependency order, workspace apps ordered
     after them). No remap of object or field positions, about 2 weeks, and a Base Application hotfix rebuilds 79 % of the
     set under either unit.
   - Alternative: **one entry per app** with stable references and a merge of other apps' contributions. About 4 to 5
     weeks, riskier, and it keeps only System Application and Business Foundation (14 % of the objects) across a Base
     Application hotfix.
   - Alternative: **do not build.** Pick this if a corpus run 1.4 to 2.3 times faster and a bucket run 1.4 to 2 times
     faster is not worth the second path (decision "Two paths stay"). The small-app run is 4 to 6 times faster.

## Owner decisions (2026-10-14)

The owner accepted every recommendation above and answered question 5:

1. Blocks are serialized with serde + postcard (or bincode) and compressed per object with zstd.
2. The install-baseline snapshot is not in version 1.
3. The cache lives in `<cache root>/compile/`, under the root the download cache already uses.
4. WebAssembly has no compile cache in version 1.
5. **One cache entry per dependency set**, not per app (design A). Workspace apps are ordered last.
6. CI caches the cache files with `actions/cache`; a separate step builds or verifies the cache; the measured run is a
   cache hit; the job summary reports the run time with the cache on and the cache build time apart.

Decided without owner review (2026-10-14): see `docs/owner-review.md` (the file layout of an entry, the way a block is
lowered again from source, the corruption handling, and the CLI spelling).

## What was built (2026-10-14, branch `compile-cache`)

- **Crates.** `al-cache` (core, no OS access): `Key`, the entry file, `build` and `Entry`. `al-sema/provided.rs`:
  `DepFacts`, `DepBlock`, `DepSource`, `relower_set`. `al-testrun/compile_cache.rs`: the key of a run, reading, writing
  and building the file. `lower::lower_for_cache` builds an entry (the complete compile of the dependency apps alone);
  `lower::compile_loaded_cached` is a compile that takes those objects from it.
- **The file.** Header (magic, format, key digest, where the index is, hashes), the zstd dictionary, one block per
  dependency object (postcard, zstd level 3 with the dictionary and a content checksum), the index last (the facts of
  every object and where each block sits). Opening reads the header, the index and the dictionary and checks their
  hashes and the length; a block is checked when it is read.
- **What a block is.** The lowered `ir::Object`, why each routine cannot run, and the direct calls of each routine.
  What a variable of a page, report or xmlport type drives is added when the call graph is asked for
  (`Lowered::calls_of`), from the page extensions and parts of the run. The facts beside the blocks are what the
  lowering of other objects reads: the table behind a page, why a page cannot be driven, its parts, the numbering of
  routines, the layout of a query.
- **What makes a block unusable.** A block records the objects whose extensions or implementers it embeds: itself,
  the interface of an interface call or test, the enum of an enum cast, a page or report a variable names (with the
  parts of a page), a page whose control index it uses. The workspace "touches" the objects its own objects extend
  or implement (and the interfaces they extend). Every block that lists a touched object is lowered again from source,
  with the objects that extend each other. A test per case compares the whole `Program`, the refusals and the call
  graph of the cached compile with the complete compile (`provided.rs`), and `demand_audit.rs` runs its 78 probes
  in the cached mode against the complete compile.
- **Order.** `CorpusRuntime::try_run_suites_with` puts the dependency apps first in the program and maps the app
  indices of the caller (`SuiteRun::app`, `RunOptions::only`, `TestResult::app`) both ways.
- **The key** holds the format integer, the sha256 of the executable, the target, the preprocessor symbols, and per
  dependency app its id, name, dependency ids, publisher, version and a hash of every path and byte of its source. For
  an app from a package the source hash is made from the files already read (about 30 ms for Business Central); the
  hash of the zip directory that the first version of this ADR planned needs a change in `al-deps` and is left for
  when the load of the sources is skipped.
- **Outcomes.** The cached compile is the complete compile for the dependency apps. It gives the same outcome as the
  eager run for every corpus test and the same as the demand-driven run except in `tools/demand-eager-known-differences.txt`
  (a test the complete compile refuses before it starts because a page variable's call graph holds every routine of
  the page). Those five tests pass in the uncached run and are refused with the cache. `al-test-cli` therefore uses the
  cache only with `--compile-cache`; the ratchet runs without it. `alrunner test` uses it unless
  `--no-compile-cache` or `ALRUNNER_NO_COMPILE_CACHE=1`.

### Measured with the implementation (2026-10-14, release build, 12 cores shared with other agents)

Two sessions under `flock /tmp/al-perf.lock`, each back to back: uncached, cold (the entry is deleted, so the run
builds it), hit, hit, uncached again. Wall seconds, CPU seconds (user + system), and the peak resident size of the
largest process. The load average is in `docs/adr/022-measurements.txt` (4 to 9 for the first three rows, 8 to 15 for
the last), so compare the CPU column and the pairs, not rows. The first three rows are from the session at the final
commit (`measure2`), the 32 buckets from the session before the last two merges of `main` (see the note below the table).

| Run | Uncached (two runs) | Cold: build + run | Of which the build | Hit (two runs) | Memory: uncached / cold / hit |
|---|---|---|---|---|---|
| Small app, one test (`alrunner`, BC 29, no test data) | 1.60 s, 1.51 s; CPU 7.1, 7.0 s | 5.33 s; CPU 23.6 s | 4.51 s | 0.74 s, 0.75 s; CPU 2.6 s | 1259 / 3922 / 896 MB |
| Corpus, BC 29, Microsoft's apps (`al-test-cli`) | 2.24 s, 2.48 s; CPU 12.2, 12.5 s | 6.49 s; CPU 28.3 s | 5.00 s | 1.46 s, 1.51 s; CPU 7.1, 7.4 s | 1445 / 3766 / 1232 MB |
| Tests-ERM, BC 27.5, test data (`alrunner`) | 12.02 s, 11.72 s; CPU 39.1, 40.7 s | 13.99 s; CPU 50.7 s | 5.76 s | 8.09 s, 8.34 s; CPU 27.8, 28.2 s | 3107 / 4183 / 2112 MB |
| All 32 buckets, one process, test data (earlier session) | 33.4 s, 30.4 s; CPU 130.8, 125.5 s | 29.2 s; CPU 124.9 s | 4.71 s | 35.0 s, 34.8 s; CPU 118.5, 116.1 s | 6310 / 4963 / 4710 MB |

Note on the 32 buckets: `main` as of the last merge needs more than 18 GB for the combined run with test data and is
killed, with or without the cache (the binary built from `main` in the main checkout at 18:41 too), so the row is from
the session before. It is a regression on `main`, not an effect of the cache.

- **Build.** 4.5 to 5.8 s and 3.8 to 4.2 GB, for an entry of 12.9 to 13.3 MB. The complete check of the dependency apps
  runs on all cores now (`Checker::check_objects_parallel`); before, it was serial and the build took 10 to 19 s.
- **Hit.** The file opens in 0.03 to 0.05 s (key, read, header and index). Reading the blocks takes 0.05 to 0.17 s
  on all cores (more when the machine is loaded). A small app runs 2 times faster than uncached, uses a third of
  the CPU and 30 % less memory. The corpus runs 1.6 times faster with 40 % less CPU. Tests-ERM runs 1.45 times faster
  (the 5 to 6 s of tests with test data do not change) with 30 % less CPU and a third less memory.
- **All 32 buckets: no gain in wall time.** The workspace holds 91 MB of test sources, which load, check and lower the
  same with and without the cache (load 2.5 s, check 1.3 s, lowering 1.1 to 1.5 s), and 19 s of the run is the
  tests with test data. The cache replaces the demand compile of the dependency apps (about 3 s on a quiet machine)
  by reading the blocks (0.15 s). The CPU time is 8 % lower, the peak memory 25 % lower.
- **Not reached: the projections of the measurement section** (small app 4 to 6 times, corpus 1.4 to 2.3 times). The
  sources of the dependency apps are still read, inflated, hashed and scanned on every run (0.2 to 0.35 s), every
  block of the objects the workspace does not change is decoded at the start (more objects than a run reaches), and
  the whole program is built (the engine reads every object). Skipping these needs the object index and the member
  tables in the entry, a workspace without dependency sources, and a lazily filled `Program::objects`; see "Not done".
- **Outcomes.** The program of the cached compile equals the program of the complete compile object for object
  (`provided.rs` tests, the 78 probes of `demand_audit.rs`), every corpus test of BC 27, 28 and 29 and of the OnPrem
  and isolation suites has the outcome it has in the eager run, and Tests-ERM with test data and all 40300 tests of the
  32 buckets have the outcome of the eager run (0 differences). Against the uncached run (demand-driven compile): the
  corpus differs in the 5 tests of `tools/demand-eager-known-differences.txt` (on all three versions), Tests-ERM in
  none, and the 32 buckets in 7 of 40300 tests (a test that passes or fails uncached is not run with the cache). The
  cause is the same as in the corpus: a routine that only the complete compile has is in the static call graph of a
  page variable, for example `Navigate.OnAction` through a `RunObject` action of a dependency page.

### Not done

- **Skipping the load of the dependency sources, and lazy objects.** The object index and the member tables are not in
  the entry; the sources of the dependency apps are read and scanned on every run; every needed block is decoded at
  the start. `Program::objects` is a plain list and the engine reads all of it (tables, the event registry), so lazy
  decode needs those whole-program passes stored in the entry first.
- **The same outcome as the uncached run for every test: done (2026-10-14, branch `zero-diff`, owner decision "there
  should be zero differences").** The static call graph of a page, report or xmlport variable (`lower.rs`,
  `driven_edges`) is a function of the sources (routine roles), not of what was compiled. A `TestPage` variable adds the
  page, its extensions and its parts; the `RunObject` targets of a workspace page, and of the page the variable names,
  are compiled but add an edge only for workspace pages. A `Page` variable of a dependency page adds no edge, and a
  platform method called on it (`Run`) reaches the page. A report or xmlport variable adds its triggers, and the demand
  compile reaches them whenever the variable is declared. `CurrPage` adds nothing. The corpus on BC 27, 28 and 29 and the
  suites have no difference between demand, eager and cached, and `tools/demand-eager-known-differences.txt` is empty.
- **`alrunner cache` (list, verify, prune)**; CI prunes with a shell step and verifies with
  `al-test-cli --build-compile-cache`.
- **The install-baseline snapshot** (owner decision: not in version 1).

Decided without owner review (2026-10-14):

- A damaged, cut or foreign file is ignored with a note, removed, and the run builds a new one (or, if the entry was
  read but a block fails, compiles without it). The first version of this ADR said the run stops. Ignoring cannot
  change an outcome and needs no manual step. `--require-compile-cache` makes any of that an error.
- The corpus tool is opt-in (`--compile-cache`), because its baseline is recorded under the demand-driven compile.
- A run that uses an entry sets its modification time; a run that builds an entry removes the entries nothing used for
  30 days (an app that changes on every run would otherwise leave 13 MB behind each time).
- `al-test-cli --build-compile-cache` builds or checks the entries and prints the build time; CI uses it. There is no
  `alrunner cache` command yet.
- The facts and blocks of an entry hold the dependency objects only; the sources of the dependency apps are still read
  and scanned on every run, and the blocks are all decoded at the start (in parallel).

## Consequences

- **IR serialization becomes a seam.** The format is versioned by an integer in the header and the key. Changing an
  IR type changes the runtime build id and so invalidates all entries; no migration code is needed.
- **The program order changes, `Program` does not.** The dependency apps come first in the program of a cached run, so
  an object index means the same in the entry and in the run; `CorpusRuntime` maps the caller's app indices both ways.
  Lazy object slots are not built yet (see "Not done").
- **First run is slow.** Building the entry is the complete compile of the dependency set: measured 4.6 to 5.3 s and
  3.8 to 4.1 GB for Business Foundation, System Application and Base Application of BC 27.5 to 29 (it was 10 to 19 s
  while the complete check ran on one thread). The runner prints one line to stderr saying it is building the cache
  once. Later runs, and every CI run after the cache-build step, do not pay it.
- **Memory:** the builder holds the complete compile in memory once (3.8 to 4.1 GB measured, so it needs a cap of at
  least 5 GB). A cached run holds the decoded program: 0.9 GB for a small app, 1.2 GB for the corpus, 2.1 GB for
  Tests-ERM, against 1.3, 1.4 and 3.1 GB uncached.
- **Disk:** an entry per set of dependency apps (13 MB), left behind after each hotfix. A run that builds an entry
  removes the ones nothing used for 30 days. `alrunner cache` and `alrunner doctor` (ADR-016) are not built.
- **Two paths stay.** The demand-driven compile remains (cache off, WebAssembly, differential), so both are
  maintained. The cached path adds no new behaviour: it gives the outcomes of the complete compile, which are those of
  the uncached path except in a few tests (see "Outcomes" above and the open question in `docs/owner-review.md`).
- **ADR-016's rule** (no feature that only hides start-up or run cost) is not broken: this is the owner's explicit
  request, placed after the performance work, with a go/no-go gate.

## Conformance

- **Key misses (one test per input).** Build an entry, change exactly one input, assert a miss and a rebuild: app
  id, version, source (one changed byte in one source file, and one changed entry in a central directory), target BC
  version, runtime build id, the key of a dependency entry, the format integer. The same inputs hit. A test lists the
  key struct's fields and fails if one has no miss test.
- **Corrupted entry.** Flip a byte in the header, in the index and in a block: each is refused with a named error,
  the entry is deleted and rebuilt, and the corrupted bytes are never executed. A truncated file does the same.
- **Atomic and parallel.** Several processes build the same entry at once; every reader sees a whole file. Two
  builds produce identical bytes.
- **Sharing.** Two projects with different own extensions load the same dependency-set entry without rewriting it.
  A customer table extension on a Base table and a customer event subscriber give the same outcomes as an uncached
  run.
- **Outcome identity.** The corpus with caching on and with `--no-compile-cache` gives the same outcome and the
  same refusal text for every test, for each BC major (the JUnit files, with the duration attributes removed, are
  byte-identical). Extends the demand-versus-eager check.
- **CI step order.** The workflow file is checked by a test: restore, build/verify, measured run with
  `--require-compile-cache`, comparison, summary, prune and save, in that order; the summary has the cache build
  time on its own line.
- **Licensing.** The existing `no_microsoft_source_in_repo` test also fails if a cache file is tracked by git, and
  the release job's asset list has no cache file.
