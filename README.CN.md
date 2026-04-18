<p align="center">
  <img src="ui/assets/waywallen-ui.svg" alt="Waywallen" width="128" />
</p>

<h1 align="center">Waywallen</h1>

<p align="center"><strong> Wallpaper Manager for Linux </strong></p>

---

Waywallen 是一个为 Linux 桌面打造的动态壁纸方案  
最初是 wallpaper engine plugin for kde  

---

## 界面

## 快速开始

Waywallen 由两部分组成，按需分别安装：

1. **本体** —— daemon + 管理 UI + 渲染器。负责壁纸的加载、渲染与分发。
2. **桌面集成插件** —— 把画面接到你所在 DE 的桌面背景上。不同 DE 用不同插件，按需选装。

### 1. 安装本体

**Flatpak**

```bash
flatpak install org.waywallen.waywallen
```

### 2. 安装桌面集成插件

| 桌面 | 插件 | 获取方式 |
|------|------|----------|
| **KDE Plasma 6** | `waywallen-kde` | Pling 商店搜索 **Waywallen**，或从 `waywallen-kde/` 安装 |
| **GNOME / Hyprland / 其它** | — | 暂未提供官方插件，欢迎接入 `waywallen-display` 自行实现 |

装好插件后，**右键桌面 → 配置壁纸 → 选择 Waywallen**，挑一张开始。

## 兼容性

| 项目 | 现状 |
|------|------|
| KDE Plasma 6 | ✅ |
| GNOME / Hyprland / 其它 | ⚠️ 需自行接入显示端 |
| 场景壁纸 | ✅ open-wallpaper-engine |
| 视频壁纸 | ✅ mpv |
| 网页壁纸 | ⚠️ 规划中 |

## 贡献 & 反馈

欢迎 issue / PR，尤其是：

- 更多 DE 的显示端适配（Hyprland、Sway、GNOME……）
- Wallpaper Engine 剩余特性补齐
- 翻译、截图、示例壁纸