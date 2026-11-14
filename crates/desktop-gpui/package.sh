#!/usr/bin/env bash
# Packages Oxide for Desktop (Beta) for the machine that runs it.
#
# The Electron app hands both of its halves to electron-builder. This front-end
# is one native binary, so this script is what turns it into the thing people
# install: a `.app` in a `.dmg` on macOS, and a `.tar.gz` bundle — binary, icon
# and desktop entry — on Linux and Windows. The names it writes are the ones
# `.github/workflows/desktop-gpui.yml` uploads and the ones a release carries;
# they are deliberately distinct from the stable app's so the two installers can
# sit side by side.
#
# Signing and notarization happen when the corresponding secrets are configured,
# exactly as they do for the stable app: an imported Developer ID identity is
# used to sign the bundle (hardened runtime, timestamped) and the Apple API key,
# when present, notarizes it and staples the ticket.
#
# Usage: package.sh [arm64|x64]
#
#   cd crates/desktop-gpui && ./package.sh
#   ./package.sh x64
#
# The artifacts land in `crates/desktop-gpui/artifacts/`.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$here"

requested_arch="${1:-}"
case "${requested_arch:-$(uname -m)}" in
  arm64 | aarch64) arch=arm64 ;;
  x64 | x86_64) arch=x64 ;;
  *)
    echo "error: unknown architecture '${requested_arch}' (expected arm64 or x64)" >&2
    exit 2
    ;;
esac

host_arch="$(uname -m)"
case "$host_arch" in
  arm64 | aarch64) host=arm64 ;;
  x86_64 | x64) host=x64 ;;
  *) host="$host_arch" ;;
esac
if [ "$arch" != "$host" ]; then
  echo "error: asked for $arch but this machine builds $host binaries" >&2
  exit 2
fi

version="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
if [ -z "$version" ]; then
  echo "error: could not read the version from Cargo.toml" >&2
  exit 1
fi

if [ "${OXIDE_SKIP_BUILD:-0}" != "1" ]; then
  echo "building oxide-desktop-gpui $version"
  cargo build --release
fi

artifact_dir="$here/artifacts"
rm -rf "$artifact_dir"
mkdir -p "$artifact_dir"

case "$(uname -s)" in
  Darwin) platform=macos ;;
  Linux) platform=linux ;;
  MINGW* | MSYS* | CYGWIN*) platform=win ;;
  *)
    echo "error: unsupported platform $(uname -s)" >&2
    exit 2
    ;;
esac

app_name="Oxide Beta"
bundle_id="dev.oxide.desktop.beta"
plugins="$here/../desktop/plugins"

if [ "$platform" = "macos" ]; then
  binary="target/release/oxide-desktop-gpui"
  if [ ! -x "$binary" ]; then
    echo "error: $binary was not built" >&2
    exit 1
  fi

  app="$artifact_dir/$app_name.app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cp "$binary" "$app/Contents/MacOS/oxide-desktop-gpui"
  cp "$here/../desktop/icons/icon.icns" "$app/Contents/Resources/icon.icns"
  if [ -d "$plugins" ]; then
    cp -R "$plugins" "$app/Contents/Resources/plugins"
  fi

  cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleDevelopmentRegion</key>
  <string>en</string>
  <key>CFBundleDisplayName</key>
  <string>$app_name</string>
  <key>CFBundleExecutable</key>
  <string>oxide-desktop-gpui</string>
  <key>CFBundleIconFile</key>
  <string>icon.icns</string>
  <key>CFBundleIdentifier</key>
  <string>$bundle_id</string>
  <key>CFBundleInfoDictionaryVersion</key>
  <string>6.0</string>
  <key>CFBundleName</key>
  <string>$app_name</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>$version</string>
  <key>CFBundleVersion</key>
  <string>$version</string>
  <key>LSApplicationCategoryType</key>
  <string>public.app-category.developer-tools</string>
  <key>LSMinimumSystemVersion</key>
  <string>13.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
</dict>
</plist>
PLIST

  identity="$((security find-identity -v -p codesigning 2>/dev/null || true) \
    | sed -n 's/.*"\(Developer ID Application: [^"]*\)"/\1/p' | head -1)"
  if [ -n "$identity" ]; then
    echo "signing with $identity"
    codesign --force --deep --options runtime --timestamp --sign "$identity" "$app"
  else
    echo "no Developer ID identity found; leaving the bundle unsigned"
  fi

  if [ -n "${APPLE_API_KEY:-}" ] && [ -n "${APPLE_API_KEY_ID:-}" ] && [ -n "${APPLE_API_ISSUER:-}" ]; then
    echo "notarizing"
    temp_work="$(mktemp -d)"
    ditto -c -k --keepParent "$app" "$temp_work/OxideBeta.zip"
    xcrun notarytool submit "$temp_work/OxideBeta.zip" \
      --key "$APPLE_API_KEY" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER" --wait
    xcrun stapler staple "$app"
    rm -rf "$temp_work"
  else
    echo "no notarization credentials; skipping notarization"
  fi

  dmg="$artifact_dir/Oxide-Beta-macos-$arch.dmg"
  rm -f "$dmg"
  hdiutil create -volname "$app_name" -srcfolder "$app" -ov -format UDZO "$dmg" >/dev/null
  echo "wrote $dmg"
  exit 0
fi

stage="$artifact_dir/stage"
mkdir -p "$stage"
if [ "$platform" = "win" ]; then
  binary="target/release/oxide-desktop-gpui.exe"
  package="$artifact_dir/Oxide-Beta-win-$arch-Setup.tar.gz"
else
  binary="target/release/oxide-desktop-gpui"
  package="$artifact_dir/Oxide-Beta-linux-$arch-Setup.tar.gz"
fi
if [ ! -f "$binary" ]; then
  echo "error: $binary was not built" >&2
  exit 1
fi
if [ "$platform" = "win" ]; then
  cp "$binary" "$stage/oxide-desktop-gpui.exe"
  cp "$here/../desktop/icons/icon.ico" "$stage/oxide-beta.ico"
else
  cp "$binary" "$stage/oxide-desktop-gpui"
  cp "$here/../desktop/icons/icon.png" "$stage/oxide-beta.png"
  cat > "$stage/oxide-beta.desktop" <<DESKTOP
[Desktop Entry]
Name=Oxide Beta
Comment=An AI coding agent (beta GPUI front-end)
Exec=oxide-desktop-gpui
Icon=oxide-beta
Terminal=false
Type=Application
Categories=Development;
DESKTOP
fi
if [ -d "$plugins" ]; then
  cp -R "$plugins" "$stage/plugins"
fi
cat > "$stage/README.txt" <<README
Oxide for Desktop (Beta) $version

This is the beta GPUI front-end. It reads the same configuration, sessions and
approvals as the terminal and the stable desktop app — run it beside them.

Unpack this folder somewhere it can stay and run oxide-desktop-gpui.
The folder's plugins/ directory is installed into the shared plugin store at
launch, exactly as the stable app's bundled plugins are.
README

tar -czf "$package" -C "$stage" .
echo "wrote $package"
