#!/usr/bin/env bash
# Convenience launcher: ./run.sh <CLASS-URL> [extra uadl flags]
#
# Works both from a release tarball (uadl sits next to this script) and from a
# source checkout (builds target/release/uadl on first use). Reads the token
# from token.txt beside this script or from $UNACADEMY_TOKEN, and writes the
# MP4 in the same directory.
set -euo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

if [ $# -lt 1 ]; then
    echo "usage: ./run.sh <CLASS-URL-or-UID> [flags]"
    echo "  e.g. ./run.sh https://unacademy.com/class/slug/ABCD1234"
    echo "       ./run.sh ABCD1234 --fast"
    exit 2
fi

if [ -x "$HERE/uadl" ]; then
    BIN="$HERE/uadl"                       # release tarball
else
    BIN="$HERE/target/release/uadl"        # source checkout
    [ -x "$BIN" ] || cargo build --release --manifest-path "$HERE/Cargo.toml"
fi

TOKEN="${UNACADEMY_TOKEN:-}"
if [ -z "$TOKEN" ] && [ -f "$HERE/token.txt" ]; then
    TOKEN="$(tr -d '\r\n' < "$HERE/token.txt")"
fi
if [ -z "$TOKEN" ]; then
    echo "no token: put the accessToken cookie in token.txt, or export UNACADEMY_TOKEN"
    exit 2
fi

URL="$1"; shift
exec "$BIN" "$URL" "$TOKEN" -o "$HERE" "$@"
