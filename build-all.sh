#!/usr/bin/env bash
# Build uadl for every Linux architecture and package release tarballs into
# dist/. Same target list and layout as .github/workflows/release.yml, so you
# can reproduce a release locally.
#
#   ./build-all.sh              all targets
#   ./build-all.sh aarch64      only targets matching "aarch64"
#
# Needs `cross` and a running Docker daemon:
#   cargo install cross --locked
set -uo pipefail
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$HERE"

# Tier 1: expected to build. Best-effort ones are marked with a trailing "?"
# and are allowed to fail without failing the run.
TARGETS=(
    x86_64-unknown-linux-gnu
    x86_64-unknown-linux-musl
    aarch64-unknown-linux-gnu
    aarch64-unknown-linux-musl
    armv7-unknown-linux-gnueabihf
    armv7-unknown-linux-musleabihf
    arm-unknown-linux-gnueabihf
    i686-unknown-linux-gnu
    i686-unknown-linux-musl
    riscv64gc-unknown-linux-gnu?
    powerpc64le-unknown-linux-gnu?
    s390x-unknown-linux-gnu?
    loongarch64-unknown-linux-gnu?
)

command -v cross >/dev/null || {
    echo "cross not found: cargo install cross --locked"
    exit 1
}
docker info >/dev/null 2>&1 || {
    echo "docker is not running; cross needs it to hold the target toolchains"
    exit 1
}

VERSION="$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)"
FILTER="${1:-}"
rm -rf dist && mkdir -p dist

built=() failed=()
for entry in "${TARGETS[@]}"; do
    target="${entry%\?}"
    optional=""; [ "$entry" != "$target" ] && optional=1
    [ -n "$FILTER" ] && [[ "$target" != *"$FILTER"* ]] && continue

    echo
    echo "=== $target ==="
    if ! cross build --release --locked --target "$target"; then
        if [ -n "$optional" ]; then
            echo "  best-effort target failed, skipping"
        else
            echo "  FAILED"
        fi
        failed+=("$target")
        continue
    fi

    name="uadl-${VERSION}-${target}"
    mkdir -p "dist/$name"
    cp "target/$target/release/uadl" "dist/$name/"
    cp README.md run.sh "dist/$name/"
    tar -czf "dist/$name.tar.gz" -C dist "$name"
    rm -rf "dist/${name:?}"
    built+=("$target")
    echo "  -> dist/$name.tar.gz"
done

( cd dist && sha256sum ./*.tar.gz > SHA256SUMS 2>/dev/null )

echo
echo "built ${#built[@]}:  ${built[*]:-none}"
[ ${#failed[@]} -gt 0 ] && echo "failed ${#failed[@]}: ${failed[*]}"
ls -la dist
