# ADR 0001: libcosmic for the Ferese shell

Status: provisional, pending the M8 lifecycle spike
Date: 2026-09-25

## Decision

Use libcosmic for the Ferese shell UI if the M8 lifecycle spike passes. Pin the
dependency to commit `03d7dcb832bc6d15bc1bb07327a1aa426b2d5bc0` until a suitable
stable release exists.

Keep the spike in `tools/ferese-shell-spike`; it is not the production shell.
The production shell will continue to use public layer-shell for surface roles
and Ferese-private protocols only for control and semantic effects.

## Why

libcosmic is a Rust-native toolkit designed for Wayland desktop components. Its
current API can create layer-shell surfaces without an ordinary xdg-toplevel,
select an active or explicit output, request keyboard interactivity, observe
configure/focus/output events, and destroy surfaces through the runtime.

The previous GPUI direction depended on an older release and did not provide a
convincing first-class path for Ferese shell surfaces.

## Constraints found during source review

- The selected revision declares Rust 1.93, so the isolated spike overrides the
  workspace's Rust 1.87 minimum. Ferese core crates keep their existing MSRV.
- libcosmic exposes the created `WlSurface` in public Wayland layer events. This
  is necessary but may not be sufficient to bind `ferese-effects-v1` on the
  toolkit's event queue.
- The spike must therefore prove private semantic-effects attachment before the
  production shell adopts libcosmic. If that cannot be done through supported
  APIs, Ferese will place a thin native Wayland surface adapter beneath the UI
  or revisit this decision; it will not replace public layer-shell with a
  private surface role.

## Validation gate

The provisional decision becomes accepted only after the spike demonstrates:

1. public layer-surface creation on the intended output;
2. output discovery and updates;
3. configure and resize delivery;
4. on-demand keyboard focus and focus release;
5. clean compositor-driven and application-driven shutdown; and
6. `ferese-effects-v1` attachment to the toolkit-owned surface without an
   unsupported libcosmic or Iced fork.

## Spike result

Items 1 through 5 pass against nested Ferese. Item 6 passes after libcosmic
publishes the surface in a focus event: the spike reconstructs a supported
`wayland-client` connection handle, binds the capability-filtered effects
global, and assigns `panel` to the same `wl_surface`.

The decision remains provisional because libcosmic does not currently publish
its layer-surface creation event to the application. A normal non-focusable top
bar therefore has no deterministic pre-map surface handle. Production work may
proceed with libcosmic widgets and application state, but the top-bar surface
path must first gain either:

1. a supported upstream surface-created callback; or
2. the thin native Wayland surface adapter already allowed by the M8 plan.

Ferese will not make the persistent top bar steal keyboard focus merely to
obtain its surface handle.
