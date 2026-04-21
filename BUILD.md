# Building Waywallen

End-to-end build instructions for developers. 

## System dependencies

| Dependency | Version | Notes |
|------------|---------|-------|
| Rust | stable | |
| Clang | 22+ | [LLVM-22.1.4-Linux-X64](https://github.com/llvm/llvm-project/releases/download/llvmorg-22.1.4/LLVM-22.1.4-Linux-X64.tar.xz) |
| CMake | 3.28+ | |
| Vulkan SDK | ≥ 1.1 |  |
| Qt6 | ≥ 6.10 | Quick, DBus, Protobuf |
| mpv | - |  |

## Building the Daemon

```bash
cargo build --release
```

## Building the UI and Plugins

```bash
cmake --preset clang-release -DCMAKE_INSTALL_PREFIX=install
cmake --build build/clang-release
cmake --install build/clang-release
```

## Launching

```bash
cd ./install
export QML_IMPORT_PATH=./lib/qt6/qml
../target/release/waywallen --ui ./bin/waywallen-ui --plugin ./share/waywallen
```