<p align="center">
  <img src="ui/assets/waywallen-ui.svg" alt="Waywallen" width="128" />
</p>

<h1 align="center">Waywallen</h1>

<p align="center"><strong> Менеджер обоев для Linux </strong></p>

<a href="README.md">English README</a> · <a href="README.CN.md">中文 README</a> · <a href="https://discord.gg/2xEdmMrhRF">Discord</a>

---

Waywallen — решение для динамических обоев на рабочих столах Linux.<br>
Изначально проект разрабатывался как плагин Wallpaper Engine для KDE.

---

## Скриншоты

<p align="center">
  <img src="ui/assets/main_page.webp" alt="Главная страница Waywallen" width="720" />
</p>

## Быстрый старт

### Установка

**Готовые сборки**  
Скачайте последнюю версию AppImage на [странице релизов](https://github.com/waywallen/waywallen/releases).

**Flatpak**

<a href='https://flathub.org/en/apps/org.waywallen.waywallen'>
<img width='240' alt='Установить из Flathub' src='https://flathub.org/api/badge?locale=ru'/>
</a>

**Из исходного кода**  
См. [BUILD.md](BUILD.md).  

Waywallen собирается с помощью [Lito](https://github.com/litocpp/lito) (поставьте ему звезду).  

### Интеграция с рабочим столом

| Рабочий стол | Интеграция | Ввод с мыши | Автопауза |
|--------------|------------|:-----------:|:----------:|
| **KDE Plasma** | [waywallen-display](https://github.com/waywallen/waywallen-display/) | ✅ | ✅ |
| **GNOME** | [waywallen-display](https://github.com/waywallen/waywallen-display/) | ✅ | ✅ |
| **Hyprland** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **Niri** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **Wayfire** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **COSMIC** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **Sway** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ❌ |

## Известная проблема

- Для веб-обоев на видеокартах NVIDIA необходимо отключить параметр `shared_texture_enabled` в настройках веб-рендерера.
- Flatpak требует разрешение D-Bus `--talk-name=org.mpris.MediaPlayer2.*` для получения информации о текущей композиции. Добавьте его для текущего пользователя командой:
  ```bash
  flatpak override --user --talk-name='org.mpris.MediaPlayer2.*' org.waywallen.waywallen
  ```

## Плагины обоев

- Плагин изображений
- Плагин видео
  - Аппаратное декодирование через Vulkan и VA-API
- Плагин glTF
- Плагин [Wallhaven](https://wallhaven.cc/)
- Плагин [Sketchfab](https://sketchfab.com/)

### Сторонние плагины

- [open-wallpaper-engine](https://github.com/waywallen/open-wallpaper-engine)
  - Поддержка сцен
  - Поддержка веб-обоев

> [!NOTE]
> Для установки стороннего плагина необходимо вручную скачать ZIP-архив и установить его на странице плагинов в интерфейсе.<br>
> После установки Waywallen будет уведомлять вас об обновлениях плагина и устанавливать их.

> [!WARNING]
> **Сторонние плагины — это доверенный код.** Точка входа плагина на Lua выполняется внутри демона Waywallen с правами вашей учётной записи — так же, как любое устанавливаемое вами приложение, он может читать и изменять ваши файлы и обращаться к сети. **Устанавливайте только те плагины, которым доверяете**, желательно после просмотра их исходного кода.

## Часто задаваемые вопросы

- Как работает аппаратное декодирование видео?  
  В режиме `auto` по умолчанию используется следующий порядок отката:  
  `vulkan -> vaapi -> sw`  
  Вместо `auto` режим `hwdec` можно выбрать в настройках `waywallen-video`.  

  Мы не планируем добавлять отдельный бэкенд NVDEC.  
  Пользователям NVIDIA следует использовать [nvidia-vaapi-driver](https://github.com/elFarto/nvidia-vaapi-driver), чтобы использовать NVDEC через VA-API.

- Как получить логи?  
  Посуточные логи по UTC пишутся в `~/.local/state/waywallen/logs/daemon/` (или `$XDG_STATE_HOME/waywallen/logs/daemon/`); сохраняется до 8 файлов `waywallen.YYYY-MM-DD.log`, а `waywallen-current.log` указывает на текущий файл.
  Чтобы повысить подробность, остановите демон и перезапустите:
  ```bash
  export WW_LOG=debug
  ./waywallen
  ```
  Для Flatpak журналы хранятся в `~/.var/app/org.waywallen.waywallen/.local/state/waywallen/logs/daemon/`.
  Чтобы перезапустить приложение с подробным журналированием, выполните:
  ```bash
  flatpak kill org.waywallen.waywallen
  flatpak run \
    --env=WW_LOG=debug \
    org.waywallen.waywallen
  ```

- Как выполнить отладку во Flatpak?
  ```bash
  flatpak install org.waywallen.waywallen.Debug
  flatpak run --devel --command=bash org.waywallen.waywallen
  # 1. Запуск напрямую
  [📦 org.waywallen.waywallen ~]$ gdb waywallen
  (gdb) run
  Enable debuginfod for this session? (y or [n]) n
  ...
  # Получение трассировки стека
  (gdb) bt

  # 2. Или использование файла дампа памяти
  coredumpctl dump <id> -o core.save
  flatpak run --devel --filesystem=host --command=bash org.waywallen.waywallen
  [📦 org.waywallen.waywallen ~]$ gdb waywallen core.save
  ...
  ```
