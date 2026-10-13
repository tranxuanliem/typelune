#!/usr/bin/env bash
# Rebuild assets/brand/Typelune.icns from the original SVG artwork (see docs/brand.md).
# Rendering uses AppKit via scripts/rasterize-svg.swift (transparent background), then
# iconutil. Each slot is rendered directly at its pixel size (no downscaling):
#   16, 32, 64 px  ← assets/brand/icon-macos-small.svg (hand-tuned)
#   128–1024 px    ← assets/brand/icon-macos.svg
# The result is checked: every slot must have transparent corners and an opaque centre.
# The generated .icns is committed, so normal builds and CI do not run this script.
set -euo pipefail
cd "$(dirname "$0")/.."
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
set_dir="$work/Typelune.iconset"
mkdir -p "$set_dir"
S=assets/brand/icon-macos-small.svg
L=assets/brand/icon-macos.svg
swift scripts/rasterize-svg.swift \
  "$S" "$set_dir/icon_16x16.png" 16 \
  "$S" "$set_dir/icon_16x16@2x.png" 32 \
  "$S" "$set_dir/icon_32x32.png" 32 \
  "$S" "$set_dir/icon_32x32@2x.png" 64 \
  "$L" "$set_dir/icon_128x128.png" 128 \
  "$L" "$set_dir/icon_128x128@2x.png" 256 \
  "$L" "$set_dir/icon_256x256.png" 256 \
  "$L" "$set_dir/icon_256x256@2x.png" 512 \
  "$L" "$set_dir/icon_512x512.png" 512 \
  "$L" "$set_dir/icon_512x512@2x.png" 1024 >/dev/null
iconutil -c icns "$set_dir" -o assets/brand/Typelune.icns
# Verify the packed file, not just the inputs.
iconutil -c iconset assets/brand/Typelune.icns -o "$work/verify.iconset"
swift scripts/rasterize-svg.swift --check-alpha "$work"/verify.iconset/*.png
echo "wrote assets/brand/Typelune.icns"
