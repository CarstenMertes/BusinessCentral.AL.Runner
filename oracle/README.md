# Oracle

`oracle.toml` lists one `[[version]]` table per BC major (ADR-015). Each table
points to the JUnit report the corpus CI produced against a real BC service
tier, and to the ratchet for that major. The JUnit is the ground truth the
pass rate is measured against: every `<testcase>` in it passed on real BC.

| Field | Value |
|---|---|
| Corpus repo | StefanMaron/BusinessCentral.AL.Language.Tests |
| Corpus commit | `cf6e546793a92c482499ce86eadf0a0b13d2a742` (must equal the `corpus` submodule pin) |
| Workflow run | 38306086475 (`ci.yml`, push to master) |

Layout:

```text
oracle/oracle.toml        the pin and the [[version]] tables
oracle/<major>/junit.xml  JUnit of the newest minor of that major (the oracle leg)
metrics/<major>/passing.txt  the ratchet for that major
```

The oracle leg of a major is its newest minor: 27.5, 28.5, 29.0. Every JUnit
file must come from the CI run of the pinned corpus commit. The runner of the
28.x and 29.0 legs adds an `OnRun` pseudo-case per test codeunit; the reader
skips them (ADR-015 1.6).

To bump: move the submodule, download the matching run's
`junit-test-results-<version>` artifact **by artifact id** (two artifacts share
each name), copy it unchanged to `oracle/<major>/junit.xml`, and update
`oracle.toml`. Never edit a JUnit file.
