#!/usr/bin/env bash
# Copy a curated set of host system libraries into the active conda env and
# emit pkg-config .pc files re-rooted at $CONDA_PREFIX, so downstream cmake /
# cargo finds them there instead of /usr.
#
# Currently handled:
#   - pipewire (libpipewire-0.3 + libspa-0.2): conda-forge has no package
#     and building from source pulls alsa/dbus/glib/systemd/etc., none of
#     which match our sysroot 2.28 baseline. The host headers + .so are
#     glibc-only, so they Just Work on every distro we target.
#   - fontconfig: we want the host build (sysconfdir = /etc) so the AppImage
#     reads the user's /etc/fonts/fonts.conf at runtime; the conda-forge
#     fontconfig is rooted at the conda prefix and finds no user fonts.
#   - libpulse: wavsen's default audio backend (libpulse>=14.0); conda-forge
#     has no client-only package and the host .so is glibc-only.
#   - GBM: Lito resolves it through pkg-config and needs the host development
#     library and public header in the conda prefix.
#
# Each lib has its own pkg-config stamp; only missing ones are refreshed.
# FORCE=1 reinstalls everything. Prerequisites on the host:
#   - /usr/bin/pkg-config
#   - the host -devel package for each lib below (headers + .pc)

set -euo pipefail

[[ -n "${CONDA_PREFIX:-}" ]] || {
    printf '\033[1;31mERROR:\033[0m CONDA_PREFIX not set; activate the conda env first\n' >&2
    exit 1
}

step() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }
fail() { printf '\033[1;31mERROR:\033[0m %s\n' "$*" >&2; exit 1; }

command -v /usr/bin/pkg-config >/dev/null \
    || fail "/usr/bin/pkg-config not found; install pkg-config on the host"

# Run host pkg-config with the conda env's vars stripped — otherwise
# PKG_CONFIG_PATH would already point at $CONDA_PREFIX and our queries would
# loop back instead of finding the system layout.
host_pkgconf() {
    env -u PKG_CONFIG_PATH -u PKG_CONFIG_LIBDIR /usr/bin/pkg-config "$@"
}

# copy_headers <host_includedir> <subdir>
#   $CONDA_PREFIX/include/<subdir>  <- <host_includedir>/<subdir>
copy_headers() {
    local host_incdir="$1" subdir="$2"
    [[ -d "$host_incdir/$subdir" ]] \
        || fail "host header dir missing: $host_incdir/$subdir (install -devel package)"
    rm -rf "$CONDA_PREFIX/include/$subdir"
    cp -a "$host_incdir/$subdir" "$CONDA_PREFIX/include/"
}

# copy_header <host_includedir> <filename>
#   $CONDA_PREFIX/include/<filename> <- <host_includedir>/<filename>
copy_header() {
    local host_incdir="$1" filename="$2"
    [[ -f "$host_incdir/$filename" ]] \
        || fail "host header missing: $host_incdir/$filename (install -devel package)"
    rm -f "$CONDA_PREFIX/include/$filename"
    cp -a "$host_incdir/$filename" "$CONDA_PREFIX/include/"
}

# copy_libs <host_libdir> <basename>
#   matches lib<basename>.so* (so + SONAME + symlinks), copies into
#   $CONDA_PREFIX/lib, preserving symlinks.
copy_libs() {
    local host_libdir="$1" base="$2" found=0 f
    # Conda tools may still require another SONAME of the same library.
    shopt -s nullglob
    for f in "$host_libdir/lib${base}.so" "$host_libdir/lib${base}.so".*; do
        cp -a --remove-destination "$f" "$CONDA_PREFIX/lib/"
        found=1
    done
    shopt -u nullglob
    [[ "$found" -eq 1 ]] || fail "no lib${base}.so* under $host_libdir"
}

# copy_dir_if_present <host_dir> <dest_parent>
#   $dest_parent/$(basename host_dir) <- host_dir, no-op if missing
copy_dir_if_present() {
    local src="$1" dest_parent="$2"
    [[ -d "$src" ]] || return 0
    local name; name="$(basename "$src")"
    rm -rf "$dest_parent/$name"
    cp -a "$src" "$dest_parent/"
}

mkdir -p "$CONDA_PREFIX/include" "$CONDA_PREFIX/lib" "$CONDA_PREFIX/lib/pkgconfig"

