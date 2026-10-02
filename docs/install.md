# Installation and updates

How to install the `oxide` CLI, the desktop app, and the VS Code extension, and
how to keep an installation current or remove it. For configuration after
install, see [configuration.md](configuration.md).

## Prebuilt binary

### macOS and Linux

```sh
curl -fsSL https://raw.githubusercontent.com/jaysonwu991/oxide/main/install.sh | bash
```

The installer detects your OS/arch, downloads the matching release, verifies its
SHA-256 checksum, and installs `oxide` to `~/.local/bin` by default.

Overrides:

| Variable | Purpose |
| --- | --- |
| `OXIDE_VERSION` | Version to install (with or without a leading `v`). Defaults to the latest release. |
| `OXIDE_INSTALL_DIR` | Install directory. Defaults to `$HOME/.local/bin`. |
| `OXIDE_REPO` | GitHub repo slug. Defaults to `jaysonwu991/oxide`. |

### Windows (x86_64)

```powershell
irm https://raw.githubusercontent.com/jaysonwu991/oxide/main/install.ps1 | iex
```

The PowerShell installer detects your OS/arch, downloads the matching release,
verifies its SHA-256 checksum, and installs `oxide.exe` to
`%LOCALAPPDATA%\Programs\Oxide` by default. The same script also runs on macOS
and Linux under PowerShell (`pwsh`), installing `oxide` to `~/.local/bin`.

Overrides:

| Variable | Purpose |
| --- | --- |
| `OXIDE_VERSION` | Version to install (with or without a leading `v`). Defaults to the latest release. |
| `OXIDE_INSTALL_DIR` | Install directory. Defaults to `%LOCALAPPDATA%\Programs\Oxide` on Windows, `$HOME/.local/bin` elsewhere. |
| `OXIDE_REPO` | GitHub repo slug. Defaults to `jaysonwu991/oxide`. |

## Supported platforms

| Platform | Rust target | Archive |
| --- | --- | --- |
| macOS (Apple Silicon) | `aarch64-apple-darwin` | `.tar.gz` |
| macOS (Intel) | `x86_64-apple-darwin` | `.tar.gz` |
| Linux (x86_64) | `x86_64-unknown-linux-gnu` | `.tar.gz` |
| Linux (ARM64) | `aarch64-unknown-linux-gnu` | `.tar.gz` |
| Windows (x86_64) | `x86_64-pc-windows-msvc` | `.zip` |

## From source

Requires a stable Rust toolchain (edition 2021). The workspace splits the shared
agent core (`crates/core`), the terminal CLI (`crates/cli`), and the desktop app
(`crates/desktop`); the VS Code extension in `editors/vscode` is a separate pnpm
package.

```sh
cargo install --path crates/cli
```

## Desktop app

Prebuilt bundles are drafted under `desktop-v*` releases on the
[releases page](https://github.com/jaysonwu991/oxide/releases). To build from
source:

```sh
cd crates/desktop
pnpm install
pnpm start        # development run
pnpm run make     # packaged bundle
```

See [desktop.md](desktop.md) for signing and packaging details.

## VS Code extension

Install **Oxide** (`jaysonwu991.oxide-vscode`) by downloading
`oxide-vscode-<version>.vsix` from an `extension-v*`
[release](https://github.com/jaysonwu991/oxide/releases) and running
**Extensions: Install from VSIX…** in VS Code. The extension drives the `oxide`
CLI, so install the CLI first. To build it from source:

```sh
cd editors/vscode
pnpm install
pnpm run compile   # tsc -p .
pnpm test          # compile, then node --test out/test/
pnpm run package   # vsce package -> oxide-vscode-<version>.vsix
```

## Updating in place

```sh
oxide update                    # install the newest CLI release
oxide update --check            # report the newest release without installing it
oxide update --version 0.26.0   # install (or roll back to) a specific version
oxide update --force            # reinstall even when already current
oxide update --check --json     # the same report for a front-end
```

`oxide update` reads the release manifest the installers use, so a release that
belongs to the desktop app or the VS Code extension is never installed as the
CLI, and verifies the archive's SHA-256 checksum (warning when a release carries
none). It replaces the running binary only after the unpacked one reports its
version, so a truncated download or a wrong-platform archive is refused instead
of taking the place of a working binary — as is an archive that reports a
version other than the release it was unpacked for, rather than replacing a
working binary and then claiming the requested tag. `--version` takes a tag or a
bare version, with or without a leading `v`, and resolves without asking GitHub
anything. A Homebrew install is left to `brew upgrade oxide`, and a binary that
is not at a released location — a `target/debug` build — needs `--force` before
it is replaced. Both installers leave a `.oxide-install` file beside the binary
they unpack, and `oxide update` writes the same marker when it replaces one, so
a custom `OXIDE_INSTALL_DIR` is recognized as a released install rather than
needing `--force`. Restart `oxide` to run the new version.

`--check --json` prints that report as JSON — `current`, `latest`, `tag`,
`pinned`, `updateAvailable`, `installation`, `installable`, `path`, `advice`
and `releaseUrl` — which is what the desktop app's **Check for Updates…** command
and the VS Code extension's **Oxide: Check for Updates...** read before offering
to run `oxide update` for the release it named.

## Uninstalling

```sh
oxide uninstall --dry-run                 # preview removals
oxide uninstall                           # preview, confirm, and uninstall
oxide uninstall --keep-config --keep-data # retain configuration and user data
oxide uninstall --force                   # skip confirmation
```

Package-manager installations are removed through Cargo or Homebrew when
detected. Prebuilt installations print the final command needed to remove the
currently running executable after cleanup completes.
