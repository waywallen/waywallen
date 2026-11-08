#!/usr/bin/env bash
# Build a minimal FFmpeg from source into the active conda env.
#
# Why source instead of conda-forge:
#   - control the codec / demuxer / filter set (smaller binary, fewer deps)
#   - inherit the same sysroot/glibc baseline as the rest of the project,
#     because configure picks up CC and CFLAGS / LDFLAGS from the activated
#     conda env (which the clang_linux-64 activation populates with --sysroot).
#
# Idempotent: skips the build if libavcodec.pc, the requested version, and this
# build script's checksum match the installed stamps. Set FORCE=1 to rebuild.
#
# Tunables (env vars):
#   FFMPEG_VERSION      git tag to check out, default n7.1.5
#   FFMPEG_REPOSITORY   git repository, default the official GitHub mirror
#   FORCE               set to 1 to rebuild even if the stamps match

set -euo pipefail

[[ -n "${CONDA_PREFIX:-}" ]] || {
    printf '\033[1;31mERROR:\033[0m CONDA_PREFIX not set; activate the conda env first\n' >&2
    exit 1
}

FFMPEG_VERSION="${FFMPEG_VERSION:-n7.1.5}"
FFMPEG_REPOSITORY="${FFMPEG_REPOSITORY:-https://github.com/FFmpeg/FFmpeg.git}"
FFMPEG_SRC="$CONDA_PREFIX/.ffmpeg-src"
PKG_STAMP="$CONDA_PREFIX/lib/pkgconfig/libavcodec.pc"
VERSION_STAMP="$CONDA_PREFIX/.waywallen-ffmpeg-version"
CONFIG_STAMP="$CONDA_PREFIX/.waywallen-ffmpeg-config"
CONFIG_HASH="$(sha256sum "${BASH_SOURCE[0]}")"
CONFIG_HASH="${CONFIG_HASH%% *}"

step() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }

if [[ -f "$PKG_STAMP" \
    && -f "$VERSION_STAMP" \
    && "$(<"$VERSION_STAMP")" == "$FFMPEG_VERSION" \
    && -f "$CONFIG_STAMP" \
    && "$(<"$CONFIG_STAMP")" == "$CONFIG_HASH" \
    && -z "${FORCE:-}" ]]; then
    step "FFmpeg $FFMPEG_VERSION already installed in \$CONDA_PREFIX (set FORCE=1 to rebuild)"
    exit 0
fi

step "Building FFmpeg $FFMPEG_VERSION into $CONDA_PREFIX"

if [[ -d "$FFMPEG_SRC/.git" ]]; then
    git -C "$FFMPEG_SRC" remote set-url origin "$FFMPEG_REPOSITORY"
    git -C "$FFMPEG_SRC" fetch --depth 1 origin "refs/tags/$FFMPEG_VERSION"
    git -C "$FFMPEG_SRC" checkout --detach FETCH_HEAD
else
    rm -rf "$FFMPEG_SRC"
    git clone --depth 1 --branch "$FFMPEG_VERSION" \
        "$FFMPEG_REPOSITORY" "$FFMPEG_SRC"
fi

# Curated minimal feature set. Tweak as the renderer plugins grow new format
# requirements. The decoder list covers the common still / animated image
# and video container payloads; the audio decoders exist so media_probe can
# enumerate audio tracks in mp4/mkv files even though we never play them.
DECODERS=(
    # video. The built-in `av1` decoder is hwaccel-only (no sw path); we
    # also enable `libdav1d` for the sw fallback. The decoder pick happens
    # in video_decoder.cpp: hw paths use native `av1`, sw uses libdav1d.
    h264 hevc av1 libdav1d vp8 vp9 mpeg4 mjpeg
    # image (also used for image-sequence demuxing)
    png apng webp gif bmp tiff pam pbm pgm ppm libjxl
    # audio (probe-only)
    aac mp3 opus vorbis flac pcm_s16le pcm_s16be
)
ENCODERS=()  # waywallen never encodes
DEMUXERS=(
    mov matroska
    # wavsen.image uses custom AVIO, including image2 -> image2pipe fallback.
    # Configure names for the *_pipe formats have an image_ prefix.
    image2 image2pipe gif apng ico
    image_png_pipe image_jpeg_pipe image_webp_pipe image_bmp_pipe image_tiff_pipe
    image_pam_pipe image_pbm_pipe image_pgm_pipe image_ppm_pipe image_jpegxl_pipe jpegxl_anim
    aac mp3 ogg flac wav
)
PARSERS=(
    h264 hevc av1 vp8 vp9 mjpeg
    aac mpegaudio opus vorbis flac
    png webp bmp gif pnm jpegxl
)
BSFS=(
    h264_mp4toannexb hevc_mp4toannexb
    vp9_metadata vp9_superframe
    av1_metadata
)
FILTERS=(
    scale format setpts fps null copy
    buffer buffersink abuffer abuffersink
)
PROTOCOLS=(file pipe)
# With --disable-everything, hwaccels must be enabled explicitly. VAAPI covers
# Intel/AMD via libva (mesa); Vulkan covers VK_KHR_video_decode on radv/anv.
HWACCELS=(
    h264_vaapi hevc_vaapi av1_vaapi vp8_vaapi vp9_vaapi mpeg4_vaapi mjpeg_vaapi
    h264_vulkan hevc_vulkan av1_vulkan
)

