#!/usr/bin/env bash
# Build waywallen end-to-end and produce a single-file AppImage at:
#     <repo>/waywallen-<version>-<architecture>.AppImage

set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ENV_FILE="$PROJECT_DIR/environment.yml"
ENV_NAME="${WAYWALLEN_CONDA_ENV:-waywallen}"
ENV_PREFIX="${WAYWALLEN_CONDA_PREFIX:-$PROJECT_DIR/build/conda-envs/$ENV_NAME}"
TMP_DIR="${TMPDIR:-/tmp}"
WAYWALLEN_DISPLAY_REPO="${WAYWALLEN_DISPLAY_REPO:-https://github.com/waywallen/waywallen-display.git}"
WAYWALLEN_DISPLAY_REF="${WAYWALLEN_DISPLAY_REF:-3c9c4a736677182695bbe5bc36fadaf8e211a091}"
APPDIR="$PROJECT_DIR/build/AppDir"
INSTALL_DIR="$APPDIR/usr"
TOOLS_DIR="$PROJECT_DIR/build/_tools"
WAYWALLEN_DISPLAY_SRC="${WAYWALLEN_DISPLAY_SRC:-$TMP_DIR/waywallen-display-src}"

info() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }
fail() { printf '\033[1;31mERROR:\033[0m %s\n' "$*" >&2; exit 1; }

host_arch="$(uname -m)"
case "$host_arch" in
    x86_64)
        APPIMAGE_ARCH="x86_64"
        CONDA_TARGET="linux-64"
        CONDA_GBM_ARCH="x86_64"
        ;;
    aarch64|arm64)
        APPIMAGE_ARCH="aarch64"
        CONDA_TARGET="linux-aarch64"
        CONDA_GBM_ARCH="aarch64"
        ;;
    *) fail "unsupported host architecture: $host_arch" ;;
esac

ENV_MERGED_FILE="$PROJECT_DIR/build/environment-${CONDA_TARGET}.yml"
CONDA_TARGET_PACKAGES=(
    "clang_${CONDA_TARGET}=22"
    "clangxx_${CONDA_TARGET}=22"
    "sysroot_${CONDA_TARGET}=2.28"
    "mesa-libgbm-devel-conda-${CONDA_GBM_ARCH}=23.1.4"
)

WAYWALLEN_VERSION="$(awk -F'"' '/^version = "/ { print $2; exit }' "$PROJECT_DIR/lito.toml")"
[[ -n "$WAYWALLEN_VERSION" ]] || fail "could not parse version from lito.toml"

if [[ -n "${WAYWALLEN_BUILD_VERSION:-}" ]]; then
    BUILD_TAG="$WAYWALLEN_BUILD_VERSION"
elif git -C "$PROJECT_DIR" rev-parse --short=7 HEAD >/dev/null 2>&1; then
    SHA="$(git -C "$PROJECT_DIR" rev-parse --short=7 HEAD)"
    DIRTY=""
    git -C "$PROJECT_DIR" diff --quiet --ignore-submodules HEAD 2>/dev/null || DIRTY="-dirty"
    if [[ -z "$DIRTY" ]] \
        && git -C "$PROJECT_DIR" describe --tags --exact-match --match "v$WAYWALLEN_VERSION" \
                >/dev/null 2>&1; then
        BUILD_TAG="$WAYWALLEN_VERSION"
    else
        BUILD_TAG="$WAYWALLEN_VERSION-g$SHA$DIRTY"
    fi
else
    BUILD_TAG="$WAYWALLEN_VERSION"
fi

rm -rf "$APPDIR"

APPIMAGE_OUT="$PROJECT_DIR/waywallen-$BUILD_TAG-$APPIMAGE_ARCH.AppImage"
ZSYNC_OUT="$APPIMAGE_OUT.zsync"
UPDATE_INFORMATION="gh-releases-zsync|waywallen|waywallen|latest|waywallen-*-$APPIMAGE_ARCH.AppImage.zsync"
info "Building $APPIMAGE_ARCH AppImage tagged as $BUILD_TAG"

command -v conda >/dev/null \
    || fail "conda not found. Install Miniconda first: https://docs.conda.io/projects/miniconda/"
command -v cargo >/dev/null \
    || fail "cargo not found. Install rustup first: https://rustup.rs/"
command -v curl >/dev/null || fail "curl not found"
command -v git >/dev/null || fail "git not found"
command -v python3 >/dev/null || fail "python3 not found"
command -v zsyncmake >/dev/null || fail "zsyncmake not found. Install zsync first"
[[ -f "$ENV_FILE" ]] || fail "missing $ENV_FILE"

