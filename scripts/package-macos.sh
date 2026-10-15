#!/usr/bin/env bash
# Builds a universal (arm64 + x86_64) Rivet.app and packs it as .dmg and .zip.
# Usage: RIVET_VERSION=v1.2.0 ./scripts/package-macos.sh
set -euo pipefail
cd "$(dirname "$0")/.."

version="${RIVET_VERSION:-}"
version="${version#v}"
[ -n "$version" ] || version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)-dev"
export RIVET_VERSION="$version"

for target in aarch64-apple-darwin x86_64-apple-darwin; do
  rustup target add "$target" >/dev/null
  cargo build --locked --release --bin rivet --target "$target"
done

out="target/macos-release"
app="$out/Rivet.app"
rm -rf "$out" && mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources/examples"
lipo -create \
  target/aarch64-apple-darwin/release/rivet \
  target/x86_64-apple-darwin/release/rivet \
  -output "$app/Contents/MacOS/Rivet"
sed "s/__VERSION__/$version/g" packaging/Info.plist > "$app/Contents/Info.plist"
cp assets/app-icon.icns "$app/Contents/Resources/app-icon.icns"
cp examples/shop-api.yaml "$app/Contents/Resources/examples/"
# Ad-hoc signature so Apple Silicon runs the binary (not notarized).
codesign --force --deep --sign - "$app"

name="Rivet-$version-macos-universal"
ditto -c -k --keepParent "$app" "$out/$name.zip"
# hdiutil's automatic sizing for -srcfolder occasionally comes out too small
# ("No space left on device"); give it an explicit size with headroom and retry.
size_mb=$(( $(du -sm "$app" | cut -f1) + 64 ))
for attempt in 1 2 3; do
  if hdiutil create -volname "Rivet" -srcfolder "$app" -size "${size_mb}m" \
       -fs HFS+ -ov -format UDZO "$out/$name.dmg" >/dev/null; then
    break
  fi
  [ "$attempt" = 3 ] && { echo "hdiutil failed" >&2; exit 1; }
  echo "hdiutil failed (attempt $attempt), retrying…" >&2
  size_mb=$(( size_mb * 2 ))
  sleep 5
done
echo "macOS packages: $out/$name.dmg, $out/$name.zip"
