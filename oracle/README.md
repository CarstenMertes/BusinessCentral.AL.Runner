# Oracle

`oracle.toml` lists one `[[version]]` table per BC major (ADR-015). Each table
points to the JUnit report the corpus CI produced against a real BC service
tier, and to the ratchet for that major. The JUnit is the ground truth the
pass rate is measured against: every `<testcase>` in it passed on real BC.

| Field | Value |
|---|---|
| Corpus repo | StefanMaron/BusinessCentral.AL.Language.Tests, branch `al-runtime-rs` |
| Corpus pin | `faf1c152e9026deea4f0420b33594bd9273374e3` (the `corpus` submodule; must equal `corpus_commit` in `oracle.toml`) |

| Major | Oracle leg | Corpus commit of the run | Workflow run | Artifact | Artifact id | Cases |
|---|---|---|---|---|---|---|
| 27 | 27.5 | `faf1c15` (the pin) | 38863476839 | `junit-test-results-27.5` | 12031675897 | 10861 |
| 28 | 28.5 | `faf1c15` (the pin) | 38863476839 | `junit-test-results-28.5` | 12031084931 | 10876 (+26 `OnRun`) |
| 29 | 29.0 | `faf1c15` (the pin) | 38863476839 | `junit-test-results-29.0` | 12031661922 | 10896 (+26 `OnRun`) |

Every file has 0 failures. Each run published two artifacts with the same name; the
oracle is the large one (the main suite, `junit-al-language.xml`). The small one is
the OnPrem suite (`junit-al-language-onprem.xml`, 72 cases): it is the oracle of the
`onprem` test app below.

## Other test apps

`[[version.suite]]` tables in `oracle.toml` add a test app with its own oracle file, its own
ratchet (`metrics/<major>/passing-<name>.txt`) and its own report line. They are not part of the
overall count.

| Name | App | Oracle file | From | Cases |
|---|---|---|---|---|
| `onprem` (27, 28, 29) | `tests/al-language-onprem` | `<major>/junit-onprem.xml` | the small artifact of the same CI run as the main oracle (ids 11957600568, 11957362118, 11956862735), copied unchanged | 72 |
| `isolation-function` (28 only) | `tests/al-language-isolation-probe` | `28/xunit-isolation-function.xml` | the Windows nightly of 2026-10-13, BC 28.4, run 38341042967, artifact 11788517730 | 7 |
| `isolation-codeunit` (28 only) | the same app | `28/xunit-isolation-codeunit.xml` | the same artifact | 7 (T5 fails on BC, on purpose) |

The isolation probe never runs in the Linux CI legs: it needs two custom test runners
(`-testRunnerCodeunitId`), which only the Windows nightly provides. Its files are BcContainerHelper
XUnit, not JUnit, and come from the nightly on corpus `master` at `cf6e546`, whose probe app is
identical to the pin's (the CLI checks the corpus diff, ADR-007). There is no isolation oracle for
BC 27 and 29, and the leg is 28.4, not 28.5. The control suite fails `T5` on real BC; the report
shows that the runtime fails it too.

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
each name), copy it unchanged to `oracle/<major>/junit.xml` (the large one) and
`oracle/<major>/junit-onprem.xml` (the small one), and update `oracle.toml`. Never edit a
JUnit or XUnit file.
