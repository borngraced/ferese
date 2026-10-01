# Why Ferese

Most Wayland desktops are assembled from separate programs such as a compositor, a bar, a notification daemon, a launcher, a lock screen with each having its own config and its own idea of how things should look. Ferese is built as one desktop. Window management and the shell share a single configuration, so personalizing it means changing one thing, not five.

## Choose how windows fit

Keep windows at useful widths and move through them horizontally with scrolling columns,
divide the screen into nested splits with tree tiling, or let individual windows float
alongside either layout. When you want to see how everything fits together, overview
brings your windows and workspaces into view.

![Ferese Overview showing windows and workspaces](images/screenshots/ferese-blue-overview.png)

## Change the desktop together

Choose a theme in Settings and build on it with your own wallpaper, colors, font,
corners and motion, with changes applying across the desktop as you make them. Window
and shell corners have separate controls, so you can shape each to your liking.

![Ferese Settings and Control Center in Rosé Pine](images/screenshots/rose-pine-desktop.png)

To give your shell corners a different radius, add:

```kdl
theme {
    geometry {
        shell-radius 16
    }
}
```

Explore [Configuration](configuration.md) for everything you can change, or follow
[Installation](installation.md) to try Ferese inside your current desktop with a nested
preview.