# GBM
install_gbm() {
    local stamp="$CONDA_PREFIX/lib/pkgconfig/gbm.pc"
    if [[ -f "$stamp" && -f "$CONDA_PREFIX/include/gbm.h" \
        && -e "$CONDA_PREFIX/lib/libgbm.so" && -z "${FORCE:-}" ]]; then
        step "GBM already installed in \$CONDA_PREFIX (set FORCE=1 to refresh)"
        return 0
    fi

    host_pkgconf --exists gbm \
        || fail "host has no gbm.pc; install mesa-libgbm-devel / libgbm-dev"

    local gbm_ver gbm_libdir gbm_incdir gbm_pcdir
    gbm_ver="$(host_pkgconf --modversion gbm)"
    gbm_libdir="$(host_pkgconf --variable=libdir gbm)"
    gbm_incdir="$(host_pkgconf --variable=includedir gbm)"
    gbm_pcdir="$(host_pkgconf --variable=pcfiledir gbm)"
    [[ -f "$gbm_pcdir/gbm.pc" ]] || fail "host gbm.pc missing: $gbm_pcdir/gbm.pc"

    step "Copying GBM $gbm_ver from host"

    copy_header "$gbm_incdir" gbm.h
    copy_libs "$gbm_libdir" gbm
    cp -a "$gbm_pcdir/gbm.pc" "$stamp"
    sed -i \
        -e "s|^prefix=.*|prefix=$CONDA_PREFIX|" \
        -e 's|^includedir=.*|includedir=${prefix}/include|' \
        -e 's|^libdir=.*|libdir=${prefix}/lib|' \
        "$stamp"

    step "GBM installed -> $stamp"
}

# libxml2
install_libxml2() {
    local stamp="$CONDA_PREFIX/lib/pkgconfig/libxml-2.0.pc"
    if [[ -f "$stamp" && -f "$CONDA_PREFIX/include/libxml2/libxml/parser.h" \
        && -e "$CONDA_PREFIX/lib/libxml2.so" && -z "${FORCE:-}" ]]; then
        step "libxml2 already installed in \$CONDA_PREFIX (set FORCE=1 to refresh)"
        return 0
    fi

    host_pkgconf --exists "libxml-2.0 >= 2.9.14" \
        || fail "host has no libxml-2.0 >= 2.9.14; install libxml2-devel / libxml2-dev"

    local xml_ver xml_libdir xml_incdir xml_pcdir
    xml_ver="$(host_pkgconf --modversion libxml-2.0)"
    xml_libdir="$(host_pkgconf --variable=libdir libxml-2.0)"
    xml_incdir="$(host_pkgconf --variable=includedir libxml-2.0)"
    xml_pcdir="$(host_pkgconf --variable=pcfiledir libxml-2.0)"
    [[ -f "$xml_pcdir/libxml-2.0.pc" ]] \
        || fail "host libxml-2.0.pc missing: $xml_pcdir/libxml-2.0.pc"

    step "Copying libxml2 $xml_ver from host"

    copy_headers "$xml_incdir" libxml2
    copy_libs "$xml_libdir" xml2
    cp -a "$xml_pcdir/libxml-2.0.pc" "$stamp"
    sed -i \
        -e "s|^prefix=.*|prefix=$CONDA_PREFIX|" \
        -e 's|^exec_prefix=.*|exec_prefix=${prefix}|' \
        -e 's|^includedir=.*|includedir=${prefix}/include|' \
        -e 's|^libdir=.*|libdir=${prefix}/lib|' \
        "$stamp"

    step "libxml2 installed -> $stamp"
}

# pipewire (libpipewire-0.3 + libspa-0.2)
install_pipewire() {
    local stamp="$CONDA_PREFIX/lib/pkgconfig/libpipewire-0.3.pc"
    if [[ -f "$stamp" && -z "${FORCE:-}" ]]; then
        step "pipewire already installed in \$CONDA_PREFIX (set FORCE=1 to refresh)"
        return 0
    fi

    host_pkgconf --exists libpipewire-0.3 \
        || fail "host has no libpipewire-0.3.pc; install pipewire-devel / libpipewire-0.3-dev"
    host_pkgconf --exists libspa-0.2 \
        || fail "host has no libspa-0.2.pc; install pipewire-devel / libspa-0.2-dev"

    local pw_ver spa_ver pw_libdir pw_incdir spa_incdir pw_moduledir spa_plugindir
    pw_ver="$(host_pkgconf --modversion libpipewire-0.3)"
    spa_ver="$(host_pkgconf --modversion libspa-0.2)"
    pw_libdir="$(host_pkgconf --variable=libdir libpipewire-0.3)"
    pw_incdir="$(host_pkgconf --variable=includedir libpipewire-0.3)"
    spa_incdir="$(host_pkgconf --variable=includedir libspa-0.2)"
    pw_moduledir="$(host_pkgconf --variable=moduledir libpipewire-0.3)"
    spa_plugindir="$(host_pkgconf --variable=plugindir libspa-0.2)"

    step "Copying pipewire $pw_ver + libspa $spa_ver from host"

    copy_headers "$pw_incdir"  pipewire-0.3
    copy_headers "$spa_incdir" spa-0.2
    copy_libs    "$pw_libdir"  pipewire-0.3
    # Runtime: pipewire loader dlopens these. Build doesn't need them but the
    # bundled AppImage does, so stage them here once.
    copy_dir_if_present "$spa_plugindir" "$CONDA_PREFIX/lib"
    copy_dir_if_present "$pw_moduledir"  "$CONDA_PREFIX/lib"

    cat > "$CONDA_PREFIX/lib/pkgconfig/libspa-0.2.pc" <<EOF
prefix=$CONDA_PREFIX
includedir=\${prefix}/include
libdir=\${prefix}/lib

plugindir=\${libdir}/spa-0.2

Name: libspa
Description: Simple Plugin API
Version: $spa_ver
Cflags: -I\${includedir}/spa-0.2 -D_REENTRANT
EOF

    cat > "$stamp" <<EOF
prefix=$CONDA_PREFIX
includedir=\${prefix}/include
libdir=\${prefix}/lib

moduledir=\${libdir}/pipewire-0.3

Name: libpipewire
Description: PipeWire Interface
Version: $pw_ver
Requires: libspa-0.2
Libs: -L\${libdir} -lpipewire-0.3
Cflags: -I\${includedir}/pipewire-0.3 -D_REENTRANT
EOF
    step "pipewire installed -> $stamp"
}

