# Release performance baseline

Measured 2026-09-27 at commit `08a5866`, using
`cargo build --release --locked -p ferese -p ferese-shell -p feresectl`.

Two disposable nested sessions used separate runtime/config directories, default
configuration, the bundled wallpaper, and the release shell. Performance logging
was enabled with `FERESE_TRACE_PERFORMANCE=1`. The nested output reported
1422 × 1696 physical pixels, 2× scale and 60 Hz. The parent DRM output was a
2880 × 1800 eDP panel at 120 Hz and 2× scale.

## Measurements

CPU is process CPU time from `/proc/PID/stat` divided by elapsed monotonic time,
expressed as a percentage of one core. First-run samples lasted 10 seconds;
second-run samples lasted 5 seconds. These are short observational samples,
not statistically established improvements or before/after comparisons.

| Scenario | Compositor CPU, run 1 / run 2 | Shell CPU, run 1 / run 2 |
| --- | --- | --- |
| Idle shell, no application window | 5.3% / 5.8% | 1.5% / 1.6% |
| Overview with one terminal | 18.6% / 16.2% | 1.3% / 1.4% |
| After window transitions, terminal still open | 6.6% / 6.4% | 1.5% / 1.4% |

Compositor RSS was approximately 81–82 MiB initially and 91–92 MiB after the
short exercise. This does not establish a leak or long-term memory stability.
Main-thread voluntary context switches were approximately 162–165/s at idle
and 498–505/s in overview; these are scheduling proxies, not hardware wakeups.

A five-second idle log interval contained 60 damaged frames and 531 no-damage
attempts, approximately 118 attempts/s. A later overview interval contained
591 damaged frames in 5001 ms. Nested frame scheduling and overview damage
therefore deserve investigation first. The parent runs at 120 Hz, so the
mismatch with the nested output's reported 60 Hz must be understood before
attributing these counts to duplicate scheduling.

## Behavioral exercise

Both sessions exited with status 0. The probes created a terminal, entered and
left overview, and toggled maximize, fullscreen and floating twice each.
The second session exercised `theme.typography.font_family`, wallpaper fit/fill,
and replacement image paths through filesystem-driven reloads. Logs confirmed
accepted reloads and replacement wallpaper decoding. This verifies execution
and liveness, not pixel-perfect appearance or GPU allocation-failure recovery.
The first probe used an incorrect font key; only the second probe counts as
font reload coverage.

Raw local artifacts:

- `/tmp/ferese-profile-7cbrpopn/{results.json,session.log}`
- `/tmp/ferese-profile-h2ypepn6/{results.json,session.log}`
- Probe scripts: `/tmp/ferese-profile.py` and `/tmp/ferese-profile-reloads.py`

## Limits and next work

The installed DRM compositor is release `20260927-042025-168287`; its binary hash
differs from the tested release build. A read-only host sample measured 9.8%
compositor CPU and 1.7% shell CPU, but overlapped the nested workload and is not
an idle baseline or validation of the new DRM code. No session was replaced.
Only one monitor was connected, so multi-monitor behavior remains untested.

Next investigate nested refresh scheduling and repeated overview damage, using
these scenarios for before/after measurements. Preserve client frame callbacks,
animation pacing and idle responsiveness. Status command polling remains a
secondary target; blur invalidation needs a separate translucent-material
workload before ranking its cost. New-build DRM and multi-monitor validation
remain necessary before claiming hardware performance improvements.

## Follow-up investigation

A repeat baseline (`/tmp/ferese-profile-n8437l1i`) confirmed 60 Hz after startup
but ran near 60 attempts/s rather than 118. Compositor CPU was 3.0% idle, 9.2%
in overview and 4.6% after transitions. The earlier high-rate behavior is not
consistently reproduced, so it is not sufficient evidence of duplicate redraws.
The candidate refresh-period guard was discarded after testing.

The guard run (`/tmp/ferese-profile-llcrkvm5`) measured 4.0% idle, 8.6% overview,
and 3.6% after transitions. Render attempts fell slightly below the nominal
cadence. This did not establish a benefit, so no frame-pacing change was kept.
The next investigation is unconditional thumbnail damage in overview.

