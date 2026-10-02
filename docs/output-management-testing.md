# Output management tests

Run these on a direct DRM session. Nested mode cannot exercise KMS commits,
physical hotplug, logind lid policy or cross-GPU restoration. Do not use nested
results as evidence that a driver accepts the atomic request.

Build and run the policy/state tests first:

```sh
cargo test --release -p ferese -p ferese-core -p feresectl -- --test-threads=1
cargo clippy --release -p ferese -p ferese-core -p feresectl --all-targets -- -D warnings
```

Record `feresectl outputs` and `feresectl output-profiles` before each change.
Check `identity`, `connected`, `requested_enabled`, `applied_enabled`, mode,
scale, transform, position, profile, mirror source and configuration error.
Manual changes normally need `feresectl output-confirm` within 15 seconds;
use `output-revert` to restore immediately. The timeout is profile-configurable.

## Hardware checks

1. Connect one external monitor, then a second. Check distinct IDs and identities,
   most-specific profile selection and deterministic positions. Unplug/replug
   each; serial identities should survive connector changes where unambiguous.
   Check identical monitors with missing/duplicated serials on separate ports.
2. Select internal-only, external-only and extend profiles. Unaffected outputs
   should keep their globals and workspaces. Try `output-internal off` undocked:
   it must fail without disabling the panel. Close/open the lid docked, then
   unplug the dock with the lid closed. Repeat docking while the lid is closed.
3. Undocked, close the lid. Check logind suspension. Repeat with a block sleep
   inhibitor; Ferese must not bypass it. Set `lid-policy="ignore"` in the matching
   profile and confirm lid closure does not trigger Ferese's suspend request.
4. Suspend or switch VT, change monitor topology while inactive, then resume.
   Check that connector and lid state are current, the lock screen wakes and
   frame callbacks continue. If a DRM device fails to reactivate, its outputs
   must not appear active.
5. Mirror different resolutions, scales and rotations. The desktop, overview,
   shell and pointer must match the source; letterboxing must preserve the full
   scene. Check lock-screen mirroring too. Remove a target, then the source;
   remaining windows must be reachable. Return to extend and verify each
   monitor gets a workspace without duplicating ownership.
6. Move and resize floating windows near each edge. Remove their home output,
   remove the evacuation destination, then reconnect the home. Windows must
   remain visible throughout. Deleted workspaces must not return. If you explicitly
   reassign a workspace, that must change its home; reconnecting the old monitor
   must not move it back.
7. Make a manual switch without confirming and disconnect feresectl. Check that
   timeout restores the actual old modes/positions. Repeat with timeout disabled.
   Apply a second change before confirming: its timeout must still restore the
   original confirmed configuration.
   A connector or lid change must cancel the pending confirmation and use auto.
8. Reload duplicate profile names/selectors, invalid mode syntax, zero scale,
   unknown transforms, invalid mirror sources and all-disabled profiles. Check
   the error and verify the accepted config/output state remains active.

## Driver failure checks

Use VKMS or a dedicated DRM test seat with a fault-injection shim; avoid forcing
errors on the desktop being used to run the test. Inject failures during buffer allocation,
TEST_ONLY, a later real CRTC commit, output disable and rollback. The `output_transaction` unit tests cover those failures without hardware.

A TEST_ONLY failure must leave existing hardware and logical outputs unchanged.
A real-commit failure must restore earlier commits where possible. If restoration
fails, the unavailable output must lose its logical global/workspace ownership;
remaining windows must evacuate to a working output. With multiple GPUs, make the later GPU's commit fail and check that Ferese
attempts to restore the earlier devices. This
is not a cross-GPU atomic commit.

Check a mode change under load with a pending page flip. The old flip must not
release callbacks for the new frame. Verify ordinary and mirror source feedback
still reports actual presentation, while targets send no duplicate callbacks.

Capture compositor tracing around each failure. Include driver/GPU names and the
before/after `outputs` dumps when reporting results; a passing mock transaction
does not establish driver-level rollback behavior.

Lock and session ownership checks:

- In mirror mode, delay or fail target rendering while acquiring a session lock.
  The lock must remain unconfirmed until every physical display has presented
  protected content. Reconfigure a mirror and repeat; old presentation evidence
  must not satisfy the new configuration.
- Switch to a TTY and check `systemd-inhibit --list`: Ferese must release its
  `handle-lid-switch` inhibitor. Returning to Ferese should reacquire it.
- Replace a monitor on the same connector. `feresectl outputs` must report the
  replacement identity, rather than the previous monitor's identity.
