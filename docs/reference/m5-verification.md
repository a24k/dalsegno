# M5 verification

Results for the exit criteria in [roadmap.md](../roadmap.md), from the service recording the
reference stream in production. The run that counts was on 0.5.0-rc.13, from 2026-11-04 13:07 UTC
to 2026-11-07 14:16 UTC, a little over 3 days. v0.5.0 is the same code with a new version number
(D-32).

## Earlier runs

The soak started on rc.3 on 2026-11-03, and the redeploys of rc.4 to rc.11 served as restarts.
Two problems on rc.11 made it start again on rc.13:

| Found | Fix |
| --- | --- |
| A change was opened, confirmed and kept, though its next cycle matched the old version | #45: such a change closes as `dismissed` |
| The origin answered 204 at the head for a fragment it had not made yet, and it was logged as an error | #46: 204 is "not yet" |

## Results

| Criterion | Result |
| --- | --- |
| At least 3 days, with one deliberate restart | 3 days and 1 hour; restarted on 2026-11-06 at 06:58 UTC, back in 14 s |
| No lost fragments | 0 lost, in every daily summary |
| No failed jobs | 154 jobs ran and all finished |
| A stable period | 42,244.9 s throughout |
| The restart's gap was filled | 5 fragments, filled in 13 s with 0 lost; 2 at the first start, the same way |
| No unexpected notification | None; see below |
| No repeated IP family switches | No switch logged |
| No slowdowns | One back-off from the origin, handled by halving the rate for 10 minutes |

The jobs were 73 rounds of features and analysis, one each hour, 4 sweeps, 2 backfills and 2
prefetches. Features took about 11 s and analysis under 1 s, or about 6 s when it closed a change.

The log had no error and two warnings: the back-off above, and the start notification after the
restart, which failed once and arrived on the retry a minute later.

## Notifications

| Notification | Count |
| --- | --- |
| Service started | 2 |
| Daily summary | 3 |
| Change seen | 2 |
| Change dismissed | 3 |
| DVR deadline | 1 |

All three changes were false signals on an unchanged loop, one of them opened on rc.11, and each
closed as `dismissed` one cycle later, as #45 intended. They open about once a day, at loop
positions with nothing in common. Why tracking raises them is #49, in milestone 7.

The deadline reminder was for the change confirmed on rc.11, which stays confirmed. It is expected,
and it does no harm.

## Not exercised

The stream did not change during the run, so "no missed change" was not tested in production. The
change lifecycle is covered by the tests in `crates/app/src/monitor.rs` and by the dismissed changes
above.
