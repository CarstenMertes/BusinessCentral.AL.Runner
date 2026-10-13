# Oracle

`oracle.toml` lists one `[[version]]` table per BC major (ADR-015). Each table
points to the JUnit report the corpus CI produced against a real BC service
tier, and to the ratchet for that major. The JUnit is the ground truth the
pass rate is measured against: every `<testcase>` in it passed on real BC.

| Field | Value |
|---|---|
| Corpus repo | StefanMaron/BusinessCentral.AL.Language.Tests, branch `al-runtime-rs` |
| Corpus pin | `cdf315abd1e7115fcbfbcc88c720371e75989b8e` (the `corpus` submodule; must equal `corpus_commit` in `oracle.toml`) |

| Major | Oracle leg | Corpus commit of the run | Workflow run | Artifact | Artifact id | Cases |
|---|---|---|---|---|---|---|
| 27 | 27.5 | `cdf315a` (the pin) | 38476427517 | `junit-test-results-27.5` | 11856955675 | 5059 |
| 28 | 28.5 | `cdf315a` (the pin) | 38480686379 | `junit-test-results-28.5` | 11858881269 | 5059 (+26 `OnRun`) |
| 29 | 29.0 | `cf6e546793a9` (older) | 38306086475 | `junit-test-results-29.0` | 11773034371 | 4836 (+26 `OnRun`) |

Every file has 0 failures. Each run published two artifacts with the same name; the
oracle is the large one (the main suite, `junit-al-language.xml`). The small one is
the OnPrem suite and is not used.

BC 29 stays on the older commit because the 29.0 run on the pin is unusable: the
corpus workflow's dispatch path does not define `BC29PLUS`, so four guarded tests
failed (`docs/owner-review.md`). The CLI accepts an oracle for an older commit only
when the corpus diff since then adds files under `tests/` and changes none (ADR-007).
The 223 cases the pin added are then absent from the BC 29 report and ratchet.

Layout:

```text
oracle/oracle.toml        the pin and the [[version]] tables
oracle/<major>/junit.xml  JUnit of the newest minor of that major (the oracle leg)
metrics/<major>/passing.txt  the ratchet for that major
```

The oracle leg of a major is its newest minor: 27.5, 28.5, 29.0. Every JUnit
file must come from the CI run of the pinned corpus commit, or of the older commit its
`[[version]]` table names (ADR-007). The runner of the
28.x and 29.0 legs adds an `OnRun` pseudo-case per test codeunit; the reader
skips them (ADR-015 1.6).

To bump: move the submodule, download the matching run's
`junit-test-results-<version>` artifact **by artifact id** (two artifacts share
each name), copy it unchanged to `oracle/<major>/junit.xml`, and update
`oracle.toml`. Never edit a JUnit file.
