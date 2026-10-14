# ADR-022: Compile cache: one entry per dependency app version

- Status: Proposed (owner answered the open questions on 2026-10-14; building waits for the measurement below)
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

2. **One entry per app id + version**, not per stack. Base Application is shared by all projects, and a customer
   extension, which depends on Base Application and is not part of its entry, never invalidates it. An entry holds:
   - a header: magic, format integer, the key (decision 4), a checksum of the index;
   - an object index: object type + id -> offset and length of its block;
   - one string table for the whole entry (names, texts, reasons); blocks refer to strings by number;
   - the event-subscriber index, precomputed (publisher -> subscribers), including subscribers of objects in other
     apps, by name;
   - the member and type tables the workspace is checked against (fields, enum values, procedure signatures,
     extension fields the app adds to other apps' tables);
   - **one block per object**: its lowered IR, compressed on its own with zstd, with its own checksum;
   - per routine, the refusal reason if lowering refused it (decision 7);
   - optionally, per app set, the install-baseline result (ADR-004 snapshot; open question 2).

   Positions, source-file numbers and `Object::app` are local to the entry and remapped when it is loaded.

3. **Loading costs as little as possible.**
   - Open = read the header and the index. Nothing else.
   - The event index and the member tables are used as stored. They are not rebuilt from objects.
   - A block is read, checked and decoded the first time a test touches its object. An object no test touches is never
     decoded. `Program` therefore gets lazily filled object slots (`OnceLock`, because test codeunits run on worker
     threads). This changes how the engine reaches `Program::objects` and needs an accessor (ADR-011, additive).
   - The format code is a core crate over bytes (ADR-006). The file access (read, rename, directory) sits next to
     the existing download cache in `al-deps/src/cache.rs`.

4. **The key** has five inputs. Every one must be in it, and a test proves each one misses when it changes:
   1. app id and 2. app version;
   3. a hash of the app's source. For an artifact the hash is made once when the artifact is stored (ADR-013
      decision 13 keeps CRC-32 and size per entry). For `.alpackages` and `--deps` files it is a hash of the zip
      central directory (name, size and CRC-32 of every entry), which needs no data read;
   4. the target BC version, with the runtime version (`Target`, ADR-015: gates change lowering);
   5. the runtime build id. Any compiler change must invalidate. Decided here: the hash of the running executable
      (it cannot miss a crate; a hash of source files can). The measurement section times it; if it costs too
      much, the build script hashes all workspace sources and `Cargo.lock` instead.

   The entry of an app also takes the keys of the entries of its dependencies, because its lowering was checked
   against their member tables. A dependency hotfix therefore rebuilds the apps that depend on it, and nothing else.
   The format integer is part of the key as well; a mismatch is a miss and rebuilds, never an error (ADR-016).

   **No input can be forgotten.** The key is one struct. The hash function destructures it field by field without
   `..`, so adding a field that is not hashed does not compile. The install-baseline key (if built) takes the keys of
   every entry in the set plus the test-data and normalization inputs, built the same way.

5. **Writing is safe and local.**
   - Write to a temporary file in the cache folder, then rename. A reader sees the whole old file or the whole new
     file. Two parallel runs may both build the same entry; the last rename wins, and the bytes are the same
     (the build is deterministic, a test checks it).
   - A damaged file is refused and rebuilt, never used: the header and index checksum are checked at open, a block
     checksum at first touch. A failed check stops the run with a named error (not a test failure and not a pass),
     deletes the entry, and the next run rebuilds. `alrunner cache verify` reads every block, for CI.
   - **Licensing (ADR-013, "cache only"):** the cache is a local file made on the user's machine from source the
     user downloaded. Nothing compiled from Microsoft code is committed, released, uploaded as a release asset or
     shared outside the user's own CI cache.
   - Location: open question 3.

6. **CI** caches the cache folder between pipeline runs with `actions/cache`.
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

## Measurement before building

A throwaway spike, an `examples/` program in `al-test-cli` next to `phases` and `compile_dump`. It serializes the
lowered program of a full corpus run with Microsoft's apps and reads it back. It replaces these estimates (owner,
unmeasured): load 50-150 ms for about 35 000 reached routines; corpus run about 1 s; single test about 0.3 s;
32 buckets 6-8 s.

Measure, with the load average noted (`flock /tmp/al-perf.lock`):

1. Size of the serialized program, raw, per-object zstd, per-object LZ4, whole-file zstd. Per-object overhead.
2. Time to read it all back (full decode), and time for lazy decode of only the objects that run reached.
3. Time to open (header + index), with and without the stored event index and member tables, against rebuilding them.
4. The eager compile time for Base Application and for the full Microsoft stack (estimate: 10-30 s per Base App
   version), and the peak memory of it.
5. Peak memory of a cached run against an uncached one.
6. The cost of the executable hash and of the central-directory hash (decision 4).
7. Whether IR refers to fields by field number or by position in a table layout that other apps' extensions change.
   If by position, the entry cannot be shared across projects until that changes. This decides whether decision 2
   works as written.

**Go / no-go.** Build only if a warm cached run with Microsoft's apps is clearly faster than the best uncached run
after the performance work, on the corpus and on one bucket run. If the uncached run is already near the cached
one, reject this ADR. The owner's rule stands: never add a cache to hit a number the work itself could reach.

## Open questions for the project owner

Each is one decision. The recommendation comes first.

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

## Owner decisions (2026-10-14)

The owner accepted every recommendation above:

1. Blocks are serialized with serde + postcard (or bincode) and compressed per object with zstd.
2. The install-baseline snapshot is not in version 1.
3. The cache lives in `<cache root>/compile/`, under the root the download cache already uses.
4. WebAssembly has no compile cache in version 1.

## Consequences

- **IR serialization becomes a seam.** The format is versioned by an integer in the header and the key. Changing an
  IR type changes the runtime build id and so invalidates all entries; no migration code is needed.
- **`Program` changes shape** (lazy object slots, entry-local indexes) and the compile pipeline must produce output
  per app. Today `Program` is one list for all apps. Additive, but not small.
- **First run is slow.** Building the entries is the eager compile of the whole closure: estimated 10-30 s per Base
  App version, to be measured. The runner prints one line to stderr saying it is building the cache once. Later
  runs, and every CI run after the cache-build step, do not pay it.
- **Memory:** the builder holds an eager compile in memory once. A cached run holds only the index and decoded
  objects, which may be below today's demand compile; the spike measures it.
- **Disk:** an entry per app version, left behind after each hotfix. `alrunner cache prune` and `alrunner doctor`
  (ADR-016) report and remove it.
- **Two paths stay.** The demand-driven compile remains (cache off, WebAssembly, differential), so both are
  maintained. The cached path adds no new behaviour: it must give the outcomes of the uncached path (ADR-009).
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
- **Sharing.** Two projects with different own extensions load the same Base Application entry without rewriting it.
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
