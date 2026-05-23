<p align="center">
  <img src="ui/assets/waywallen-ui.svg" alt="Waywallen" width="128" />
</p>

<h1 align="center">Waywallen</h1>

<p align="center"><strong> Wallpaper Manager for Linux </strong></p>

<a href="README.CN.md">中文 README</a> · <a href="https://discord.gg/6rx99hx9j">Discord</a>

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

**Prebuilt binaries** — grab the latest archive from the [Releases page](https://github.com/waywallen/waywallen/releases).

**From source** — see [BUILD.md](BUILD.md).

### Desktop integration

| Desktop | Integration |
|---------|-------------|
| **KDE Plasma** | [waywallen-display](https://github.com/waywallen/waywallen-display/) |
| **Niri** | `zwlr_layer_shell_v1` |
| **Sway** | `zwlr_layer_shell_v1` |
| **GNOME** | ️planned |

## Compatibility

| Item | Status |
|------|--------|
| Image wallpapers | ✅ |
| Scene wallpapers | ✅ via [open-wallpaper-engine](https://github.com/waywallen/open-wallpaper-engine) |
| Video wallpapers | ✅ |
| Web wallpapers | ✅ via [open-wallpaper-engine](https://github.com/waywallen/open-wallpaper-engine) |

---

# NOTE: For KDE Plasma 6
If you are using KDE Plasma 6, your display won't be detected by Waywallen.
KDE Plasma 6 handles wallpaper completely different than older version, waywallen just ran daemon on the background and it can’t detect display layouts on it’s own in Plasma 6,
it relies on Waywallen KDE Plasma 6 Plugin to detect display and manage the surface geometry via DMA-BUF and render wallpapers.

1. Download Waywallen KDE Plasma 6 Plugin Zip files on here: https://store.kde.org/p/2356221
2. Open Konsole, change directory to the location of the KDE Plasma 6 Plugin Zip file and run:
```
kpackagetool6 --type Plasma/Wallpaper -i waywallen-kde-YOUR_VERSION_HERE.zip
```
3. Restart Plasma Shell:
```
systemctl --user restart plasma-plasmashell.service
```
4. Go to Desktop, right click > Desktop & Wallpaper (Ctrl + Shift + D)
<img width="1366" height="768" alt="image" src="https://github.com/user-attachments/assets/8803764c-76a8-4936-aab9-c337773d1cf4" />
5. Choose Wallpaper Type: Waywallen
<img width="1366" height="768" alt="image" src="https://github.com/user-attachments/assets/c2d85d81-c8cf-4e96-b23a-91cf45afdf6a" />

And now your Waywallen can detect your display:
<img width="1366" height="768" alt="image" src="https://github.com/user-attachments/assets/3a5fc996-ec9c-47ea-81ab-ee2d24ca54be" />
