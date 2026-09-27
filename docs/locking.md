# Native locker

Ferese Lock uses `ext-session-lock-v1`, not a fullscreen application overlay.
Each output receives its own surface, including outputs added while locked.
The compositor protects the desktop if the client exits or crashes.

## Preview

```sh
cargo build --release --locked -p ferese-lock
target/release/ferese-lock --preview
```

This is an ordinary, closable window. Submitting the field does not call PAM,
lock, or unlock anything. Do not type your actual password into a visual preview.

Settings → Lock screen provides clock visibility, date visibility, time format,
wallpaper dimming and softness, and a safe preview button. Your account picture is loaded from
AccountsService or `~/.face`, with a user silhouette as a fallback.

The locker reads Ferese's wallpaper, theme colors, typography, and shell radius
at startup. The wallpaper is softened once and shared across displays. Set softness
to zero to keep it sharp. Escape clears the password. Caps Lock is shown above authentication
status. Password verification runs on a worker, and repeated attempts are held
for two seconds after failure, in addition to delays enforced by PAM.

## Installation and locking

The normal `scripts/install.sh` includes the native binary and installs
`/etc/pam.d/ferese-lock` if absent. Existing administrator policies are preserved.
Fedora/Arch installations use `system-auth`; Debian-family installations use
`common-auth` and `common-account`; the fallback uses the system `login` stack.
There is no setuid binary, password command argument, or shadow-file reader.
Both authentication and account validation must succeed. Expired credentials
must be updated outside the locker. Interactive multi-factor enrollment and
password changes are not supported by this initial password UI.

```sh
ferese-lock                # returns after confirmation, UI stays running
ferese-lock --foreground   # stays attached until unlock
```

The `swayidle -w` example in [Configuration](configuration.md#login-items-and-locking)
can invoke the default command. Auto-locking is opt-in. This change does not
install or start an idle daemon, replace your login manager, or change the Control Center's
suspend action. Test real password authentication inside a nested Ferese session
before enabling this on your desktop.

## Security and recovery

Never interpret the preview as proof of a locked desktop. A missing PAM policy
or unsupported compositor produces an error before a lock request. A failure
or authentication-worker panic never authorizes unlock. A crash after the lock
request may leave the session permanently locked; end that session from another
TTY if necessary. Do not restart the compositor to test crash behavior on a live
session with unsaved work.

The pinned libcosmic backend emits `Locked` twice: once when requesting a lock
and once on the actual protocol callback. The startup handshake waits for the
second notification. Re-audit this behavior whenever upgrading libcosmic. The
parent times out with an error after 15 seconds and leaves the UI running; it
never reports success merely because the window appeared.

Validation references: [session-lock protocol](https://wayland.app/protocols/ext-session-lock-v1),
[PAM authentication](https://man7.org/linux/man-pages/man3/pam_authenticate.3.html),
and [PAM account checks](https://man7.org/linux/man-pages/man3/pam_acct_mgmt.3.html).