if [[ -n "${LITO_BIN:-}" ]] && "$LITO_BIN" --help >/dev/null 2>&1; then
    info "Using lito: $LITO_BIN"
else
    info "Installing lito"
    curl -fsSL https://raw.githubusercontent.com/litocpp/lito/main/install.sh | bash
    LITO_BIN="$HOME/.local/bin/lito"
    [[ -x "$LITO_BIN" ]] || fail "lito installer did not create $LITO_BIN"
fi

export XDG_CACHE_HOME="${XDG_CACHE_HOME:-$PROJECT_DIR/build/.cache}"
export CONDARC="${CONDARC:-$PROJECT_DIR/build/condarc}"
if [[ ! -f "$CONDARC" ]]; then
    mkdir -p "$(dirname "$CONDARC")"
    {
        printf 'channels:\n'
        printf '  - conda-forge\n'
        printf '  - nodefaults\n'
        printf 'channel_priority: strict\n'
        printf 'default_channels: []\n'
        printf 'auto_activate_base: false\n'
    } > "$CONDARC"
fi

info "Writing merged conda env: $ENV_MERGED_FILE"
mkdir -p "$(dirname "$ENV_MERGED_FILE")"
python3 - "$ENV_FILE" "$ENV_MERGED_FILE" "${CONDA_TARGET_PACKAGES[@]}" <<'PY'
import sys
from pathlib import Path

src = Path(sys.argv[1])
dst = Path(sys.argv[2])
extras = sys.argv[3:]

lines = src.read_text().splitlines()
existing = set()
in_dependencies = False
for raw in lines:
    line = raw.split("#", 1)[0].rstrip()
    if line.strip() == "dependencies:":
        in_dependencies = True
        continue
    if not in_dependencies:
        continue
    stripped = line.strip()
    if stripped.startswith("- "):
        existing.add(stripped[2:].strip())

extras = [pkg for pkg in extras if pkg not in existing]
if extras:
    insert_at = len(lines)
    for idx, raw in enumerate(lines):
        if raw.strip() == "- llvm-tools=22":
            insert_at = idx + 1
            break

    additions = [f"  - {pkg}" for pkg in extras]
    if insert_at < len(lines) and lines[insert_at].strip():
        additions.append("")
    lines[insert_at:insert_at] = additions

dst.write_text("\n".join(lines) + "\n")
PY

info "Preparing conda env: $ENV_PREFIX"
set +u
# shellcheck disable=SC1091
source "$(conda info --base)/etc/profile.d/conda.sh"
set -u

if [[ -d "$ENV_PREFIX/conda-meta" ]]; then
    conda env update -p "$ENV_PREFIX" -f "$ENV_MERGED_FILE" --prune
else
    conda env create -p "$ENV_PREFIX" -f "$ENV_MERGED_FILE"
fi

set +u
conda activate "$ENV_PREFIX"
set -u

# Keep Cargo's linker on the same sysroot as Conda's native libraries.
RUST_HOST_TARGET="$(rustc -vV | awk '/^host: / { print $2 }')"
[[ -n "$RUST_HOST_TARGET" ]] || fail "could not read the rustc host target"
[[ -n "${CC:-}" ]] || fail "Conda C compiler is unavailable"
RUST_LINKER_ENV="CARGO_TARGET_${RUST_HOST_TARGET^^}_LINKER"
RUST_LINKER_ENV="${RUST_LINKER_ENV//-/_}"
export "${RUST_LINKER_ENV}=${CC}"

bash "$PROJECT_DIR/scripts/build_ffmpeg.sh"
bash "$PROJECT_DIR/scripts/copy_syslibs.sh"

QT_VER="$("$CONDA_PREFIX/bin/qmake6" -query QT_VERSION)"
if [[ ! -f "$CONDA_PREFIX/lib/cmake/Qt6Protobuf/Qt6ProtobufConfig.cmake" ]]; then
    info "Building qtgrpc v$QT_VER"
    QTGRPC_SRC="$PROJECT_DIR/build/_qtgrpc-src"
    QTGRPC_BUILD="$PROJECT_DIR/build/_qtgrpc-build"
    rm -rf "$QTGRPC_SRC" "$QTGRPC_BUILD"
    git clone --depth 1 --branch "v$QT_VER" \
        https://code.qt.io/qt/qtgrpc.git "$QTGRPC_SRC"
    cmake -S "$QTGRPC_SRC" -B "$QTGRPC_BUILD" -G Ninja \
        -DCMAKE_BUILD_TYPE=Release \
        -DCMAKE_C_COMPILER=clang \
        -DCMAKE_CXX_COMPILER=clang++ \
        -DCMAKE_SYSROOT="$CONDA_BUILD_SYSROOT" \
        -DCMAKE_PREFIX_PATH="$CONDA_PREFIX" \
        -DCMAKE_INSTALL_PREFIX="$CONDA_PREFIX" \
        -DQT_FEATURE_grpc=OFF \
        -DBUILD_TESTING=OFF \
        -DQT_BUILD_EXAMPLES=OFF \
        -DQT_BUILD_TESTS=OFF
    cmake --build "$QTGRPC_BUILD" --parallel
    cmake --install "$QTGRPC_BUILD"
