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
# DMG: the app, a link to /Applications to drag it onto and an install README.
stage="$out/dmg"
rm -rf "$stage" && mkdir -p "$stage"
ditto "$app" "$stage/Rivet.app"
ln -s /Applications "$stage/Applications"
cp packaging/dmg-README.txt "$stage/README — установка.txt"

# hdiutil's automatic sizing for -srcfolder occasionally comes out too small
# ("No space left on device"); give it an explicit size with headroom and retry.
rw="$out/rw.dmg"
# A unique volume name while Finder lays the window out (an already mounted
# "Rivet" volume would be ambiguous); renamed to "Rivet" before packing.
build_vol="RivetBuild$$"
size_mb=$(( $(du -sm "$stage" | cut -f1) + 64 ))
for attempt in 1 2 3; do
  if hdiutil create -volname "$build_vol" -srcfolder "$stage" -size "${size_mb}m" \
       -fs HFS+ -ov -format UDRW "$rw" >/dev/null; then
    break
  fi
  [ "$attempt" = 3 ] && { echo "hdiutil failed" >&2; exit 1; }
  echo "hdiutil failed (attempt $attempt), retrying…" >&2
  size_mb=$(( size_mb * 2 ))
  sleep 5
done

# Finder window layout (icon view, app on the left, Applications on the right,
# README below). Cosmetic: when Finder scripting is unavailable or hangs the
# DMG keeps Finder's default layout.
attach="$(hdiutil attach "$rw" -readwrite -noverify -noautoopen)"
dev="$(echo "$attach" | head -1 | awk '{print $1}')"
mnt="$(echo "$attach" | sed -n 's|.*\(/Volumes/.*\)$|\1|p' | tail -1)"
[ -d "$mnt" ] || { echo "cannot mount $rw" >&2; exit 1; }
perl -e 'alarm 90; exec @ARGV' osascript - "$build_vol" <<'APPLESCRIPT' || echo "Finder layout skipped" >&2
on run argv
  tell application "Finder"
    set d to disk (item 1 of argv)
    open d
    set w to container window of d
    set current view of w to icon view
    set toolbar visible of w to false
    set statusbar visible of w to false
    set bounds of w to {200, 120, 760, 520}
    set opts to icon view options of w
    set arrangement of opts to not arranged
    set icon size of opts to 96
    set text size of opts to 12
    set position of item "Rivet.app" of d to {140, 150}
    set position of item "Applications" of d to {420, 150}
    set position of item "README — установка.txt" of d to {280, 300}
    update d without registering applications
    delay 2
    close w
  end tell
end run
APPLESCRIPT
sync
diskutil rename "$mnt" "Rivet" >/dev/null
hdiutil detach "$dev" -quiet || hdiutil detach "$dev" -force -quiet
hdiutil convert "$rw" -format UDZO -imagekey zlib-level=9 -ov -o "$out/$name.dmg" >/dev/null
rm -rf "$rw" "$stage"
echo "macOS packages: $out/$name.dmg, $out/$name.zip"
