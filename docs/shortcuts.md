# Shortcuts and gestures

`Super` is usually the Windows key. These are the built-in defaults. Open
**Settings → Shortcuts** to change bindings and gestures or disable the login
guide. Your saved bindings can override these defaults.

## Move and resize with the mouse

| Action | Result |
| --- | --- |
| Super + left-button drag on a floating window | Move the window |
| Super + right-button drag on a floating window | Resize from the nearest corner |
| Super + Shift + Space | Switch the focused window between tiled and floating |

You can drag from inside the window; a title bar or resize border is not required.
To place an app freely, toggle it to floating, then Super-drag it.

## Focus, move, and resize

`H`, `J`, `K`, and `L` mean left, down, up, and right.

| Shortcut | Action |
| --- | --- |
| Super + H / J / K / L | Focus in that direction |
| Super + Shift + H / J / K / L | Move the focused window in that direction |
| Super + Ctrl + H / J / K / L | Resize in that direction |
| Super + Q | Close the focused window |
| Super + F | Maximize or restore the window |
| Super + Shift + F | Enter or leave fullscreen |

Maximize fills the available workspace. Fullscreen also hides the bar and window
decorations.

## Columns and layouts

| Shortcut | Action |
| --- | --- |
| Super + R | Cycle column width |
| Super + C | Center the focused column |
| Super + [ | Place the window in a neighbouring column |
| Super + ] | Extract the window into its own column |
| Super + M | Switch scrolling / tree layout |
| Super + Tab | Open or close overview |

In scrolling mode, **Super + [** uses the left neighbour when available, otherwise
the right. Try it with two terminals to place them vertically in one column.
**Super + ]** separates the focused window again.

In overview, select a workspace to see its windows, then click the window you
want. Directional focus keys move selection; Enter activates it and Escape closes
overview.

## Workspaces

| Shortcut | Action |
| --- | --- |
| Super + 1–9 | Switch to that workspace |
| Super + Shift + 1–9 | Move the window to that workspace |

Workspaces are created as needed. Selecting one visible on another monitor focuses
that monitor.

## Desktop actions

| Shortcut | Action |
| --- | --- |
| Super + Enter | Open the configured terminal (`foot` by default) |
| Print Screen | Capture the active monitor and open Satty |
| Super + Shift + S | Select a screenshot area and open Satty |
| Super + Shift + E | Ask to log out |

In Satty, Enter saves the edited screenshot and copies it to the clipboard;
Escape discards it. See [Configuration](configuration.md#commands-and-bindings)
for dependencies and custom screenshot commands.

## Touchpad gestures

| Three-finger swipe | Action on release |
| --- | --- |
| Up / down | Next / previous workspace on this monitor |
| Left / right | Focus the window to the right / left |

Short, diagonal, or cancelled swipes do nothing. Gestures can use the same actions
as keyboard bindings, including overview or moving a window. Four- and five-finger
bindings are supported too.

## Add a shortcut

For example, bind a lock command and use a swipe to open overview:

```kdl
commands {
    lock "ferese-lock"
}
binding "Super+Alt+L" "spawn" "lock"
binding "Swipe3Up" "toggle-overview"
```

These are examples, not defaults. To disable a built-in binding:

```kdl
binding "Super+Q" disabled=#true
```

See [Commands and bindings](configuration.md#commands-and-bindings) for action
names, arguments, and physical-key matching.
