# Performance measurements

Use release builds and a fixed workload when comparing performance. Record the
commit, output size, scale, refresh rate, graphics driver, and whether the session
is nested or direct. Keep the nested preview visible: host occlusion changes
rendering behavior.

## Collect a sample

```sh
cargo build --release --locked -p ferese -p ferese-shell -p feresectl
FERESE_TRACE_PERFORMANCE=1 target/release/ferese --backend nested \
  --grant-effects --grant-shell-control -- target/release/ferese-shell
```

Compare idle, several windows, overview, Settings navigation, translucent
popovers, and screen capture separately. Allow startup to settle and repeat each
sample. Record process CPU, child-process CPU, RSS, render attempts, and damaged
frames. Keep build work and unrelated activity outside the sampling interval.

CPU percentages below use one core as 100%. Short samples are observations,
not guarantees or hardware-session benchmarks.

## Historical results

On 2026-09-27 at `08a5866`, two nested runs used a 1422 × 1696 output at 2× scale,
reporting 60 Hz on a 120 Hz host. Samples lasted 5–10 seconds.

| Scenario | Compositor CPU, two runs | Shell CPU, two runs |
| --- | --- | --- |
| Idle, no application window | 5.3% / 5.8% | 1.5% / 1.6% |
| Overview with one terminal | 18.6% / 16.2% | 1.3% / 1.4% |
| After transitions | 6.6% / 6.4% | 1.5% / 1.4% |

Compositor RSS increased from about 81–82 MiB to 91–92 MiB during the exercise.
These samples do not establish a leak or long-term stability. A follow-up had
lower rendering rates, so the original observations did not prove duplicate scheduling.

A 20-second status-worker comparison removed five commands per poll (15 to 10).
Total measured CPU time including child processes fell from 2.41 to 1.90 seconds.
The timing wrapper was included, and this was one comparison.

For push status delivery (`4bf0757` to `b0583eb`), a synthetic transport test
measured median delivery delay of 129.398 ms before and 0.130 ms after. It excluded
queries, UI dispatch, and painting. Visible shell samples suggested lower typical
CPU, but an after-run spike raised the mean; they did not establish a robust
average CPU saving.

These results predate later changes. Run fresh comparisons for current claims,
and validate direct rendering and multiple monitors separately.
