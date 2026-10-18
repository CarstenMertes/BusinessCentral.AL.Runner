# ADR-045: The test clock moves forward on every read

- Status: Accepted (owner, 2026-10-18)
- Date: 2026-10-18
- Amends: ADR-013 decision 9, item 2 (the clock is frozen by default)

## Context

The test clock stood still. `Today`, `Time` and `CurrentDateTime` answered one instant for the whole run, and only `Sleep(n)` moved it. The reason
was repeatability: a clock that follows the wall makes an outcome depend on machine load (a Base Application subscriber compares
`Format(xRec) <> Format(Rec)` after `Item.OnModify` sets "Last Time Modified" to `Time`).

Real BC never gives a test a predictable time between two reads, so a test can only rely on "later than". A frozen clock breaks
code that waits for time to pass without calling `Sleep` (retry loops that stop when `CurrentDateTime` passes a deadline; a partner
app's return-retry test expected three retries and got one). Owner decision: the fake clock moves forward by a fixed small step on
every read, and by n milliseconds on `Sleep(n)`.

## Decision

1. **One source.** The session keeps the clock: the host's instant (`Clock::now_utc_millis`, fixed at the first read of a run) plus a
   counter of milliseconds the session has moved it. `Today`, `Time`, `CurrentDateTime`, the .NET `DateTime.Now/UtcNow/Today` and
   `DateTimeOffset.Now/UtcNow` all read that one value.
2. **Step: 1 millisecond.** `DateTime` and `Time` have millisecond resolution, so 1 ms is the smallest step that makes two reads
   differ. A read answers the current instant, then moves the clock on by 1 ms. The first read of a session is therefore the host
   instant unchanged (the deterministic host still starts at 2024-01-15T12:00:00Z).
3. **`Sleep(n)`** adds n ms to the same counter and does not wait. `Sleep(0)` moves nothing. A negative n stays refused (not pinned).
   Between two reads around `Sleep(n)` the difference is n + 1 ms.
4. **What does not move the clock.** Internal readers: system stamps (`SystemCreatedAt` and friends, still strictly increasing by
   themselves), the default `WorkDate`, record link creation time, report and page header times. They read the current instant
   without stepping, so a stamp written between two AL reads lies between them.
5. **`ALRUNNER_REAL_CLOCK=1` stays.** It makes the host answer the machine clock; the session step and `Sleep` offset still apply on
   top (a 1 ms step per read, negligible against a moving wall clock).
6. **Repeatability is kept.** The result depends only on the number of reads and the sleeps, not on load. The date can still roll
   over during a run, as before.

## Consequences

- Code that loops until time has passed now ends. Two reads never compare equal; `A = B` between two reads is no longer true.
- `Format(Time)` changes at a second boundary after 1000 reads. The Item workflow case above compares `Format` at seconds; it can
  now see a change that the frozen clock hid. The Microsoft buckets Tests-ERM and Tests-SCM were run before and after (see the pull
  request); changed outcomes are listed there.
- Corpus tests that pin only "later" or "at least n ms" pass unchanged.

## Conformance

- Corpus pins that must still pass (all rely on order, not on equality or exact differences):
  `session/W5SessionFacts.al:37-40` (and :52, Sleep moves the clock by at least n), `session/TestProbeSessionMiscContracts.al:784-786`,
  `session/TestSessionFunctions.al:39-65`, `record/TestSystemFieldStampsContracts.al:60-97`, `record/TestRecordLinkColumns.al:104-106`,
  `record/TestRecordSystemId.al:510-764`, `record/TestXRecStampFields.al:168-179`, `system-fields/W6SystemFieldStamps.al`.
- `crates/al-vm/tests/store/hostvalues.rs`: `every_read_of_the_clock_is_one_millisecond_later_than_the_last`,
  `sleep_moves_the_clock_by_the_time_waited`, `sleep_of_zero_is_allowed_and_a_negative_wait_is_refused`.
- `crates/al-vm/tests/dotnet.rs`: `datetime_now_is_the_host_clock_and_needs_no_dotnet_host`.
