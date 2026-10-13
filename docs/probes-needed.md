# Probes needed

Behaviour implemented from documentation only, because no corpus test pins
it yet. Each entry needs a probe test on a branch of the corpus repo (hard
rule 3, ADR-007); once its CI result is in, the entry moves into the code
comment as a corpus citation and is deleted here.

| Area | What is assumed | Source today | Code |
|---|---|---|---|
| Format | Decimal format 0/1 uses `,` thousands and `.` decimals (en-US), decimals trimmed of trailing zeros | doc table "Standard decimal formats" (US) | `al-builtins/src/format.rs` `format_decimal` |
| Format | Date format 0/1 is `MM/DD/YY`, format 2 `MMDDYY[C]D` (en-US) | doc table "Standard date formats" (US) | `format_date` |
| Format | Decimal format 2 trims trailing zeros (`1.50` → `1.5`) | doc `<Decimals>` field, no precision | `format_decimal` |
| Format | Boolean format 9 is `true`/`false` | doc table "Standard boolean formats" | `format` |
| Error | `Error(Text, ...)` sets `GetLastErrorCode()` to `Dialog` | Microsoft test libraries; corpus Assert `DialogTxt` label | `al-builtins/src/native.rs` `error_text` |