fi

info "Building and installing waywallen into AppDir: $APPDIR"
pushd "$PROJECT_DIR"
"$LITO_BIN" install \
    --no-config \
    --locked \
    --profile release \
    --prefix "$INSTALL_DIR" \
    --config "tools.cmake.search-path=[\"$CONDA_PREFIX\"]"
popd

info "Building and installing waywallen-layer-shell and waywallen-x11"
if [[ -d "$WAYWALLEN_DISPLAY_SRC/.git" ]]; then
    git -C "$WAYWALLEN_DISPLAY_SRC" remote set-url origin "$WAYWALLEN_DISPLAY_REPO"
else
    rm -rf "$WAYWALLEN_DISPLAY_SRC"
    git clone "$WAYWALLEN_DISPLAY_REPO" "$WAYWALLEN_DISPLAY_SRC"
fi
git -C "$WAYWALLEN_DISPLAY_SRC" fetch --tags origin "$WAYWALLEN_DISPLAY_REF" \
    || git -C "$WAYWALLEN_DISPLAY_SRC" fetch --tags origin
git -C "$WAYWALLEN_DISPLAY_SRC" checkout --detach "$WAYWALLEN_DISPLAY_REF"
pushd "$WAYWALLEN_DISPLAY_SRC"
cargo build \
    --bin waywallen-layer-shell \
    --bin waywallen-x11 \
    --features x11 \
    --release \
    --locked
popd
for display_bin in waywallen-layer-shell waywallen-x11; do
    install -Dm755 \
        "$WAYWALLEN_DISPLAY_SRC/target/release/$display_bin" \
        "$INSTALL_DIR/bin/$display_bin"
done

mkdir -p "$TOOLS_DIR"
LINUXDEPLOY="$TOOLS_DIR/linuxdeploy-$APPIMAGE_ARCH.AppImage"
LINUXDEPLOY_QT="$TOOLS_DIR/linuxdeploy_plugin_qt"
APPIMAGETOOL="$TOOLS_DIR/appimagetool-$APPIMAGE_ARCH.AppImage"
download_if_missing() {
    local url="$1" dest="$2"
    if [[ ! -x "$dest" ]]; then
        info "Downloading $(basename "$dest")"
        curl -fsSL --retry 3 -o "$dest" "$url"
        chmod +x "$dest"
    fi
}
download_if_missing \
    "https://github.com/linuxdeploy/linuxdeploy/releases/download/continuous/linuxdeploy-$APPIMAGE_ARCH.AppImage" \
    "$LINUXDEPLOY"
download_if_missing \
    "https://github.com/linuxdeploy/linuxdeploy-plugin-qt/releases/download/continuous/linuxdeploy-plugin-qt-$APPIMAGE_ARCH.AppImage" \
    "$LINUXDEPLOY_QT"
download_if_missing \
    "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$APPIMAGE_ARCH.AppImage" \
    "$APPIMAGETOOL"

APPRUN_TMP="$(mktemp -t waywallen-AppRun.XXXXXX)"
trap 'rm -f "$APPRUN_TMP"' EXIT
cat > "$APPRUN_TMP" <<'APPEOF'
#!/usr/bin/env bash
HERE="$(dirname "$(readlink -f "$0")")"
export LD_LIBRARY_PATH="$HERE/usr/lib:${LD_LIBRARY_PATH:-}"
export QT_PLUGIN_PATH="$HERE/usr/plugins:${QT_PLUGIN_PATH:-}"
export QML2_IMPORT_PATH="$HERE/usr/qml:${QML2_IMPORT_PATH:-}"
export QML_IMPORT_PATH="$QML2_IMPORT_PATH"
exec "$HERE/usr/bin/waywallen" "$@"
APPEOF
chmod +x "$APPRUN_TMP"

