# Building Waywallen

End-to-end build instructions for developers. 

## System dependencies

| Dependency | Version | Notes |
|------------|---------|-------|
| Rust | stable | |
| Clang | 22+ | UI and companion C++ projects pin Clang via `CMAKE_CXX_COMPILER=clang++` + `CMAKE_LINKER=lld` |
| CMake | 3.28+ | UI requires Ninja generator (`cmake-ninja` buildsystem) |
| Vulkan SDK | ≥ 1.1 |  |
| Qt6 | ≥ 6.10 | Quick, DBus, Protobuf |

## Building the daemon (Rust)

```bash
cargo build --release
```

## Building the UI (Qt6 / QML)

The UI lives in `ui/` and is an independent CMake project (top-level target name: `waywallen-ui`).

```bash
cmake -S ui -B ui/build -G Ninja \
    -DCMAKE_BUILD_TYPE=RelWithDebInfo \
    -DCMAKE_CXX_COMPILER=clang++ \
    -DCMAKE_LINKER=lld
cmake --build ui/build
```

## Companion components

These live in sibling directories in the same repo umbrella:

| Component | Build |
|-----------|-------|
| `waywallen-bridge` | build/install with cmake |
| `open-wallpaper-engine` | build/install with cmake(preset) |
| `waywallen-mpv` | see its README |

## Launching

```bash
QML_IMPORT_PATH=$PWD/ui/build/clang-debug/qml_modules \
./target/release/waywallen --ui $PWD/ui/build/waywallen-ui

# --plugin <install-prefix>/share/waywallen
```