# al-runtime-rs

A time-boxed spike: can agents build a Rust runtime that executes AL with real
fidelity, measured against the
[AL language test corpus](https://github.com/StefanMaron/BusinessCentral.AL.Language.Tests)?

Tier 1 only: AL language semantics plus an in-memory record engine. No Base App.

```
cargo test --workspace                 # unit + contract + harness tests
cargo run -p al-test-cli -- --markdown pass-rate.md   # corpus pass rate vs BC 27.5
```

Clone with `--recurse-submodules`; the corpus is a pinned submodule.

- `docs/handoff.md` — the spike brief
- `docs/adr/` — seam decisions; change only through a new ADR
- `docs/corpus-inventory.md` — what the corpus needs, per area
- `oracle/` — real-BC results the pass rate is measured against
- `metrics/passing.txt` — the ratchet: cases that must keep passing