info "Staging dependencies into AppDir"
DESKTOP_FILE="$INSTALL_DIR/share/applications/org.waywallen.waywallen.desktop"
ICON_FILE="$INSTALL_DIR/share/icons/hicolor/scalable/apps/org.waywallen.waywallen.svg"
[[ -f "$DESKTOP_FILE" ]] || fail "missing .desktop file: $DESKTOP_FILE"
[[ -f "$ICON_FILE" ]] || fail "missing icon: $ICON_FILE"

pushd "$TOOLS_DIR"
"$LINUXDEPLOY_QT" --appimage-extract
"$LINUXDEPLOY" --appimage-extract
LINUXDEPLOY="$TOOLS_DIR/squashfs-root/AppRun"
popd

cd "$PROJECT_DIR/build"
LINUXDEPLOY_EXECUTABLE_ARGS=(
    --executable "$INSTALL_DIR/bin/waywallen-ui"
    --executable "$INSTALL_DIR/bin/waywallen-video-renderer"
)
PATH="$TOOLS_DIR:$PATH" \
LD_LIBRARY_PATH="$INSTALL_DIR/lib:$CONDA_PREFIX/lib" \
QMAKE="$CONDA_PREFIX/bin/qmake6" \
EXTRA_PLATFORM_PLUGINS="libqwayland.so" \
EXTRA_QT_PLUGINS="wayland-decoration-client;wayland-shell-integration" \
QML_SOURCES_PATHS="$PROJECT_DIR/ui/qml" \
"$LINUXDEPLOY" \
    --appdir "$APPDIR" \
    --plugin qt \
    "${LINUXDEPLOY_EXECUTABLE_ARGS[@]}" \
    --desktop-file "$DESKTOP_FILE" \
    --icon-file "$ICON_FILE" \
    --custom-apprun "$APPRUN_TMP"

cp -rv "$CONDA_PREFIX/lib/qt6/plugins/wayland-graphics-integration-client" "$APPDIR/usr/plugins/"
cp -v "$CONDA_PREFIX/lib/libstdc++.so.6" "$APPDIR/usr/lib/"
cp -v "$CONDA_PREFIX/lib/libgcc_s.so.1" "$APPDIR/usr/lib/"

pushd "$APPDIR"
rm -rf ./usr/lib/qt6
rm -rf ./usr/lib/libQt6QuickTemplates*
rm -rf ./usr/lib/libQt6QuickControls2*
rm -rf ./usr/lib/libQt6QuickDialogs*
rm -rf ./usr/lib/libQt6QuickParticles.so.?
rm -rf ./usr/lib/libQt6QuickShapesDesignHelpers.so.?
rm -rf ./usr/lib/libvulkan.so.1 ./lib/libva*
rm -rf ./usr/lib/libgcc_s.so.1
rm -rf ./usr/lib/libdbus-1.so.3
rm -rf ./usr/lib/libcom_err.so.3
rm -rf ./usr/lib/libkrb5*
rm -rf ./usr/lib/libk5crypto.so.3
rm -rf ./usr/lib/libgssapi_krb5*
rm -rf ./usr/lib/libxkbcommon*
rm -rf ./usr/lib/*.a
popd

info "Pruning unused QuickControls2 styles"
QUICKCONTROLS2_PRUNE=(Basic Fusion FluentWinUI3 Imagine Material Universal designer)
for style in "${QUICKCONTROLS2_PRUNE[@]}"; do
    for libdir in "$APPDIR/usr/lib" "$APPDIR/usr/lib64"; do
        [[ -d "$libdir" ]] || continue
        find "$libdir" -maxdepth 1 -type f \
            -name "libQt6QuickControls2${style}*.so*" -print -delete 2>/dev/null || true
    done
    rm -rfv "$APPDIR/usr/qml/QtQuick/Controls/${style}" 2>/dev/null || true
done

info "Packing AppImage"
rm -f "$APPIMAGE_OUT" "$ZSYNC_OUT"
pushd "$PROJECT_DIR"
PATH="$TOOLS_DIR:$PATH" \
ARCH="$APPIMAGE_ARCH" \
"$APPIMAGETOOL" --appimage-extract-and-run \
    --no-appstream \
    --updateinformation "$UPDATE_INFORMATION" \
    "$APPDIR" "$APPIMAGE_OUT"
popd
[[ -f "$APPIMAGE_OUT" ]] || fail "AppImage build failed"
[[ -f "$ZSYNC_OUT" ]] || fail "zsync metadata generation failed"

cat <<EOF

Build complete: $APPIMAGE_OUT
Update metadata: $ZSYNC_OUT

Run it:
    chmod +x "$APPIMAGE_OUT"
    "$APPIMAGE_OUT"

Rebuild: re-run ./scripts/build_appimage.sh.
EOF
