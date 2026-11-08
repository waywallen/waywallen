<p align="center">
  <img src="ui/assets/waywallen-ui.svg" alt="Waywallen" width="128" />
</p>

<h1 align="center">Waywallen</h1>

<p align="center"><strong> Linux 壁纸管理器 </strong></p>

<a href="README.md">English README</a> · <a href="README.RU.md">Русский README</a> · <a href="https://discord.gg/2xEdmMrhRF">Discord</a>

---

Waywallen 是一个为 Linux 桌面打造的动态壁纸解决方案。<br>
它最初是 KDE 的 Wallpaper Engine 插件。

---

## 界面

<p align="center">
  <img src="ui/assets/main_page.webp" alt="Waywallen 主界面" width="720" />
</p>

## 快速开始

### 安装

**预编译包**  
到 [Releases 页面](https://github.com/waywallen/waywallen/releases) 下载最新版本。

**Flatpak**

<a href='https://flathub.org/en/apps/org.waywallen.waywallen'>
  <img width='240' alt='Get it on Flathub' src='https://flathub.org/api/badge?locale=zh-Hans'/>
</a>

**从源码构建**  
见 [BUILD.md](BUILD.md)。  

Waywallen 使用 [Lito](https://github.com/litocpp/lito) 构建（请为它点个 Star）。  

### 桌面集成

| 桌面 | 集成 | 鼠标输入 | 自动暂停 |
|---------|-------------|:-----------:|:----------:|
| **KDE Plasma** | [waywallen-display](https://github.com/waywallen/waywallen-display/) | ✅ | ✅ |
| **GNOME** | [waywallen-display](https://github.com/waywallen/waywallen-display/) | ✅ | ✅ |
| **Hyprland** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **Niri** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **Wayfire** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **COSMIC** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **Sway** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ❌ |

## 已知问题

- NVIDIA GPU 运行网页壁纸时，需要在网页渲染器设置中关闭 `shared_texture_enabled`。
- Flatpak 需要 `--talk-name=org.mpris.MediaPlayer2.*` D-Bus 权限才能获取当前播放的歌曲信息。可使用以下命令为当前用户添加：
  ```bash
  flatpak override --user --talk-name='org.mpris.MediaPlayer2.*' org.waywallen.waywallen
  ```

## 壁纸插件

- 图片插件
- 视频插件
  - 通过 Vulkan 和 VA-API 进行硬件解码
- glTF 插件
- [Wallhaven](https://wallhaven.cc/) 插件
- [Sketchfab](https://sketchfab.com/) 插件

### 第三方插件

- [open-wallpaper-engine](https://github.com/waywallen/open-wallpaper-engine)
  - 场景壁纸支持
  - 网页壁纸支持

> [!NOTE]
> 要安装第三方插件，请手动下载其 ZIP 压缩包，并在界面的插件页面中安装。<br>
> 安装完成后，Waywallen 会提示并安装插件更新。

> [!WARNING]
> **第三方插件是受信任的代码。** 插件的 Lua 入口会在 Waywallen 守护进程中以你的用户账户权限运行——就像你安装的任何应用程序一样，它可以读写你的文件并访问网络。**请只安装你信任的插件**，最好在查看其源代码后再安装。

## FAQ

- 硬件视频解码如何工作？  
  默认 `auto` 模式使用以下回退顺序：  
  `vulkan -> vaapi -> sw`  
  可以在 `waywallen-video` 的设置中选择 `hwdec` 模式，而不使用 `auto`。  

  我们不计划添加独立的 NVDEC 后端。  
  NVIDIA 用户应使用 [nvidia-vaapi-driver](https://github.com/elFarto/nvidia-vaapi-driver)，通过 VA-API 暴露 NVDEC。  

- 如何获取日志？  
  日志按 UTC 日期写入 `~/.local/state/waywallen/logs/daemon/`（或 `$XDG_STATE_HOME/waywallen/logs/daemon/`），最多保留 8 个 `waywallen.YYYY-MM-DD.log`；`waywallen-current.log` 指向当前文件。
  如需提高详细程度，请先退出正在运行的守护进程，然后：
  ```bash
  export WW_LOG=debug
  ./waywallen
  ```
  Flatpak 日志存储在 `~/.var/app/org.waywallen.waywallen/.local/state/waywallen/logs/daemon/`。
  可使用以下命令以详细日志重新启动：
  ```bash
  flatpak kill org.waywallen.waywallen
  flatpak run \
    --env=WW_LOG=debug \
    org.waywallen.waywallen
  ```

- 如何在 flatpak 中调试？
  ```bash
  flatpak install org.waywallen.waywallen.Debug
  flatpak run --devel --command=bash org.waywallen.waywallen
  # 1. 直接运行
  [📦 org.waywallen.waywallen ~]$ gdb waywallen
  (gdb) run
  Enable debuginfod for this session? (y or [n]) n
  ...
  # 获取堆栈
  (gdb) bt

  # 2. 或使用 coredump 文件
  coredumpctl dump <id> -o core.save
  flatpak run --devel --filesystem=host --command=bash org.waywallen.waywallen
  [📦 org.waywallen.waywallen ~]$ gdb waywallen core.save
  ...
  ```
