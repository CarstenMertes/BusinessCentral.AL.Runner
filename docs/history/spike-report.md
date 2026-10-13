# End-of-spike report

The brief (`spike-brief.md`) asked one question: *does the corpus pass rate
climb steadily while the architecture holds, or do agents plateau on
undocumented semantics?*

**Answer: it climbs, and the architecture held. Recommendation: Go.** The
project continues as a product, a Rust-native AL runtime.

## Pass-rate curve (BC 27.5 oracle, 4836 cases, 0 failing at every step)

| Step | Pass |
|---|---:|
| First corpus tests run | 53 |
| Records, triggers, install codeunits | 87 |
| Honest refusals introduced (silent wrong behaviour turned into "not run") | 78 |
| Collections, SetFilter, guids | 110 |
| Dialog handlers | 113 |
| FlowFields, CalcFields, AutoIncrement, Truncate | 192 |
| Events: publishers, subscribers, table events, binding | 885 |
| JSON, XML, Evaluate, streams, text (and the perf rewrite) | 1308 |
| RecordRef, FieldRef, RecordId, relations, rename propagation | 1716 |
| TestPage runtime (ADR-012) | 2412 |

The pass count dipped once on purpose, from 87 to 78. That is when guessed
behaviour was replaced by explicit refusals, and the rate has never been
bought with wrong answers since. Each step after 113 was one area built in
parallel by its own agent, in its own worktree, against the same ratchet.

## The brief's milestones

| Milestone | Brief's bar | Result |
|---|---|---|
| M0 Harness | Every corpus case reported, pass rate in CI | Done |
| M1 Front end | Lossless parse of every corpus file; fixtures type-check | Done |
| M2 Pure semantics | types, text, collections, codeunit, error-handling | 72% (644 / 892); the rest is mostly guarded `Codeunit.Run`, TransactionModel, a few builtins |
| M3 Record engine | record, recordref, fieldref | 56% (700 / 1252); the rest is mostly Base App tables and Global Triggers |
| M4 Stretch | json, xml, streams, session | 47% (321 / 681); json, xml and streams are above 90%; session is blocked by system tables |

Areas the brief put out of scope are now partly done: handlers and TestPage
(497 of 1115) and page teardown and close (139 of 141).

## Where agents got stuck, and why

- **Not on undocumented semantics.** The corpus pins far more behaviour than
  the documentation does, and every implemented behaviour cites a corpus test.
  Where nothing pinned a behaviour, agents refused it or recorded the
  assumption (about 135 entries in `docs/probes-needed.md`). They did not
  guess.
- **On objects outside the workspace.** About 550 tests need Microsoft's apps
  or platform system tables. That is a scope boundary, not a plateau, and
  ADR-013 addresses it.
- **On accidental passes.** A few tests passed for the wrong reason, such as
  an unseeded store or comparing empty timestamps. The ratchet process caught
  them, and they were moved to "not run".
- **On process, not semantics.** Merge conflicts between parallel areas, one
  hung shell command, and red/green slipping under time pressure in a few
  places (`docs/status.md`, "Known issues").

## Performance

The brief's budget was a cold compile of the corpus within 1 s. It went from
1.4–1.8 s to about 0.5 s (issue #1); the main cause was a quadratic lexer.
A full corpus run takes about 1 s; the brief allowed 5 s.

## Next

See `docs/status.md`: ADR-013 (real dependency apps), TestPage parts, page
extensions, request pages, queries, TransactionModel and guarded
`Codeunit.Run`.
