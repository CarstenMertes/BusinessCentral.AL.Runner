# Oracle baseline

`junit-27.5.xml` is the JUnit report the corpus CI produced against a real BC
27.5 service tier. It is the ground truth the runtime's pass rate is measured
against: every `<testcase>` in it passed on real BC.

| Field | Value |
|---|---|
| Corpus repo | StefanMaron/BusinessCentral.AL.Language.Tests |
| Corpus commit | `cf6e546793a92c482499ce86eadf0a0b13d2a742` (must equal the `corpus` submodule pin) |
| Workflow run | 38306086475 (`ci.yml`, push to master) |
| Artifact | `junit-test-results-27.5` (id 11772863083) |
| Tests / failures | 4836 / 0 |

`oracle.toml` holds the same commit in machine-readable form; a test in
`al-test-cli` fails if it drifts from the submodule pin.

To bump: move the submodule, download the matching run's
`junit-test-results-27.5` artifact, replace the XML, update both files.
