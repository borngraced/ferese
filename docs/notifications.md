# Notifications

Ferese shows compact notification popups and a history panel opened from the bar.
Their colors, font, corners, and background follow the shell theme.

## Popups and history

Up to three popups are visible at once. Hovering pauses a popup's timer.
History keeps the latest 100 notifications and clears when the shell exits.
App action buttons work when provided by the sender. Transient messages leave
history when closed.

Critical notifications bypass Do Not Disturb and remain visible unless the app
provides a timeout. Other apps can also request a timeout or no timeout.

## Settings

**Settings → Notifications** controls popup visibility, the default timeout, and
the Do Not Disturb preference. The bar toggle changes Do Not Disturb for the
current session; Settings saves it for future sessions. Hidden popups still enter
history.

```kdl
notifications {
    show-popups #true
    do-not-disturb #false
    timeout-ms 6000
}
```

The default timeout accepts 1000–30000 milliseconds.

## Send a test notification

```sh
notify-send --app-name=Ferese --icon=dialog-information "Hello" "Your desktop notification"
```

## Troubleshooting

Only one notification server can run on the session bus. The session launcher
stops `swaync.service` before starting the shell. When launching the shell manually,
stop any existing notification server first. If another server owns the interface,
Ferese logs that native notifications are unavailable.

Ferese supports plain text, app icons, actions, replacement, and urgency. Rich
markup, embedded images, sounds, and saved notification history are not supported.
