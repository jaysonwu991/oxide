#!/usr/bin/env bash
# Packages Oxide for Desktop for the machine that runs it.
#
# The Electron app hands both of its halves to electron-builder. This front-end
# is one native binary, so this script is what turns it into the thing people
# install: a `.app` in a `.dmg` on macOS, and a `.tar.gz` bundle — binary, icon
# and desktop entry — on Linux and Windows, plus an `.AppImage` on Linux. The names it writes are the ones
# `.github/workflows/desktop-tauri.yml` uploads and the ones the shared updater
# resolves. Electron remains available from its own versioned
# releases.
#
# Signing and notarization happen when the corresponding secrets are configured,
# exactly as they do for the Electron app: an imported Developer ID identity is
# used to sign the bundle (hardened runtime, timestamped) and the Apple API key,
# when present, notarizes it and staples the ticket.
#
# Usage: package.sh [arm64|x64]
#
#   cd crates/desktop-tauri && ./package.sh
#   ./package.sh x64
#
# The artifacts land in `crates/desktop-tauri/artifacts/`.

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

node "$here/../../scripts/prepare-diagrams.mjs" desktop

if [ "${OXIDE_SKIP_BUILD:-0}" != "1" ]; then
  echo "building oxide-desktop-tauri $version"
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

app_name="Oxide"
bundle_id="dev.oxide.desktop"
plugins="$here/../desktop/plugins"

if [ "$platform" = "macos" ]; then
  binary="target/release/oxide-desktop-tauri"
  if [ ! -x "$binary" ]; then
    echo "error: $binary was not built" >&2
    exit 1
  fi

  app="$artifact_dir/$app_name.app"
  mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
  cp "$binary" "$app/Contents/MacOS/oxide-desktop-tauri"
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
  <string>oxide-desktop-tauri</string>
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
  <!-- The folders macOS keeps behind a per-app grant. A copy of an image from
       one of them reaches this app as a file URL, and the grant is the app's
       to hold: without a purpose string the system has nothing to show, and a
       paste of a file in these folders answers nothing at all. -->
  <key>NSDesktopFolderUsageDescription</key>
  <string>Oxide attaches the images, PDFs and files you copy from your Desktop.</string>
  <key>NSDocumentsFolderUsageDescription</key>
  <string>Oxide attaches the images, PDFs and files you copy from your Documents folder.</string>
  <key>NSDownloadsFolderUsageDescription</key>
  <string>Oxide attaches the images, PDFs and files you copy from your Downloads folder.</string>
