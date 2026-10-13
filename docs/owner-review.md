# Decisions taken without the owner

Each line is a decision an agent took while the owner was away, with the ADR section that
records the options. Review them and change any that you disagree with.

| Date | ADR | Decision |
|---|---|---|
| 2026-10-13 | 013 | `SessionId()` and `ServiceInstanceId()` answer 1 (positive), not 0, because codeunit 60023 asserts `> 0`. |
| 2026-10-13 | 013 | The host clock is a trait: frozen at 2024-01-15T12:00:00Z in the deterministic host, the system clock on the command line; `Sleep` moves it forward without waiting. |
| 2026-10-13 | 013 | The session time zone is UTC (AL Runner uses the machine's zone); `SessionSettings.Init` answers `UTC`. |
| 2026-10-13 | 013 | `WorkDate()` is today until set, per session; a new test codeunit starts at today again. |
| 2026-10-13 | 013 | `CurrentClientType()` is `Background` and `DefaultClientType()` is `Web`. |
| 2026-10-13 | 013 | `CompanyName()` is `My Company` instead of the placeholder `CRONUS`. |
| 2026-10-13 | 013 | Temporary records are not stamped with the system created/modified fields. |
