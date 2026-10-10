<p align="center">
  <img src="ui/assets/waywallen-ui.svg" alt="Waywallen" width="128" />
</p>

<h1 align="center">Waywallen</h1>

<p align="center"><strong> Wallpaper Manager for Linux </strong></p>

<a href="README.CN.md">中文 README</a> · <a href="README.RU.md">Русский README</a> · <a href="https://discord.gg/2xEdmMrhRF">Discord</a>

---

Waywallen is a dynamic wallpaper solution for Linux desktops.<br>
It started life as a Wallpaper Engine plugin for KDE.

---

## Screenshots

<p align="center">
  <img src="ui/assets/main_page.webp" alt="Waywallen main page" width="720" />
</p>

## Quick Start

### Install

**Prebuilt binaries**  
grab the latest AppImage from the [Releases page](https://github.com/waywallen/waywallen/releases).

**Flatpak**

<a href='https://flathub.org/en/apps/org.waywallen.waywallen'>
<img width='240' alt='Get it on Flathub' src='https://flathub.org/api/badge?locale=en'/>
</a>

**From source**  
see [BUILD.md](BUILD.md).  

Waywallen is built by [Lito](https://github.com/litocpp/lito) (Please star it)  

### Desktop integration

| Desktop | Integration | Mouse input | Auto pause |
|---------|-------------|:-----------:|:----------:|
| **KDE Plasma** | [waywallen-display](https://github.com/waywallen/waywallen-display/) | ✅ | ✅ |
| **GNOME** | [waywallen-display](https://github.com/waywallen/waywallen-display/) | ✅ | ✅ |
| **Hyprland** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **Niri** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **Wayfire** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **COSMIC** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **Sway** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ❌ |

## Known issue
- Recent NVIDIA driver has bug with `egl`, please set `QSG_RHI_BACKEND=vulkan` with `plasmashell`, more details at [`waywallen-display`](https://github.com/waywallen/waywallen-display#kde-plasma-extension).  
- Web wallpapers on NVIDIA GPUs require `shared_texture_enabled` to be disabled in the web renderer settings.
- Flatpak requires the `--talk-name=org.mpris.MediaPlayer2.*` D-Bus permission to read information about the currently playing track. Grant it for the current user with:
  ```bash
  flatpak override --user --talk-name='org.mpris.MediaPlayer2.*' org.waywallen.waywallen
  ```

## Wallpaper plugins

- Image plugin
- Video plugin
  - Hardware decoding via Vulkan and VA-API
- Wallhaven plugin

### Third-party plugins

- [open-wallpaper-engine](https://github.com/waywallen/open-wallpaper-engine)
  - Scene support
  - Web support

> [!NOTE]
> To install a third-party plugin, manually download its ZIP archive and install it from the plugins page in the UI.<br>
> After installation, Waywallen will notify you of plugin updates and install them.

> [!WARNING]
> **Third-party plugins are trusted code.** A plugin's Lua entry point runs inside the Waywallen daemon with your user account's privileges — just like any application you choose to install, it can read and write your files and access the network. **Only install plugins you trust**, preferably after reviewing their source.

## FAQ

- How does hardware video decoding work?  
  The default `auto` mode uses the following fallback order:  
  `vulkan -> vaapi -> sw`  
  You can select the `hwdec` mode in the `waywallen-video` settings instead of using `auto`.  

  We do not plan to add a dedicated NVDEC backend.  
  NVIDIA users should use [nvidia-vaapi-driver](https://github.com/elFarto/nvidia-vaapi-driver) to expose NVDEC through VA-API.

- How to get logs?  
  Daily daemon logs are written under `~/.local/state/waywallen/logs/daemon/` (or `$XDG_STATE_HOME/waywallen/logs/daemon/`) as `waywallen.YYYY-MM-DD.log` (up to 8 kept); `waywallen-current.log` points to the active file.
  To raise verbosity, stop the running daemon and restart with:
  ```bash
  export WW_LOG=debug
  ./waywallen
  ```
  For Flatpak, logs are stored under `~/.var/app/org.waywallen.waywallen/.local/state/waywallen/logs/daemon/`.
  Restart it with verbose logging using:
  ```bash
  flatpak kill org.waywallen.waywallen
  flatpak run \
    --env=WW_LOG=debug \
    org.waywallen.waywallen
  ```

- How to debug in Flatpak?  
  ```bash
  flatpak install org.waywallen.waywallen.Debug
  flatpak run --devel --command=bash org.waywallen.waywallen
  # 1. Run directly
  [📦 org.waywallen.waywallen ~]$ gdb waywallen
  (gdb) run
  Enable debuginfod for this session? (y or [n]) n
  ...
  # Get the stack trace
  (gdb) bt

  # 2. Or use a core dump file
  coredumpctl dump <id> -o core.save
  flatpak run --devel --filesystem=host --command=bash org.waywallen.waywallen
  [📦 org.waywallen.waywallen ~]$ gdb waywallen core.save
  ...
  ```
