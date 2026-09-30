# Why Ferese

Ferese combines window management and a desktop shell with one configuration.
Settings, the bar, notifications, widgets, authentication prompts, and the lock
screen share colors and fonts.

## Choose how windows fit

Scrolling columns let you keep windows at useful widths and move through them
horizontally. Tree tiling divides the screen into nested splits. Floating windows
work alongside either layout, and overview shows windows across workspaces.

![Ferese Overview showing windows and workspaces](images/ferese-overview.webp)

## Change the desktop together

Choose a theme in Settings, then adjust the wallpaper, colors, font, corners,
and motion. Changes apply live. Window corners and shell corners can be set
independently.

![Ferese Settings and Control Center in Gruvbox](images/ferese-gruvbox.webp)

For example, this changes shell corners:

```kdl
theme {
    geometry {
        shell-radius 16
    }
}
```

See [Configuration](configuration.md) for all options, or
[Installation](installation.md) to try Ferese in a nested preview.
