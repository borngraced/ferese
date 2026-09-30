# Lock screen

Ferese Lock follows your desktop wallpaper, colors, font, and shell corners.
Open **Settings → Lock screen** to change its clock, date, wallpaper softness,
and inactivity delays.

## Preview and lock

```sh
ferese-lock --preview      # ordinary window; does not lock or check passwords
ferese-lock                # lock; returns after compositor confirmation
ferese-lock --foreground   # stay attached until unlock
```

Do not enter your real password in a preview. The account picture comes from
AccountsService or `~/.face`; a silhouette is used when neither is available.

Escape clears the password. Caps Lock is shown beside authentication status.
Failed attempts show an error and delay retries. Authentication uses the system's
PAM policy; both password and account validation must succeed.

## Automatic locking

Install `swayidle`, then add this to `~/.config/ferese/config.kdl` to lock after
five minutes and before system sleep:

```kdl
autostart {
    command "swayidle" "-w" "timeout" "300" "ferese-lock" "before-sleep" "ferese-lock" "lock" "ferese-lock"
}
```

Test password unlocking before enabling automatic locking. The installer provides
`/etc/pam.d/ferese-lock` and preserves an existing administrator policy.

## Inactivity and display sleep

Locked displays dim after 30 seconds without input and sleep after 120 seconds.
Keyboard, pointer, or touch activity wakes them without unlocking. Set either
delay to zero to disable it, in Settings or KDL:

```kdl
lock-screen {
    dim-after-seconds 30
    sleep-after-seconds 120
}
```

These delays start after locking, independently of the automatic-lock timer.
Display sleep does not suspend the computer. A nested preview shows black
instead of powering off the host display.

## Recovery and limits

The lock covers all displays, including newly connected ones. If the lock UI
crashes, the compositor keeps the session protected. Recovery may require ending
that session from another TTY; see [Recovery](installation.md#logout-and-recovery).

The password interface does not support password changes or interactive
multi-factor prompts. Update expired credentials outside the locker.

Developers should validate locking and readiness against
[ext-session-lock-v1](https://wayland.app/protocols/ext-session-lock-v1).
See [Development](development.md) for tests.
