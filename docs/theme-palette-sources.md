# Theme palette sources
Ferese maps upstream palettes to desktop roles. Materials are authored for Ferese;
text and accents have the adjustments below to meet desktop contrast requirements.
| Family | Source revision | License |
| --- | --- | --- |
| catppuccin | [07d02aa110ef](https://github.com/catppuccin/palette/blob/07d02aa110ef9eb7e7427afca5c73ba9cf7f8ebd/palette.json) | MIT |
| gruvbox | [ef8864bb42bf](https://github.com/morhetz/gruvbox/blob/ef8864bb42bf244f0295d1c5a403b27e3d139695/colors/gruvbox.vim) | MIT/X11 |
| rose-pine | [ff483051a47e](https://github.com/rose-pine/neovim/blob/ff483051a47e27d84bdef47703538df1ed9f4a47/lua/rose-pine/palette.lua) | MIT |
| tokyo-night | [cdc07ac78467](https://github.com/folke/tokyonight.nvim/blob/cdc07ac78467a233fd62c493de29a17e0cf2b2b6/extras/kitty/tokyonight_day.conf) | Apache-2.0; palette exports marked MIT |
| everforest | [85a86eb62409](https://github.com/sainnhe/everforest/blob/85a86eb62409e3ec88713bff3d1b9d7374e112e4/autoload/everforest.vim) | MIT |

Tokyo Night Night also uses its [official export](https://github.com/folke/tokyonight.nvim/blob/cdc07ac78467a233fd62c493de29a17e0cf2b2b6/extras/kitty/tokyonight_night.conf).

## Color adjustments

| Variant | Token | Upstream | Ferese |
| --- | --- | --- | --- |
| rose-pine-moon | muted | `#908CAA` | `#918DAA` |
| rose-pine-dawn | muted | `#797593` | `#716D89` |
| tokyo-night-day | text | `#3760BF` | `#3358B0` |
| tokyo-night-day | muted | `#6172B0` | `#4D5A8C` |
| tokyo-night-day | accent | `#2E7DE9` | `#2C76DD` |
| everforest-light | muted | `#829181` | `#677367` |
| everforest-light | accent | `#8DA101` | `#849601` |

Catppuccin Latte uses upstream subtext1 for muted labels. Gruvbox uses readable
foreground shades rather than its comment gray. Rosé Pine uses its subtle role
for secondary labels and its love accent in Dawn. Everforest uses the Hard pair.
Tokyo Night uses the upstream Night and Day exports.

License texts are bundled in [assets/themes/licenses](../assets/themes/licenses)
and copied into each installed release. Older saved Monochrome, Dracula, Ayu Light,
and Monokai selections remain readable, but are absent from the gallery.