# fontconfig
# sysconfdir is intentionally left at the host's value (typically /etc) — that
# path is also baked into libfontconfig.so itself at host build time, and we
# want the AppImage to read the user's /etc/fonts/fonts.conf at runtime so
# system fonts are visible.
install_fontconfig() {
    local stamp="$CONDA_PREFIX/lib/pkgconfig/fontconfig.pc"
    if [[ -f "$stamp" && -z "${FORCE:-}" ]]; then
        step "fontconfig already installed in \$CONDA_PREFIX (set FORCE=1 to refresh)"
        return 0
    fi

    host_pkgconf --exists fontconfig \
        || fail "host has no fontconfig.pc; install fontconfig-devel / libfontconfig-dev"

    local fc_ver fc_libdir fc_incdir fc_sysconfdir fc_confdir fc_cachedir
    fc_ver="$(host_pkgconf --modversion fontconfig)"
    fc_libdir="$(host_pkgconf --variable=libdir fontconfig)"
    fc_incdir="$(host_pkgconf --variable=includedir fontconfig)"
    fc_sysconfdir="$(host_pkgconf --variable=sysconfdir fontconfig)"
    fc_confdir="$(host_pkgconf --variable=confdir fontconfig)"
    fc_cachedir="$(host_pkgconf --variable=cachedir fontconfig)"
    : "${fc_sysconfdir:=/etc}"
    : "${fc_confdir:=$fc_sysconfdir/fonts}"
    : "${fc_cachedir:=/var/cache/fontconfig}"

    step "Copying fontconfig $fc_ver from host"

    copy_headers "$fc_incdir" fontconfig
    copy_libs    "$fc_libdir" fontconfig

    cat > "$stamp" <<EOF
prefix=$CONDA_PREFIX
exec_prefix=\${prefix}
libdir=\${prefix}/lib
includedir=\${prefix}/include
sysconfdir=$fc_sysconfdir
localstatedir=/var
PACKAGE=fontconfig
confdir=$fc_confdir
cachedir=$fc_cachedir

Name: Fontconfig
Description: Font configuration and customization library
Version: $fc_ver
Requires: freetype2
Libs: -L\${libdir} -lfontconfig
Cflags: -I\${includedir}
EOF
    step "fontconfig installed -> $stamp"
}

# libpulse (PulseAudio client) — wavsen::ffi::pulse
install_pulse() {
    local stamp="$CONDA_PREFIX/lib/pkgconfig/libpulse.pc"
    if [[ -f "$stamp" && -z "${FORCE:-}" ]]; then
        step "libpulse already installed in \$CONDA_PREFIX (set FORCE=1 to refresh)"
        return 0
    fi

    host_pkgconf --exists "libpulse >= 14.0" \
        || fail "host has no libpulse >= 14.0; install pulseaudio-libs-devel / libpulse-dev"

    local pa_ver pa_libdir pa_incdir
    pa_ver="$(host_pkgconf --modversion libpulse)"
    pa_libdir="$(host_pkgconf --variable=libdir libpulse)"
    pa_incdir="$(host_pkgconf --variable=includedir libpulse)"

    step "Copying libpulse $pa_ver from host"

    copy_headers "$pa_incdir" pulse
    copy_libs    "$pa_libdir" pulse
    # libpulse.so NEEDs libpulsecommon-*.so from $libdir/pulseaudio via an
    # $ORIGIN/pulseaudio rpath; stage that dir so the AppImage resolves it.
    copy_dir_if_present "$pa_libdir/pulseaudio" "$CONDA_PREFIX/lib"

    cat > "$stamp" <<EOF
prefix=$CONDA_PREFIX
exec_prefix=\${prefix}
libdir=\${prefix}/lib
includedir=\${prefix}/include

Name: libpulse
Description: PulseAudio Client Interface
Version: $pa_ver
Libs: -L\${libdir} -lpulse
Cflags: -D_REENTRANT -I\${includedir}
EOF
    step "libpulse installed -> $stamp"
}

install_gbm
install_libxml2
install_pipewire
install_fontconfig
install_pulse