</dict>
</plist>
PLIST

  identity="$((security find-identity -v -p codesigning 2>/dev/null || true) \
    | sed -n 's/.*"\(Developer ID Application: [^"]*\)"/\1/p' | head -1)"
  if [ -n "$identity" ]; then
    echo "signing with $identity"
    codesign --force --deep --options runtime --timestamp --sign "$identity" "$app"
  else
    # A bundle whose binary carries only the linker's signature fails
    # `codesign --verify` as a bundle ("code has no resources but signature
    # indicates they must be present"), and a copy of it that keeps the
    # download's quarantine attribute is killed on launch. Signing ad-hoc at
    # least makes the bundle valid; Gatekeeper still asks before opening a
    # build it cannot identify, which is the right-click the README mentions.
    echo "no Developer ID identity found; signing ad-hoc"
    codesign --force --deep --sign - "$app"
  fi

  if [ -n "${APPLE_API_KEY:-}" ] && [ -n "${APPLE_API_KEY_ID:-}" ] && [ -n "${APPLE_API_ISSUER:-}" ]; then
    echo "notarizing"
    temp_work="$(mktemp -d)"
    ditto -c -k --keepParent "$app" "$temp_work/Oxide.zip"
    xcrun notarytool submit "$temp_work/Oxide.zip" \
      --key "$APPLE_API_KEY" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER" --wait
    xcrun stapler staple "$app"
    rm -rf "$temp_work"
  else
    echo "no notarization credentials; skipping notarization"
  fi

  # Finder saves the icon positions and background in the image's own
  # .DS_Store, so the image is staged read-write and only compressed after the
  # layout has been written. The trap keeps a failed Finder step from leaving a
  # mounted volume behind on a release runner.
  # This is electron-builder's default DMG background, retained for visual
  # continuity with the Electron app.
  background="$here/assets/dmg-background.tiff"
  if [ ! -f "$background" ]; then
    echo "error: $background is missing" >&2
    exit 1
  fi
  dmg_work="$(mktemp -d "$artifact_dir/dmg-work.XXXXXX")"
  stage_dmg="$dmg_work/stage"
  rw_dmg="$dmg_work/Oxide-rw.dmg"
  mount_dir="$dmg_work/mount"
  volume_name="$app_name $version-$arch"
  attached=0
  cleanup_dmg() {
    if [ "$attached" = "1" ]; then
      hdiutil detach "$mount_dir" -force >/dev/null 2>&1 || true
    fi
    rm -rf "$dmg_work"
  }
  trap cleanup_dmg EXIT

  mkdir -p "$stage_dmg/.background"
  cp -R "$app" "$stage_dmg/"
  ln -s /Applications "$stage_dmg/Applications"
  cp "$background" "$stage_dmg/.background/dmg-background.tiff"

  hdiutil create -volname "$volume_name" -srcfolder "$stage_dmg" \
    -ov -format UDRW -fs HFS+ "$rw_dmg" >/dev/null
  mkdir -p "$mount_dir"
  hdiutil attach "$rw_dmg" -mountpoint "$mount_dir" \
    -readwrite -noverify -noautoopen >/dev/null
  attached=1

  osascript - "$mount_dir" "$app_name" <<'APPLESCRIPT'
on run argv
  set mountPath to item 1 of argv
  set appName to item 2 of argv
  set imageFolder to POSIX file mountPath as alias
  set backgroundFile to POSIX file (mountPath & "/.background/dmg-background.tiff") as alias
  tell application "Finder"
    open imageFolder
    set imageWindow to container window of imageFolder
    set current view of imageWindow to icon view
    set toolbar visible of imageWindow to false
    set statusbar visible of imageWindow to false
    set pathbar visible of imageWindow to false
    set sidebar width of imageWindow to 0
    set the bounds of imageWindow to {100, 100, 640, 480}
    set viewOptions to the icon view options of imageWindow
    set arrangement of viewOptions to not arranged
    set icon size of viewOptions to 80
    set text size of viewOptions to 12
    set background picture of viewOptions to backgroundFile
    set position of item (appName & ".app") of imageWindow to {130, 220}
    set position of item "Applications" of imageWindow to {410, 220}
    close imageWindow
    open imageFolder
    update imageFolder without registering applications
    delay 1
    close container window of imageFolder
  end tell
end run
APPLESCRIPT

  cp "$here/../desktop/icons/icon.icns" "$mount_dir/.VolumeIcon.icns"
  if command -v SetFile >/dev/null 2>&1; then
    SetFile -a C "$mount_dir"
  fi
  sync
  hdiutil detach "$mount_dir" >/dev/null
  attached=0

  dmg="$artifact_dir/Oxide-macos-$arch.dmg"
  rm -f "$dmg"
  hdiutil convert "$rw_dmg" -ov -format UDZO \
    -imagekey zlib-level=9 -o "$dmg" >/dev/null
  cleanup_dmg
  trap - EXIT
  echo "wrote $dmg"
  if [ -z "$identity" ]; then
    echo "note: this build is not notarized, so a downloaded copy needs one"
    echo "      right-click → Open (or: xattr -dr com.apple.quarantine \"$app\")"
  fi
  exit 0
fi

stage="$artifact_dir/stage"
mkdir -p "$stage"
if [ "$platform" = "win" ]; then
  binary="target/release/oxide-desktop-tauri.exe"
  package="$artifact_dir/Oxide-win-$arch-Setup.tar.gz"
else
  binary="target/release/oxide-desktop-tauri"
  package="$artifact_dir/Oxide-linux-$arch-Setup.tar.gz"
fi
if [ ! -f "$binary" ]; then
  echo "error: $binary was not built" >&2
  exit 1
fi
if [ "$platform" = "win" ]; then
  cp "$binary" "$stage/oxide-desktop-tauri.exe"
  cp "$here/../desktop/icons/icon.ico" "$stage/oxide.ico"
else
  cp "$binary" "$stage/oxide-desktop-tauri"
  cp "$here/../desktop/icons/icon.png" "$stage/oxide.png"
  cat > "$stage/oxide.desktop" <<DESKTOP
[Desktop Entry]
Name=Oxide
Comment=An AI coding agent
Exec=oxide-desktop-tauri
Icon=oxide
Terminal=false
Type=Application
Categories=Development;
DESKTOP
fi
if [ -d "$plugins" ]; then
  cp -R "$plugins" "$stage/plugins"
fi
cat > "$stage/README.txt" <<README
Oxide for Desktop $version

This is the native Tauri desktop app. It reads the same configuration, sessions
and approvals as the terminal and editor integrations.

Unpack this folder somewhere it can stay and run oxide-desktop-tauri.
The folder's plugins/ directory is installed into the shared plugin store at
launch, exactly as Electron's bundled plugins are.
README

tar -czf "$package" -C "$stage" .
echo "wrote $package"

if [ "$platform" = "linux" ]; then
  case "$arch" in
    x64) appimage_arch=x86_64 ;;
    *) appimage_arch=arm64 ;;
  esac
  case "$arch" in
    x64) tool_arch=x86_64 ;;
    *) tool_arch=aarch64 ;;
  esac
  appdir="$artifact_dir/Oxide.AppDir"
  rm -rf "$appdir"
  mkdir -p "$appdir/usr/bin"
  cp "$binary" "$appdir/usr/bin/oxide-desktop-tauri"
  if [ -d "$plugins" ]; then
    cp -R "$plugins" "$appdir/usr/bin/plugins"
  fi
  cp "$here/../desktop/icons/icon.png" "$appdir/oxide.png"
  cp "$stage/oxide.desktop" "$appdir/oxide.desktop"
  cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/bin/oxide-desktop-tauri" "$@"
APPRUN
  chmod +x "$appdir/AppRun"

  tool="${APPIMAGETOOL:-}"
  if [ -z "$tool" ]; then
    tool="$artifact_dir/appimagetool"
    curl -fsSL -o "$tool" \
      "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-$tool_arch.AppImage"
    chmod +x "$tool"
  fi
  appimage="$artifact_dir/Oxide-linux-$appimage_arch-Setup.AppImage"
  ARCH="$tool_arch" APPIMAGE_EXTRACT_AND_RUN=1 "$tool" --no-appstream "$appdir" "$appimage"
  rm -rf "$appdir"
  if [ -z "${APPIMAGETOOL:-}" ]; then
    rm -f "$tool"
  fi
  echo "wrote $appimage"
fi
