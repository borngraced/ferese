# Why Ferese

Omarchy and Ferese come from two different ideas about how a desktop should be
built. Omarchy brings existing Linux tools together and configures them as one
environment, which gives it access to mature components that already do their
jobs well.

A setup like that might combine:

```text
Hyprland + bar + launcher + notifications + lock screen + theme scripts
```

There is nothing wrong with this approach, and being able to replace each part
is useful, but those parts still have their own configuration, state, rendering,
and lifecycle. Matching their themes can make them look related, while keeping
their behavior in sync remains an ongoing part of maintaining the desktop.

With Ferese, we are building the compositor and shell together, so workspaces,
layout, focus, outputs, input, and materials all come from the same desktop
state. The shell and Settings use first-party interfaces to work with that state
instead of reading separate versions of it from several external tools.

This does not mean every Ferese component has to run in one process. It means
that the components share the same concepts, configuration, and protocols, so a
workspace or surface means the same thing everywhere it appears.

## What this means in practice

You can see the difference in something as simple as corner radius. Ferese has a
shared setting for shell surfaces:

```kdl
theme {
    geometry {
        shell-radius 16
    }
}
```

Changing that value updates the bar, Control Center, popovers, notifications,
widgets, and native lock screen together. Managed windows have their own radius
because they are a different kind of surface, but each value still has one clear
meaning wherever it is used.

In an assembled desktop, making the same visual change may mean editing the
window manager, bar CSS, launcher theme, notification daemon, and lock screen
config separately, then keeping those values aligned as the setup changes.

The same difference shows up in how the desktop behaves. Ferese implements
scrolling as part of its workspace model, renders blur behind shell surfaces in
the compositor, and builds the overview from the real window scene. Focus,
fullscreen, workspace geometry, and input all work from the same state rather
than waiting for separate tools to catch up with one another.

When a window opens, Ferese can handle the result as one transition:

```text
window opens → layout updates → viewport moves → input follows
```

An assembled environment may need to publish a window-manager event, pass it
through IPC, run a script, update the bar, and start another animation before
the rest of the desktop reflects the change. Each step can be fast, but it is
still another boundary that has to be maintained.

## The tradeoff we are making

Building more of the desktop ourselves gives Ferese tighter control over how its
parts work together, but it also leaves us responsible for more code. Omarchy
can rely on mature components from across the Linux desktop ecosystem, while
Ferese has to build that same level of maturity over time, and that is a tradeoff
we are willing to make to keep the desktop consistent in both behavior and
configuration.
