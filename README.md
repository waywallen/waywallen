<p align="center">
  <img src="ui/assets/waywallen-ui.svg" alt="Waywallen" width="128" />
</p>

<h1 align="center">Waywallen</h1>

<p align="center"><strong> Wallpaper Manager for Linux </strong></p>

<a href="README.CN.md">中文 README</a>

---

Waywallen is a dynamic wallpaper solution for Linux desktops.  
It started life as a Wallpaper Engine plugin for KDE.

---

## Screenshots

<p align="center">
  <img src="ui/assets/main_page.png" alt="Waywallen main page" width="720" />
</p>

## Quick Start

### Install

**Flatpak**
[org.waywallen.waywallen](https://github.com/hypengw/org.waywallen.waywallen)

**From source** — see [BUILD.md](BUILD.md).

### Desktop integration

| Desktop | Integration |
|---------|-------------|
| **KDE Plasma** | [waywallen-kde](https://github.com/waywallen/waywallen-kde) |
| **Niri** | `zwlr_layer_shell_v1` |
| **Sway** | `zwlr_layer_shell_v1` |
| **GNOME** | ❌ |

## Compatibility

| Item | Status |
|------|--------|
| Image wallpapers | ✅ |
| Scene wallpapers | ✅ via open-wallpaper-engine |
| Video wallpapers | ✅ via mpv |
| Web wallpapers | ⚠️ planned |

## Contributing & feedback

Issues and PRs are welcome — especially:

- More DE display-side integrations (Hyprland, Sway, GNOME…)
- Filling in the remaining Wallpaper Engine features
- Translations, screenshots, example wallpapers
