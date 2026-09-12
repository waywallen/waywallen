# Building Waywallen

## System dependencies

| Dependency | Version | Notes |
|------------|---------|-------|
| [Lito](https://github.com/litocpp/lito) | latest | Project build entry |
| Rust | stable | |
| Clang | 22+ | [LLVM-22.1.8-Linux-X64](https://github.com/llvm/llvm-project/releases/download/llvmorg-22.1.8/LLVM-22.1.8-Linux-X64.tar.xz) |
| CMake | 3.28+ | Host tool used by Lito source dependency providers |
| Ninja | - | Build tool used by Lito source dependency providers |
| Vulkan loader | ≥ 1.1 | runtime: `vulkan-icd-loader` (Arch) · `libvulkan1` (Debian/Ubuntu) · `vulkan-loader` (Fedora) |
| Vulkan headers | ≥ 1.1 | **build-time**, provides `vulkan/vulkan.h`: `vulkan-headers` (Arch/Fedora) · `libvulkan-dev` (Debian/Ubuntu) |
| Qt6 | ≥ 6.10 | Quick, DBus, Protobuf, ProtobufQuick, QuickControls2, WebSockets |
| GBM | - | Development package |
| ffmpeg | - | Development packages used by the renderer plugins |

## Build, install, run

```bash
lito build --profile release
lito install --prefix install --profile release
```

This produces under `install/`:

```
install/bin/
    waywallen                          # daemon (Rust)
    waywallen-ui                       # Qt/QML UI
install/lib/
    libwaywallen-bridge.so             # renderer bridge library
    pkgconfig/waywallen-bridge.pc      # pkg-config metadata
install/share/waywallen/plugins/
    org.waywallen.image/{plugin.toml, files.txt, main.lua, image/..., bin/waywallen-image-renderer}
    org.waywallen.video/{plugin.toml, files.txt, main.lua, video/..., bin/waywallen-video-renderer}
    org.waywallen.wallhaven/{plugin.toml, files.txt, main.lua, wallhaven/...}
install/share/{applications,metainfo,icons/...}/
```

Use `--profile debug` for development artifacts. To build or test a single package, select it
with `-p`:

```bash
lito build -p waywallen-ui --profile debug
lito test -p waywallen-bridge --profile debug
```

`waywallen-layer-shell` lives in the `waywallen-display` Cargo package. It is not built or
installed by the normal Lito flow; packaging that bundles the display backend builds it from the
`waywallen-display` repository.

## Launching

```bash
cd install
export QML_IMPORT_PATH="$PWD/lib/qt6/qml"
export LD_LIBRARY_PATH="$PWD/lib${LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}"
./bin/waywallen --ui ./bin/waywallen-ui --plugin ./share/waywallen
```

## Packaging

Build release artifacts and stage them into the package root with Lito:

```bash
lito build --profile release
lito install --prefix package-root/usr --profile release
```

Packages should consume the staged install tree. Lito is the public project build interface; CMake
is only used internally by source dependency providers.

The protocol XMLs (`protocol/*.xml`) and `proto/control.proto` / `proto/filter.proto` are build-time
codegen inputs and are not shipped in the package. Read them from the source tree if you need to
implement a third-party client.

A third-party client asks the daemon what it speaks instead of comparing release strings: read the
`Capabilities` property of `org.waywallen.waywallen.Daemon1`. The first entry is the control-plane
revision (`control.v1`), the rest are optional features to check by name before use. An
`UnknownProperty` error means a daemon older than the property; `Version` identifies a build, not a
contract.
