#!/usr/bin/env bash
# Run after the native release executable and Debian package have been built.
set -euo pipefail
cd "$(dirname "$0")/.."
version=${1#v}
output="$PWD/target/release/bundle/appimage"
mkdir -p "$output"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
appdir="$work/TracePlotter.AppDir"
dpkg-deb --extract target/release/bundle/deb/*.deb "$appdir"
# The regular linuxdeploy path discovers native libraries without Tauri's GTK plugin.
curl --fail --location --retry 3 \
  https://github.com/tauri-apps/binary-releases/releases/download/linuxdeploy-07333c6/linuxdeploy-x86_64.AppImage \
  --output "$work/linuxdeploy.AppImage"
chmod +x "$work/linuxdeploy.AppImage"
OUTPUT="$output/TracePlotter_${version}_amd64.AppImage" ARCH=x86_64 APPIMAGE_EXTRACT_AND_RUN=1 \
  "$work/linuxdeploy.AppImage" --appimage-extract-and-run --appdir "$appdir" \
  --executable target/release/traceplotter \
  --desktop-file "$appdir/usr/share/applications/TracePlotter.desktop" \
  --icon-file "$appdir/usr/share/icons/hicolor/128x128/apps/traceplotter.png" \
  --output appimage