## Broader profiling (visible preview)

Artifacts: `/tmp/ferese-profile-all-0lhodit4/`; script:
`/tmp/ferese-profile-all.py`. This run used the unchanged release compositor,
release shell and release Settings. The user kept the preview visible after an
earlier hidden-window run proved that host occlusion invalidated render samples.
Six-second samples are exploratory, not statistically stable benchmarks.
The captured output was 2856 × 1696 pixels; the preview was resized during the
session, so these workloads must not be compared as if output size were fixed.

| Scenario | Compositor CPU | Shell CPU | Shell waited-child CPU | Settings CPU |
| --- | ---: | ---: | ---: | ---: |
| Idle shell | 4.33% | 1.33% | 7.50% | — |
| Three terminals | 6.67% | 1.50% | 11.33% | — |
| Overview, three terminals | 17.50% | 1.17% | 8.17% | — |
| Settings open, idle | 6.17% | 1.50% | 10.83% | 0.00% |
| After capture/window transitions | 8.50% | 1.33% | 11.00% | 2.50% |

Percentages use one CPU core as 100%. Child CPU is Linux's accumulated CPU time
for reaped children, separate from the shell process. This includes the timing
wrapper: a PATH-local `timeout` wrapper invokes GNU time and the real timeout
command. Read-only PipeWire queries use the host runtime socket; no status
control actions were sent. The native installed shell cross-check, without
instrumentation, measured 2.0% self CPU plus 8.8% waited-child CPU over ten
seconds. It is an older binary and a different scene, so this corroborates the
existence of subprocess overhead rather than providing an exact comparison.

The preview recorded 273 successful status command invocations across 21 polls:
13 per poll. The three login1 capability calls consumed 1.88 seconds total wall
time, about 90 ms/poll. Three nmcli queries consumed 1.35 seconds total; two wpctl
queries consumed 0.80 seconds. GNU time's hundredth-second granularity makes
per-command CPU totals unsuitable for precise attribution. Stable capability
queries, device metadata and frequently changing values currently share one
poll cycle. Separate refresh policies or service subscriptions deserve priority.

Settings mapped in 109 ms, measured from process launch until IPC reported it
focused (not first completed paint). Idle Settings RSS was 108068 KiB, with 38
threads and 65 descriptors. Zero measured CPU over six seconds means below this
sample's resolution, not a guarantee of zero work. Startup font enumeration was
not isolated separately. Settings interaction/search/page-switch profiling is
still outstanding.

Twenty `feresectl get-outputs` calls had median process-inclusive latency of
3.05 ms and maximum 3.95 ms. Five captures took 1.13 seconds total, including
client startup, transfer and PNG encoding. Capture was enabled only in the
isolated compositor with `FERESE_ENABLE_SCREENCOPY=1`. The first screenshot was
visually inspected and showed the shell and Settings rendered. It does not
measure GPU readback independently. Window maximize/fullscreen/layout toggles
completed, but there was no instrumented input-to-photon latency measurement.

The four-second visible overview perf recording (`cpu-clock:u`, 999 Hz) collected
471 samples with no lost samples. Shared-object attribution: 47.77% Ferese
executable (including statically linked dependencies), 30.36% Mesa Gallium,
14.23% libc, 2.97% Wayland client library. Approximately 96.4% of samples were in
the main compositor thread. Texture upload/memory-copy paths appear in the
sampled stacks. This is user-space CPU sampling, not GPU timing; short samples
and incomplete frame-pointer unwinding limit function-level attribution.

The earlier broad run (`/tmp/ferese-profile-all-boiq_r30`) had a 37-second render
reporting gap while hidden, no usable perf samples, and a disabled screencopy
protocol. Its render/overview and capture results are excluded. Its bare idle
sample was 1.0% CPU, but visibility differences preclude subtracting it from the
visible shell run.

