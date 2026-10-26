#!/usr/bin/env bash
# Packages the GPUI app for macOS.
#
# The app is one native binary, so there is no installer tool between it and the
# reader: this assembles the `.app` bundle around the release build and archives
# it as `Oxide-GPUI-<version>-macos-<arch>.app.tar.gz`, the name
# `oxide_core::updates::Component::Gpui::assets()` resolves for this platform and
# the archive the app's own updater unpacks over the bundle it runs from.
#
# Usage: crates/desktop-gpui/scripts/package-macos.sh [--arch arm64|x64] [--version X]
#
# The version defaults to the one in the crate's `Cargo.toml`, which is what
# `scripts/set-version.sh` writes from a `gpui-v*` tag — the same number the app
# reports at runtime and compares a release against.

set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
crate="$root/crates/desktop-gpui"

arch=""
version=""

while [ $# -gt 0 ]; do
  case "$1" in
    --arch)
      arch="${2:-}"
      shift 2
      ;;
    --arch=*)
      arch="${1#--arch=}"
      shift
      ;;
    --version)
      version="${2:-}"
      shift 2
      ;;
    --version=*)
      version="${1#--version=}"
      shift
      ;;
    -h|--help)
      sed -n '2,18p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
      exit 0
      ;;
    *)
      echo "error: unknown argument '$1'" >&2
      exit 2
      ;;
  esac
done

if [ "$(uname -s)" != "Darwin" ]; then
  echo "error: the GPUI app is packaged for macOS only" >&2
  exit 1
fi

if [ -z "$arch" ]; then
  case "$(uname -m)" in
    arm64) arch="arm64" ;;
    x86_64) arch="x64" ;;
    *)
      echo "error: unknown machine '$(uname -m)'" >&2
      exit 1
      ;;
  esac
fi

case "$arch" in
  arm64) target="aarch64-apple-darwin" ;;
  x64) target="x86_64-apple-darwin" ;;
  *)
    echo "error: --arch must be arm64 or x64, not '$arch'" >&2
    exit 2
    ;;
esac

if [ -z "$version" ]; then
  version="$(sed -n 's/^version = "\(.*\)"$/\1/p' "$crate/Cargo.toml" | head -1)"
fi
if [ -z "$version" ]; then
  echo "error: could not read the version from $crate/Cargo.toml" >&2
  exit 1
fi

echo "packaging Oxide GPUI $version for macos-$arch"
cargo build --release --manifest-path "$crate/Cargo.toml" --target "$target"

binary="$crate/target/$target/release/oxide-desktop-gpui"
if [ ! -f "$binary" ]; then
  echo "error: $binary was not built" >&2
  exit 1
fi

artifacts="$crate/artifacts"
app="$artifacts/Oxide GPUI.app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp "$binary" "$app/Contents/MacOS/oxide-desktop-gpui"

icon="$root/crates/desktop/icons/icon.icns"
if [ -f "$icon" ]; then
  cp "$icon" "$app/Contents/Resources/icon.icns"
fi

cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key>
  <string>Oxide GPUI</string>
  <key>CFBundleDisplayName</key>
  <string>Oxide GPUI</string>
  <key>CFBundleIdentifier</key>
  <string>dev.oxide.gpui</string>
  <key>CFBundleExecutable</key>
  <string>oxide-desktop-gpui</string>
  <key>CFBundlePackageType</key>
  <string>APPL</string>
  <key>CFBundleShortVersionString</key>
  <string>${version}</string>
  <key>CFBundleVersion</key>
  <string>${version}</string>
  <key>CFBundleIconFile</key>
  <string>icon</string>
  <key>LSMinimumSystemVersion</key>
  <string>11.0</string>
  <key>NSHighResolutionCapable</key>
  <true/>
  <key>NSPrincipalClass</key>
  <string>NSApplication</string>
</dict>
</plist>
PLIST

# A Developer ID identity signs the bundle when one is configured; without it the
# bundle is signed ad hoc, which is what lets it run on the machine that built it.
if [ -n "${CODESIGN_IDENTITY:-}" ]; then
  codesign --force --deep --options runtime --timestamp \
    --sign "${CODESIGN_IDENTITY}" "$app"
else
  codesign --force --deep --sign - "$app"
fi

archive="$artifacts/Oxide-GPUI-${version}-macos-${arch}.app.tar.gz"
rm -f "$archive"
# `tar` rather than `ditto` because the app's updater unpacks it with `tar`.
tar -czf "$archive" -C "$artifacts" "Oxide GPUI.app"
shasum -a 256 "$archive" | tee "$archive.sha256"
echo "packaged $archive"
