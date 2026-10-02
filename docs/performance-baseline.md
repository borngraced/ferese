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

## Direct frame timing

The DRM backend schedules each output against its own predicted presentation
time. Render cost and a safety margin determine when compositing starts. Idle
outputs wake on demand; callback-only updates are paced to the output refresh
cycle without submitting unchanged buffers. Animation forecasts are restored
before processing input or rendering another output.

For a direct session, enable `FERESE_TRACE_PERFORMANCE=1` and
`RUST_LOG=ferese::render=debug` before starting Ferese. The timing records include
the planned render start, actual render start, timer lateness, presentation
target, DRM presentation timestamp, and request-to-presentation duration.
Missed deadlines compare submitted frames with their targets; idle gaps do not
count. Request-to-presentation starts at the compositor's redraw request, so it
does not include device input latency, client rendering before the request, or
the display's pixel response. Render cost measures CPU preparation and queueing;
asynchronous GPU work is covered conservatively by feedback-driven margins.

Check sustained animation, idle wakeup, callback-only clients, and mixed-refresh
outputs separately. Nested sessions do not exercise the DRM scheduler.

## Redraw and animation fallback counters

With the same environment variables, direct sessions report `redraw requests`
by source file and line, with call counts, requested outputs and connected outputs.
`render performance` adds per-output redraw requests and counts requests made
while a render timer was scheduled or a frame was in flight. Compare these with
`frames`, `no_damage_frames`, render time and missed deadlines; requests can
coalesce and are not themselves rendered frames.

`animation fallback scheduling` reports watchdog arms, cancellations, wakeups,
recovery requests and observations of animations with an intact normal frame
chain. Summaries are emitted during redraw activity every five seconds and on
normal exit. The fallback counters reset with each summary. An orphaned animation
gets a watchdog deadline two refresh intervals after its own output loses its
frame chain. Recovery also retains a requested final frame if another output
settles that animation while scheduling is unavailable. Idle, powered-off and
disconnected outputs do not set that deadline.
An already scheduled render or pending page flip suppresses the watchdog; this
policy does not replace DRM error recovery or diagnose a permanently lost flip.

The regression replays use a 60 Hz animation beside an idle 240 Hz output.
The old fallback rule produces 60 unnecessary wakes in one simulated second;
the new rule produces zero while the normal chain remains intact. Three cursor
moves within one output require three output requests rather than the old six.
These are deterministic scheduling counts, not live DRM CPU, latency or battery
measurements. Verify those separately with the same hardware workload before
making claims about power savings.

## Settled overview damage

On 2026-10-02, a release-build comparison against `6f2951c` used four parked
foot terminals in a private nested desktop: 1418 × 1692 pixels, 2× scale, 60 Hz,
Intel Core Ultra 7 258V and Mesa 26.2.3. Ten-second process samples measured
5.9% CPU before and 1.9% after stabilizing caption/thumbnail drawing order and
removing forced full damage from unchanged rounded thumbnails.

A settled five-second interval changed from 300 damaged frames to zero.
The nested backend still makes about 300 no-damage render attempts per interval;
this change avoids unchanged submissions rather than changing nested scheduling.
These short samples exclude host compositor CPU and do not measure battery
discharge or direct DRM performance.

The opt-in regression checks a full settled interval, a real client content
commit, and settling again. It fails against the original release.

```sh
FERESE_TEST_OVERVIEW_DAMAGE=1 FERESE_TEST_BINARY=target/release/ferese \
  FERESE_TEST_CTL=target/release/feresectl \
  python3 scripts/tests/test_overview_damage_isolated.py
```

## Idle shell audio polling

On 2026-10-02, the installed release shell was compared with the audio-event
cache in private nested desktops on the same machine. Each release had ten
seconds to settle, followed by three twenty-second samples without `perf`.
CPU includes waited child processes; one core is 100%.

| Measurement | Periodic audio queries | Persistent audio watcher |
| --- | --- | --- |
| Combined CPU, sample range | 2.25–3.15% | 0.90% |
| Combined CPU, median | 3.10% | 0.90% |
| Shell CPU, median | 0.75% | 0.75% |
| Child-process CPU, median | 2.35% | 0.15% |
| Shell RSS | 32.3–32.4 MiB | 34.9 MiB |

A separate twenty-second `perf` sample recorded twenty `wpctl` executions before
and zero afterward. The new connection observes sink properties, device routes,
and default-sink metadata. Relevant events and control actions invalidate the
cached reading; the existing `wpctl` reader preserves volume and device semantics.
Disconnected subscriptions retain bounded polling, and missing sinks retry.
Brightness and other status polling remain unchanged.

These are short nested observations, exclude the host compositor, and do not
measure battery discharge. Raw measurements and profiles were saved locally in
`/tmp/ferese-shell-profile-20261002`.

The subscription regression starts its own PipeWire server, changes default
metadata, mute and volume, removes a sink, restarts the server, and checks shutdown.
It does not change the desktop's audio server.

```sh
cargo test --release --locked -p ferese-shell native_subscription -- --ignored
```

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
