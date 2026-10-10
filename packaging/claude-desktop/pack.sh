#!/usr/bin/env bash
# Build the Claude Desktop extension: dist/filmcraft-<version>.mcpb (a zip with manifest.json,
# icon.png and server/filmcraft-cli). Double-click it (or drag it into Claude Desktop ▸ Settings ▸
# Extensions) to install. Usage: packaging/claude-desktop/pack.sh [path/to/filmcraft-cli]
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
root="$(cd "$here/../.." && pwd)"
bin="${1:-}"
if [ -z "$bin" ]; then
  (cd "$root" && cargo build --release -p filmcraft-cli)
  bin="$root/target/release/filmcraft-cli"
  [ -f "$bin.exe" ] && bin="$bin.exe"
fi
[ -f "$bin" ] || { echo "error: $bin not found" >&2; exit 1; }
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -1)"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT
mkdir -p "$stage/server"
sed "s/\"version\": \"[^\"]*\"/\"version\": \"$version\"/" "$here/manifest.json" > "$stage/manifest.json"
cp "$root/assets/app-icon/hicolor/512x512/apps/ai.storyteller.filmcraft.png" "$stage/icon.png"
cp "$bin" "$stage/server/"
chmod +x "$stage/server/"filmcraft-cli*
mkdir -p "$root/dist"
out="$root/dist/filmcraft-$version.mcpb"
rm -f "$out"
(cd "$stage" && python3 -I -c 'import os, sys, zipfile
with zipfile.ZipFile(sys.argv[1], "w", zipfile.ZIP_DEFLATED) as z:
    for d, _, fs in os.walk("."):
        for f in fs:
            p = os.path.join(d, f)
            i = zipfile.ZipInfo.from_file(p, os.path.relpath(p, "."))
            with open(p, "rb") as fh:
                z.writestr(i, fh.read(), zipfile.ZIP_DEFLATED)' "$out")
echo "wrote $out"
