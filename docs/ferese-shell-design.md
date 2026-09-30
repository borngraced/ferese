# Shell behavior

This is a maintainer reference. User options are documented in
[Configuration](configuration.md). Common UI belongs in
[ferese-theme](../crates/ferese-theme/README.md).

## Bar and popovers

The shell creates a bar for each output. Workspace buttons use compositor state
and appear in numeric order. The active window title occupies the center when
space allows. Date and time form one calendar button. Right-side controls open
sound, battery, notifications, recording, and Control Center popovers.

A popover is anchored to its control and constrained to the same output.
Opening another primary popover replaces the current one. Popovers reserve no
workspace space; the bar reserves its configured height and margins.
True fullscreen hides the bar without changing the preserved tiled layout.

Use real service availability for controls. Handle missing services and failed
actions without blocking the UI. Settings opens as a separate application.

## Overview

The compositor presents window previews; the shell supplies labels and workspace
controls. Do not continuously copy windows into the shell process.

Clicking a workspace background selects that workspace and keeps overview open
so its windows can be selected. Clicking a window activates that exact window.
Directional keys change selection, Enter activates it, and Escape dismisses.
Activation must use the selected window ID, not the window focused before overview.

## Focus and dialogs

The bar and notification popups do not normally take keyboard focus. Interactive
shell surfaces restore the previous valid app focus when dismissed.

Power actions and logout use a centered system confirmation, including blockers
when present. Input outside the confirmation must not reach applications.
Authentication, screen-sharing, and recording dialogs are separate themed windows.
Use one visible frame, content-sized height, and an output-sized height limit.
Scrollable lists must leave headings and action buttons accessible.

Attach dialog material to the content card using the shared theme helper.
Match blur regions to the displayed card throughout opening and resize; do not
paint an opaque window behind a translucent card.

## Notifications and widgets

The shell provides the standard notification service. Normal popups respect
Do Not Disturb; critical messages bypass it. History is held in memory and clears
when the shell exits. See [Notifications](notifications.md).

Clock and note widgets sit behind application windows, reserve no workspace space,
and save positions after dragging. Only interactive notes take keyboard focus.
See [Desktop widgets](desktop-widgets.md).

## Lifecycle

Shell failure must not destroy managed windows or workspace state. Reconstruct
bars and controls from compositor state after reconnecting, and clear dead shell
focus. Notification history is not reconstructed from disk.

Keep slow service queries, image loading, and filesystem work away from UI event
handling. Stop animation work once the UI settles. Validate popover placement,
fullscreen transitions, dialog dismissal, and shell restart in a nested session.
