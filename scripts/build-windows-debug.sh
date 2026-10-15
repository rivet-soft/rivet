#!/usr/bin/env bash
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

target=x86_64-pc-windows-gnu
for tool in x86_64-w64-mingw32-gcc x86_64-w64-mingw32-windres x86_64-w64-mingw32-strip zip; do
    if ! command -v "$tool" >/dev/null 2>&1; then
        echo "Missing $tool (install MinGW-w64 first)." >&2
        exit 1
    fi
done

rustup target add "$target"
export CARGO_TARGET_X86_64_PC_WINDOWS_GNU_LINKER=x86_64-w64-mingw32-gcc
cargo build --target "$target" --bin rivet

version="$(sed -n 's/^version = "\([^"]*\)"/\1/p' Cargo.toml | head -n 1)"
package="Rivet-${version}-windows-x64-debug"
output="$root/target/windows-release"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$output" "$stage/$package/examples"
cp "target/$target/debug/rivet.exe" "$stage/$package/Rivet.exe"
x86_64-w64-mingw32-strip --strip-debug "$stage/$package/Rivet.exe"
loader=(target/$target/debug/build/webview2-com-sys-*/out/x64/WebView2Loader.dll)
if [[ ! -f "${loader[0]}" ]]; then
    echo "WebView2Loader.dll was not produced by the build." >&2
    exit 1
fi
cp "${loader[0]}" "$stage/$package/WebView2Loader.dll"
cp examples/shop-api.yaml "$stage/$package/examples/shop-api.yaml"

rm -f "$output/$package.zip"
(cd "$stage" && zip -q -r "$output/$package.zip" "$package")
echo "Windows package: $output/$package.zip"