Next priority: shell status subprocesses. Then investigate sustained overview
damage and texture uploads with controlled window visibility and dimensions.
Control-center interaction, widgets, translucent-material workloads, long-term
memory growth, new-build DRM and multi-monitor runs remain unprofiled. No source
changes or profiling-note commits were made during this profiling pass.

## Status-worker optimization comparison

Compared the production status module at `08a5866` with the new single-command
brightness reader and reusable native D-Bus connections. A standalone harness
started `Service::start(None)`, consumed updates every 250 ms, and ran for 20
seconds without a UI. Both variants used the same host runtime/services and
command timing wrapper. Artifacts: `/tmp/ferese-status-compare/`.

Both completed nine polls. Notifications were available in this comparison, so
the original worker launched 15 commands/poll and the new one launched 10:
135 versus 90 invocations. Four busctl launches and one brightnessctl launch per
poll were removed. When notifications are absent, the equivalent counts are
13 versus 8. Poll frequency, fresh capability queries and post-action generation
handling remain unchanged; no status-value caching was introduced.

GNU time measured total user+system CPU, including waited children: 2.41 seconds
before, 1.90 seconds after over 20.01 seconds each (about 21% lower in this single
comparison). Timing-wrapper overhead is included, so this is not a precise
uninstrumented CPU improvement estimate. Native D-Bus adds persistent sockets
and a runtime thread pool; UI samples observed roughly 26–28 shell threads and
48 descriptors versus roughly 18 threads and 42 descriptors previously.

One UI run showed a transient self-CPU spike; a subsequent perf recording
identified tiny-skia raster work rather than D-Bus queries among its hot paths.
The UI-free comparison avoids treating variable UI activity as polling cost.

Validation: workspace suite and explicit private-dbus-daemon test passed. The
private bus exercised live capability changes, yes/challenge/no/na handling,
notification-owner appearance/disappearance, error replies, missing methods,
and a reply delayed past the configured timeout. Brightness parser tests cover
rounding, zero brightness, invalid classes and malformed/nonfinite values.

### Push status delivery measurement (b0583eb)

Compared release binaries for 4bf0757 and b0583eb with the same nested compositor,
empty isolated config and a visible preview. Binary SHA-256 hashes differ and
were checked after forcing a shell rebuild to avoid shared-target artifact reuse.
No status-command timing wrappers were active. Discarded an initial baseline
that overlapped compilation. Retained after/before/after runs, each with three
10-second samples following a three-second startup settle. Artifacts and harness:
`/tmp/ferese-status-push-measure/` (`summary.json`, `ui.py`, `latency.rs`).

Shell CPU percentages of one core:
- Before: 1.5, 1.4, 1.6 (median 1.5).
- After run 1: 0.8, 1.1, 1.1.
- After run 2: 1.0, 1.1, 7.5 (combined after median 1.1).

The 7.5% sample coincided with a large damaged-pixel burst (44.3 million in a
five-second interval), higher RSS and two additional descriptors. Its cause was
not isolated; it is retained, not silently excluded. Combined after mean is
2.1%, versus 1.5% before, so these short runs do not establish a robust average
CPU saving. Typical steady-state samples suggest a 0.4 percentage-point reduction.
Status child CPU remained roughly 6–8% of one core, dominating shell self CPU.

Steady-state damaged frames were about 12/s before versus 6.6–7.6/s in the first
after run. The repeat had similar quiet intervals plus bursts at 15.6 and 22.7/s.
Total render attempts stayed around 60/s, confirming active rather than hidden
preview sampling. No claim is made about DRM-session CPU or total idle wakeups.

An isolated transport harness used the old bounded mpsc + 250ms polling path and
an extracted copy of the new Updates::stream implementation, with 40 synthetic
updates per path at 317ms intervals. Worker-to-consumer delay:
- Before: median 129.398ms, p95 239.957ms, maximum 245.881ms.
- After: median 0.130ms, p95 0.330ms, maximum 0.425ms.
This measures delivery only, excluding status queries, UI dispatch and painting.
The preview processes exited and the temporary source worktree was removed.
