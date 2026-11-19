#!/usr/bin/env bash
# Sets the project version from a release tag.
#
# The repository keeps a placeholder version (`0.0.0`); release CI calls this
# with the pushed tag so the built CLI binary and desktop bundle report the tag
# version. Patches the workspace version in the root `Cargo.toml` and the
# component it belongs to: the Tauri package/config, the engine's `Cargo.toml`,
# or the Electron window's `package.json`. Then it is up to the caller to
# refresh the lockfiles.
#
# A tag may carry a component prefix so each component releases independently:
# `v1.2.3` / `cli-v1.2.3` for the CLI, `desktop-v1.2.3` for the desktop app,
# `desktop-tauri-v1.2.3` for the primary Tauri desktop app, and
# `extension-v1.2.3` for the VS Code extension (which is versioned in its
# own `editors/vscode/package.json`). A bare `1.2.3` is also accepted.
#
# Usage: scripts/set-version.sh v1.2.3
#        scripts/set-version.sh cli-v1.2.3
#        scripts/set-version.sh desktop-tauri-v1.2.3
#        scripts/set-version.sh desktop-v1.2.3
#        scripts/set-version.sh extension-v1.2.3

set -euo pipefail

raw="${1:-}"
if [ -z "$raw" ]; then
  echo "usage: $(basename "$0") <version|tag>" >&2
  exit 2
fi

version="${raw}"
component=""
case "$version" in
  cli-*) version="${version#cli-}" ;;
  desktop-tauri-*)
    component="tauri"
    version="${version#desktop-tauri-}"
    ;;
  desktop-*) version="${version#desktop-}" ;;
  extension-*)
    component="vscode"
    version="${version#extension-}"
    ;;
esac
version="${version#v}"
if ! printf '%s' "$version" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$'; then
  echo "error: '$raw' is not a semver version (expected like v1.2.3)" >&2
  exit 2
fi

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

if command -v python3 >/dev/null 2>&1; then
  py=python3
elif command -v python >/dev/null 2>&1; then
  py=python
else
  echo "error: python is required to patch versions" >&2
  exit 1
fi

"$py" - "$root" "$version" "$component" <<'PY'
import re
import sys

root, version, component = sys.argv[1], sys.argv[2], sys.argv[3]

# editors/vscode/package.json: the VS Code extension keeps its own version,
# separate from the `0.0.0` Cargo placeholder, and `vsce package` names the
# VSIX from it.
if component == "vscode":
    manifest = f"{root}/editors/vscode/package.json"
    with open(manifest, encoding="utf-8") as handle:
        package, count = re.subn(
            r'("version"\s*:\s*")[^"]*(")',
            lambda match: f"{match.group(1)}{version}{match.group(2)}",
            handle.read(),
            count=1,
        )
    if count != 1:
        sys.exit("error: could not find version in editors/vscode/package.json")
    with open(manifest, "w", encoding="utf-8") as handle:
        handle.write(package)
    print(f"set extension version to {version}")
    sys.exit(0)

# Cargo.toml: the `version` inside `[workspace.package]`.
cargo_path = f"{root}/Cargo.toml"
with open(cargo_path, encoding="utf-8") as handle:
    lines = handle.readlines()

in_section = False
patched_cargo = False
for index, line in enumerate(lines):
    stripped = line.strip()
    if stripped.startswith("[") and stripped.endswith("]"):
        in_section = stripped == "[workspace.package]"
        continue
    if in_section and re.match(r"^version\s*=", stripped):
        indent = line[: len(line) - len(line.lstrip())]
        lines[index] = f'{indent}version = "{version}"\n'
        patched_cargo = True
        break
if not patched_cargo:
    sys.exit("error: could not find version in [workspace.package] of Cargo.toml")
with open(cargo_path, "w", encoding="utf-8") as handle:
    handle.writelines(lines)

# crates/desktop-tauri/Cargo.toml: the Tauri desktop app is its own workspace as
# well, and reports the version in its own manifest.
if component == "tauri":
    tauri_manifest = f"{root}/crates/desktop-tauri/Cargo.toml"
    tauri_config = f"{root}/crates/desktop-tauri/tauri.conf.json"
    with open(tauri_manifest, encoding="utf-8") as handle:
        lines = handle.readlines()
    in_section = False
    patched = False
    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped.startswith("[") and stripped.endswith("]"):
            in_section = stripped == "[package]"
            continue
        if in_section and re.match(r"^version\s*=", stripped):
            indent = line[: len(line) - len(line.lstrip())]
            lines[index] = f'{indent}version = "{version}"\n'
            patched = True
            break
    if not patched:
        sys.exit("error: could not find version in [package] of crates/desktop-tauri/Cargo.toml")
    with open(tauri_manifest, "w", encoding="utf-8") as handle:
        handle.writelines(lines)
    with open(tauri_config, encoding="utf-8") as handle:
        config = handle.read()
    config = re.sub(r'(?m)^(\s*"version"\s*:\s*)"[^"]+"', rf'\g<1>"{version}"', config, count=1)
    with open(tauri_config, "w", encoding="utf-8") as handle:
        handle.write(config)
    print(f"set Tauri desktop version to {version}")
    sys.exit(0)

# crates/desktop/Cargo.toml: the desktop package is excluded from the root
# workspace, so it reports the version in its own manifest — the one the app
# compares a release against at runtime.
desktop_manifest = f"{root}/crates/desktop/Cargo.toml"
with open(desktop_manifest, encoding="utf-8") as handle:
    lines = handle.readlines()

in_section = False
patched_desktop = False
for index, line in enumerate(lines):
    stripped = line.strip()
    if stripped.startswith("[") and stripped.endswith("]"):
        in_section = stripped == "[package]"
        continue
    if in_section and re.match(r"^version\s*=", stripped):
        indent = line[: len(line) - len(line.lstrip())]
        lines[index] = f'{indent}version = "{version}"\n'
        patched_desktop = True
        break
if not patched_desktop:
    sys.exit("error: could not find version in [package] of crates/desktop/Cargo.toml")
with open(desktop_manifest, "w", encoding="utf-8") as handle:
    handle.writelines(lines)

# crates/desktop/package.json: the Electron window's own version, which the
# bundler writes into the app bundle and `app.getVersion()` reports — the number
# a release is compared against. The engine's `Cargo.toml` above is the same
# number for the harness, so the two halves of one app agree.
package_path = f"{root}/crates/desktop/package.json"
with open(package_path, encoding="utf-8") as handle:
    package = handle.read()
package, count = re.subn(
    r'("version"\s*:\s*")[^"]*(")',
    lambda match: f"{match.group(1)}{version}{match.group(2)}",
    package,
    count=1,
)
if count != 1:
    sys.exit("error: could not find version in crates/desktop/package.json")
with open(package_path, "w", encoding="utf-8") as handle:
    handle.write(package)

print(f"set version to {version}")
PY
