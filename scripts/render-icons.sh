#!/usr/bin/env bash
# Renders the launcher and avatar rasters from assets/brand/folio-icon.svg.
# Run it after changing the mark; its outputs are committed so builds need neither tool.
set -euo pipefail
cd "$(dirname "$0")/.."
for tool in inkscape magick; do
  if ! command -v "${tool}" >/dev/null 2>&1; then
    echo "${tool} is required" >&2
    exit 1
  fi
done
source_svg=assets/brand/folio-icon.svg
work="$(mktemp -d)"
trap 'rm -rf "${work}"' EXIT

render() {
  inkscape "${source_svg}" --export-type=png --export-width="$1" --export-height="$1" \
    --export-filename="$2" >/dev/null 2>&1
}

render 512 assets/brand/folio-icon-512.png
# Legacy launchers show the whole bitmap, so crop to the adaptive grid's 72-unit safe zone.
for pair in mdpi:48 hdpi:72 xhdpi:96 xxhdpi:144 xxxhdpi:192; do
  density="${pair%%:*}"
  size="${pair##*:}"
  render "$((size * 3 / 2))" "${work}/${density}.png"
  mkdir -p "assets/android/res/mipmap-${density}"
  magick "${work}/${density}.png" -gravity center -crop "${size}x${size}+0+0" +repage \
    -define webp:lossless=true "assets/android/res/mipmap-${density}/ic_launcher.webp"
done
echo "Rendered assets/brand/folio-icon-512.png and five launcher sizes."
