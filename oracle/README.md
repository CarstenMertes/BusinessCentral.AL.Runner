# Oracle

`oracle.toml` lists one `[[version]]` table per BC major (ADR-015). Each table
points to the JUnit report the corpus CI produced against a real BC service
tier, and to the ratchet for that major. The JUnit is the ground truth the
pass rate is measured against: every `<testcase>` in it passed on real BC.

| Field | Value |
|---|---|
| Corpus repo | StefanMaron/BusinessCentral.AL.Language.Tests, branch `al-runtime-rs` |
| Corpus pin | `348a7441c3b4be2dfad3e570e83d5bdc4bfad2cb` (the `corpus` submodule; must equal `corpus_commit` in `oracle.toml`) |

| Major | Oracle leg | Corpus commit of the run | Workflow run | Artifact | Artifact id | Cases |
|---|---|---|---|---|---|---|
| 27 | 27.5 | `348a744` (the pin) | 38511560267 | `junit-test-results-27.5` | 11870821886 | 5112 |
| 28 | 28.5 | `348a744` (the pin) | 38512914332 | `junit-test-results-28.5` | 11870949831 | 5112 (+26 `OnRun`) |
| 29 | 29.0 | `348a744` (the pin) | 38514656113 | `junit-test-results-29.0` | 11872093217 | 5112 (+26 `OnRun`) |

Every file has 0 failures. Each run published two artifacts with the same name; the
oracle is the large one (the main suite, `junit-al-language.xml`). The small one is
the OnPrem suite and is not used.

All three legs come from one corpus commit. The corpus workflow now defines `BC29PLUS` on
dispatch, so the 29.0 run on the pin is usable. The CLI still accepts a `corpus_commit` inside a
`[[version]]` table for an older commit, when the corpus diff since then adds files under
`tests/` and changes none (ADR-007). No table uses it today.

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
