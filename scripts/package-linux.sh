#!/usr/bin/env bash
# Builds Rivet for Linux x86_64 and packs it as .tar.gz with icon and .desktop file.
# Usage: RIVET_VERSION=v1.2.0 ./scripts/package-linux.sh
set -euo pipefail
cd "$(dirname "$0")/.."

version="${RIVET_VERSION:-}"
version="${version#v}"
[ -n "$version" ] || version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)-dev"
export RIVET_VERSION="$version"

cargo build --locked --release --bin rivet

name="Rivet-$version-linux-x64"
out="target/linux-release"
stage="$out/$name"
rm -rf "$out" && mkdir -p "$stage/examples"
cp target/release/rivet "$stage/rivet"
strip "$stage/rivet" || true
cp assets/app-icon.png "$stage/rivet.png"
cp packaging/rivet.desktop "$stage/rivet.desktop"
cp examples/shop-api.yaml "$stage/examples/"
cp README.md "$stage/"
tar -C "$out" -czf "$out/$name.tar.gz" "$name"
echo "Linux package: $out/$name.tar.gz"
