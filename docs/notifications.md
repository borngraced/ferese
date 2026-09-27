# Notifications

Ferese Shell provides themed notification cards and an in-session history panel.
Cards appear beneath the bar on the focused output and follow the selected theme.
Click the notification icon in the bar to open history, toggle Do Not Disturb, or
clear notifications. Escape or the close button closes the history panel.

## Send a notification

Notifications use the standard `org.freedesktop.Notifications` D-Bus interface.
Existing apps and `notify-send` work without Ferese-specific commands:

```sh
notify-send --app-name=Ferese --icon=dialog-information "Hello" "Your desktop notification"
```

## Popups and history

Up to three popup cards are visible at once. History keeps the most recent 100
notifications in memory and clears when the shell exits. Hovering a popup pauses
its timer. Critical notifications bypass Do Not Disturb and do not expire unless
the sender supplies a timeout. App action buttons emit the standard action signal;
a replacement updates the same notification ID. Transient messages are removed
from history when they close.

## Settings and Do Not Disturb

Settings → Notifications controls popup visibility, the initial Do Not Disturb
state, and the default timeout. The bar's Do Not Disturb toggle is temporary;
changing the Settings option saves the preference for future sessions. Apps can
request their own timeout, including no timeout.

```kdl
notifications {
    show-popups #true
    do-not-disturb #false
    timeout-ms 6000
}
```

The timeout must be between 1000 and 30000 milliseconds. Hiding popups still
records notifications in history. Appearance follows the Ferese theme.

## Session integration

Only one notification server can own the D-Bus interface. The Ferese session
launcher stops the SwayNotificationCenter user service before starting its shell.
For a manually started shell, stop that service with `systemctl --user stop
swaync.service`. If another server already owns the interface, Ferese leaves it
running and logs that native notifications are unavailable.

This implementation supports plain text, app icons, urgency, transient/resident
hints, actions, replacements, and close signals. It does not advertise rich body
markup, embedded image data, notification sounds, or history persistence.
