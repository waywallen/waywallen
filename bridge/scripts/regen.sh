#!/usr/bin/env bash
# Regenerate the C protocol bindings (include/waywallen-bridge/ipc_v1.h
# and src/ipc_v1.c) from the authoritative XML in the waywallen repo.
#
# Usage:
#     scripts/regen.sh /path/to/waywallen
#
# The script runs `wayproto-gen` from the given waywallen checkout and
# patches the `#include` line in the generated source to match the
# public header path.

set -euo pipefail

if [ $# -lt 1 ]; then
    echo "usage: $0 <path-to-waywallen-repo>" >&2
    exit 1
fi

WAYWALLEN="$1"
HERE="$(cd "$(dirname "$0")/.." && pwd)"

XML="$WAYWALLEN/protocol/waywallen_ipc_v1.xml"
TOOL="$WAYWALLEN/tools/wayproto-gen"

if [ ! -f "$XML" ]; then
    echo "error: $XML not found" >&2
    exit 1
fi
if [ ! -d "$TOOL" ]; then
    echo "error: $TOOL not found" >&2
    exit 1
fi

OUT_H="$HERE/include/waywallen-bridge/ipc_v1.h"
OUT_C="$HERE/src/ipc_v1.c"

echo "regenerating from $XML ..."
(
    cd "$TOOL"
    cargo run --quiet -- \
        --in "$XML" \
        --out-c-header "$OUT_H" \
        --out-c-source "$OUT_C"
)

# Patch the generated #include to use the public header path.
sed -i 's|#include "ww_proto.h"|#include "waywallen-bridge/ipc_v1.h"|' "$OUT_C"

echo "done:"
echo "  $OUT_H"
echo "  $OUT_C"
