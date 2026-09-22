#!/usr/bin/env bash
# Sets the project version from a release tag.
#
# The repository keeps a placeholder version (`0.0.0`); release CI calls this
# with the pushed tag so the built CLI binary and desktop bundle report the tag
# version. Patches the workspace version in `Cargo.toml` and the desktop
# `tauri.conf.json` (which duplicates it), then it is up to the caller to
# refresh `Cargo.lock` (e.g. `cargo update --workspace`).
#
# Usage: scripts/set-version.sh v1.2.3   (a leading `v` is optional)

set -euo pipefail

raw="${1:-}"
if [ -z "$raw" ]; then
  echo "usage: $(basename "$0") <version|tag>" >&2
  exit 2
fi

version="${raw#v}"
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

"$py" - "$root" "$version" <<'PY'
import re
import sys

root, version = sys.argv[1], sys.argv[2]

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

# tauri.conf.json: the app version used for the bundle.
config_path = f"{root}/crates/desktop/tauri.conf.json"
with open(config_path, encoding="utf-8") as handle:
    config = handle.read()
config, count = re.subn(
    r'("version"\s*:\s*")[^"]*(")',
    lambda match: f"{match.group(1)}{version}{match.group(2)}",
    config,
    count=1,
)
if count != 1:
    sys.exit("error: could not find version in tauri.conf.json")
with open(config_path, "w", encoding="utf-8") as handle:
    handle.write(config)

print(f"set version to {version}")
PY
