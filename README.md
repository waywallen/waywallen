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

Waywallen ships in two pieces — install them separately as needed:

1. **Core** — daemon + management UI + renderers. Handles loading, rendering and dispatching wallpapers.
2. **Desktop integration plugin** — pipes the picture onto your DE's desktop background. Different DEs need different plugins; pick the one you use.

### 1. Install the core

**Flatpak**

```bash
flatpak install org.waywallen.waywallen
```

### 2. Install a desktop integration plugin

| Desktop | Plugin | How to get it |
|---------|--------|---------------|
| **KDE Plasma 6** | `waywallen-kde` | Search **Waywallen** on the Pling store, or build from source in `waywallen-kde/` |
| **GNOME / Hyprland / others** | — | No official plugin yet — contributions welcome via `waywallen-display` |

Once installed: **right-click the desktop → Configure Wallpaper → pick Waywallen**, and choose one.

## Compatibility

| Item | Status |
|------|--------|
| KDE Plasma 6 | ✅ |
| GNOME / Hyprland / others | ⚠️ BYO display backend |
| Scene wallpapers | ✅ via open-wallpaper-engine |
| Video wallpapers | ✅ via mpv |
| Web wallpapers | ⚠️ planned |

## Contributing & feedback

Issues and PRs are welcome — especially:

- More DE display-side integrations (Hyprland, Sway, GNOME…)
- Filling in the remaining Wallpaper Engine features
- Translations, screenshots, example wallpapers
