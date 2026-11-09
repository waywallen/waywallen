<p align="center">
  <img src="ui/assets/waywallen-ui.svg" alt="Waywallen" width="128" />
</p>

<h1 align="center">Waywallen</h1>

<p align="center"><strong> Gestor de fondos de pantalla para Linux </strong></p>

<a href="README.md">English README</a> · <a href="README.CN.md">中文 README</a> · <a href="README.RU.md">Русский README</a> · <a href="https://discord.gg/2xEdmMrhRF">Discord</a>

---

Waywallen es una solución de fondos de pantalla dinámicos para escritorios Linux.<br>
Empezó su vida como un plugin de Wallpaper Engine para KDE.

---

## Capturas de pantalla

<p align="center">
  <img src="ui/assets/main_page.webp" alt="Página principal de Waywallen" width="720" />
</p>

## Inicio rápido

### Instalación

**Binarios precompilados**  
Descarga el último AppImage desde la [página de Releases](https://github.com/waywallen/waywallen/releases).

**Flatpak**

<a href='https://flathub.org/es/apps/org.waywallen.waywallen'>
<img width='240' alt='Disponible en Flathub' src='https://flathub.org/api/badge?locale=es'/>
</a>

**Desde el código fuente**  
Consulta [BUILD.md](BUILD.md).  

Waywallen se construye con [Lito](https://github.com/litocpp/lito) (por favor, dale una estrella)  

### Integración con el escritorio

| Escritorio | Integración | Entrada de ratón | Pausa automática |
|---------|-------------|:-----------:|:----------:|
| **KDE Plasma** | [waywallen-display](https://github.com/waywallen/waywallen-display/) | ✅ | ✅ |
| **GNOME** | [waywallen-display](https://github.com/waywallen/waywallen-display/) | ✅ | ✅ |
| **Hyprland** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **Niri** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **Wayfire** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **COSMIC** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ✅ |
| **Sway** | [waywallen-display/layer_shell](https://github.com/waywallen/waywallen-display/tree/main/src/bin/layer_shell) | ✅ | ❌ |

## Problemas conocidos
- Los fondos web en GPU NVIDIA requieren desactivar `shared_texture_enabled` en los ajustes del renderizador web.
- Flatpak necesita el permiso D-Bus `--talk-name=org.mpris.MediaPlayer2.*` para leer la información de la pista que se está reproduciendo. Concédelo al usuario actual con:
  ```bash
  flatpak override --user --talk-name='org.mpris.MediaPlayer2.*' org.waywallen.waywallen
  ```

## Plugins de fondos de pantalla

- Plugin de imágenes
- Plugin de vídeo
  - Decodificación por hardware mediante Vulkan y VA-API
- Plugin de glTF
- Plugin de [Wallhaven](https://wallhaven.cc/)
- Plugin de [Sketchfab](https://sketchfab.com/)

### Plugins de terceros

- [open-wallpaper-engine](https://github.com/waywallen/open-wallpaper-engine)
  - Compatibilidad con escenas
  - Compatibilidad con contenido web

> [!NOTE]
> Para instalar un plugin de terceros, descarga manualmente su archivo ZIP e instálalo desde la página de plugins de la interfaz.<br>
> Después de la instalación, Waywallen te avisará de las actualizaciones del plugin y las instalará.

> [!WARNING]
> **Los plugins de terceros son código de confianza.** El punto de entrada Lua de un plugin se ejecuta dentro del daemon de Waywallen con los privilegios de tu cuenta de usuario: igual que cualquier aplicación que decidas instalar, puede leer y escribir tus archivos y acceder a la red. **Instala solo plugins de confianza**, preferiblemente tras revisar su código fuente.

## Preguntas frecuentes

- ¿Cómo funciona la decodificación de vídeo por hardware?  
  El modo `auto`, que es el predeterminado, usa el siguiente orden de alternativas:  
  `vulkan -> vaapi -> sw`  
  En lugar de `auto`, puedes elegir el modo `hwdec` en los ajustes de `waywallen-video`.  

  No tenemos previsto añadir un backend específico para NVDEC.  
  Los usuarios de NVIDIA deberían usar [nvidia-vaapi-driver](https://github.com/elFarto/nvidia-vaapi-driver) para exponer NVDEC a través de VA-API.

- ¿Cómo obtengo los registros (logs)?  
  Los registros diarios del daemon se escriben en `~/.local/state/waywallen/logs/daemon/` (o en `$XDG_STATE_HOME/waywallen/logs/daemon/`) con el nombre `waywallen.AAAA-MM-DD.log` (se conservan hasta 8); `waywallen-current.log` apunta al archivo activo.
  Para aumentar el nivel de detalle, detén el daemon en ejecución y reinícialo con:
  ```bash
  export WW_LOG=debug
  ./waywallen
  ```
  En Flatpak, los registros se guardan en `~/.var/app/org.waywallen.waywallen/.local/state/waywallen/logs/daemon/`.
  Reinícialo con registro detallado así:
  ```bash
  flatpak kill org.waywallen.waywallen
  flatpak run \
    --env=WW_LOG=debug \
    org.waywallen.waywallen
  ```

- ¿Cómo depuro en Flatpak?  
  ```bash
  flatpak install org.waywallen.waywallen.Debug
  flatpak run --devel --command=bash org.waywallen.waywallen
  # 1. Ejecutar directamente
  [📦 org.waywallen.waywallen ~]$ gdb waywallen
  (gdb) run
  Enable debuginfod for this session? (y or [n]) n
  ...
  # Obtener la traza de la pila
  (gdb) bt

  # 2. O usar un archivo de volcado de memoria (core dump)
  coredumpctl dump <id> -o core.save
  flatpak run --devel --filesystem=host --command=bash org.waywallen.waywallen
  [📦 org.waywallen.waywallen ~]$ gdb waywallen core.save
  ...
  ```