CFG_ARGS=(
    --prefix="$CONDA_PREFIX"
    # FFmpeg's configure ignores $CC by default (it hardcodes cc=cc), which
    # silently picks up /usr/bin/cc — the host gcc — and bypasses the conda
    # sysroot. Pass them explicitly so the wrapper injects --sysroot.
    --cc="${CC:-clang}"
    --cxx="${CXX:-clang++}"
    --enable-shared
    --disable-static
    --disable-programs
    --disable-doc
    --disable-debug
    --enable-pic
    --disable-everything

    # External libs. Vulkan provides VK_KHR_video_decode hwaccel
    # (h264/hevc/av1) on radv/anv. VAAPI provides hwaccel via libva (mesa) for
    # Intel/AMD. Headers come from conda-forge vulkan-headers / libva; the
    # loaders (libvulkan.so.1, libva.so.2, libva-drm.so.2) ship via
    # vulkan-loader / libva and are bundled into the AppImage. The X / audio /
    # v4l2 libs aren't in the conda env so configure auto-disables them; we
    # don't list them here to avoid flag-name churn between FFmpeg releases.
    --enable-vulkan
    --enable-vaapi
    --enable-zlib
    --disable-vdpau
    --disable-xlib --disable-libxcb
    # libdav1d for sw AV1 decode (FFmpeg's built-in av1 decoder is
    # hwaccel-only). Headers/lib come from conda-forge `dav1d`.
    --enable-libdav1d
    --enable-libjxl
)
for x in "${DECODERS[@]}";  do CFG_ARGS+=( "--enable-decoder=$x" ); done
for x in "${ENCODERS[@]}";  do CFG_ARGS+=( "--enable-encoder=$x" ); done
for x in "${DEMUXERS[@]}";  do CFG_ARGS+=( "--enable-demuxer=$x" ); done
for x in "${PARSERS[@]}";   do CFG_ARGS+=( "--enable-parser=$x"  ); done
for x in "${BSFS[@]}";      do CFG_ARGS+=( "--enable-bsf=$x"     ); done
for x in "${FILTERS[@]}";   do CFG_ARGS+=( "--enable-filter=$x"  ); done
for x in "${PROTOCOLS[@]}"; do CFG_ARGS+=( "--enable-protocol=$x" ); done
for x in "${HWACCELS[@]}";  do CFG_ARGS+=( "--enable-hwaccel=$x"  ); done

# Forward the sysroot/optimization flags exported by the conda toolchain
# activation. configure splices these into every cc invocation it makes, so
# the resulting libs hit the sysroot 2.28 glibc baseline.
[[ -n "${CFLAGS:-}"  ]] && CFG_ARGS+=( --extra-cflags="$CFLAGS" )
[[ -n "${LDFLAGS:-}" ]] && CFG_ARGS+=( --extra-ldflags="$LDFLAGS" )

(
    cd "$FFMPEG_SRC"
    make distclean >/dev/null 2>&1 || true
    ./configure "${CFG_ARGS[@]}"
    make -j"$(nproc)"
    make install
)
printf '%s\n' "$FFMPEG_VERSION" > "$VERSION_STAMP"
printf '%s\n' "$CONFIG_HASH" > "$CONFIG_STAMP"

step "FFmpeg installed; pkg-config stamp -> $PKG_STAMP"
