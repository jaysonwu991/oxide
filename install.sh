#!/bin/sh
# Install the Oxide CLI.
#
#   curl -fsSL https://raw.githubusercontent.com/jaysonwu991/oxide/main/install.sh | bash
#
# Environment overrides:
#   OXIDE_VERSION      version to install, with or without a leading "v"
#                      (default: latest CLI release)
#   OXIDE_INSTALL_DIR  directory to install the binary into
#                      (default: $HOME/.local/bin)
#   OXIDE_REPO         GitHub repository slug (default: jaysonwu991/oxide)
set -eu

REPO="${OXIDE_REPO:-jaysonwu991/oxide}"
VERSION="${OXIDE_VERSION:-}"
INSTALL_DIR="${OXIDE_INSTALL_DIR:-$HOME/.local/bin}"
BASE_URL="https://github.com/${REPO}"
MANIFEST_NAME="Oxide-manifest"
LEGACY_MANIFEST_NAME="oxide-manifest"
# A file beside the binary naming the install, so `oxide update` recognizes a
# released install whatever OXIDE_INSTALL_DIR put it in.
MARKER_NAME=".oxide-install"

err() {
    printf 'oxide-install: error: %s\n' "$*" >&2
    exit 1
}

warn() {
    printf 'oxide-install: warning: %s\n' "$*" >&2
}

info() {
    printf 'oxide-install: %s\n' "$*" >&2
}

download() {
    if command -v curl >/dev/null 2>&1; then
        curl --fail --silent --show-error --location "$1" --output "$2"
    elif command -v wget >/dev/null 2>&1; then
        wget --quiet --output-document="$2" "$1"
    else
        err "curl or wget is required"
    fi
}

fetch() {
    if command -v curl >/dev/null 2>&1; then
        curl --fail --silent --show-error --location "$1"
    else
        wget --quiet --output-document=- "$1"
    fi
}

detect_platform() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Darwin) os_part="macos" ;;
        Linux) os_part="linux" ;;
        *) err "unsupported operating system: $os" ;;
    esac

    case "$arch" in
        arm64 | aarch64) arch_part="arm64" ;;
        x86_64 | amd64) arch_part="x64" ;;
        *) err "unsupported architecture: $arch" ;;
    esac

    platform="${os_part}-${arch_part}"
    case "$platform" in
        macos-arm64 | macos-x64 | linux-x64 | linux-arm64) ;;
        *) err "no prebuilt binary available for ${platform}" ;;
    esac

    printf '%s' "$platform"
}

# The token a release was named after before the version moved into the name,
# which is the key an older manifest carries and the name an older archive
# carries.
previous_platform() {
    case "$1" in
        macos-arm64) printf '%s' "darwin-arm64" ;;
        macos-x64) printf '%s' "darwin-x64" ;;
        linux-x64 | linux-arm64) printf '%s' "$1" ;;
        *) return 1 ;;
    esac
}

manifest_asset() {
    body="$1"
    platform="$2"
    name="$(printf '%s\n' "$body" | awk -F': ' -v k="$platform" '$1 == k {sub(/^[[:space:]]*/, "", $2); sub(/[[:space:]]*$/, "", $2); print $2; exit}')"
    if [ -z "$name" ]; then
        previous="$(previous_platform "$platform")" || previous=""
        if [ -n "$previous" ]; then
            name="$(printf '%s\n' "$body" | awk -F': ' -v k="$previous" '$1 == k {sub(/^[[:space:]]*/, "", $2); sub(/[[:space:]]*$/, "", $2); print $2; exit}')"
        fi
    fi
    [ -n "$name" ] || err "no asset for ${platform}; supported targets: macos-arm64, macos-x64, linux-x64, linux-arm64"
    printf '%s' "$name"
}

# The repository publishes the CLI (`v*`), the desktop app (`desktop-v*`), and
# the VS Code extension (`extension-v*`), so `/releases/latest` can point at a
# component that carries no CLI manifest. Prefer `latest` when it has one, and
# otherwise resolve the newest CLI release from the API.
latest_cli_tag() {
    releases="$(fetch "https://api.github.com/repos/${REPO}/releases?per_page=100" 2>/dev/null)" || return 1
    tag="$(printf '%s\n' "$releases" \
        | tr ',' '\n' \
        | sed -n 's/.*"tag_name":[[:space:]]*"\(v[0-9][^"]*\)".*/\1/p' \
        | head -n1)"
    [ -n "$tag" ] || return 1
    printf '%s' "$tag"
}

fetch_manifest() {
    fetch "${BASE_URL}/releases/latest/download/${MANIFEST_NAME}" 2>/dev/null && return 0
    fetch "${BASE_URL}/releases/latest/download/${LEGACY_MANIFEST_NAME}" 2>/dev/null && return 0
    tag="$(latest_cli_tag)" || return 1
    info "fetching ${MANIFEST_NAME} from ${tag}"
    fetch "${BASE_URL}/releases/download/${tag}/${MANIFEST_NAME}" 2>/dev/null \
        || fetch "${BASE_URL}/releases/download/${tag}/${LEGACY_MANIFEST_NAME}"
}

resolve_url() {
    platform="$1"

    if [ -n "$VERSION" ]; then
        ver="${VERSION#v}"
        printf '%s/releases/download/v%s/Oxide-%s-%s.tar.gz' "$BASE_URL" "$ver" "$ver" "$platform"
        return
    fi

    info "fetching ${MANIFEST_NAME}"
    manifest="$(fetch_manifest)" \
        || err "could not fetch ${MANIFEST_NAME}; has a release been published?"
    ver="$(printf '%s\n' "$manifest" | awk -F': ' '/^version:/ {print $2; exit}')"
    [ -n "$ver" ] || err "could not read version from ${MANIFEST_NAME}"
    asset="$(manifest_asset "$manifest" "$platform")"
    printf '%s/releases/download/%s/%s' "$BASE_URL" "$ver" "$asset"
}

# The names an archive can still be published under: the token a release was
# named after before the version moved into it, and the lowercase name the
# releases before the branding published. A pinned version — a rollback — can
# need either, and an older release only carries them.
legacy_urls() {
    platform="$1"
    ver="$2"
    ver="${ver#v}"
    previous="$(previous_platform "$platform")" || return 1
    printf '%s/releases/download/v%s/Oxide-v%s-%s.tar.gz\n' "$BASE_URL" "$ver" "$ver" "$previous"
    printf '%s/releases/download/v%s/oxide-v%s-%s.tar.gz\n' "$BASE_URL" "$ver" "$ver" "$previous"
}

verify_checksum() {
    file="$1"
    sums="$2"
    expected="$(cut -d' ' -f1 < "$sums" | head -n1)"
    [ -n "$expected" ] || return 0

    if command -v sha256sum >/dev/null 2>&1; then
        actual="$(sha256sum "$file" | cut -d' ' -f1)"
    elif command -v shasum >/dev/null 2>&1; then
        actual="$(shasum -a 256 "$file" | cut -d' ' -f1)"
    elif command -v openssl >/dev/null 2>&1; then
        actual="$(openssl dgst -sha256 "$file" | sed 's/^.*= //')"
    else
        warn "no sha256 tool found; skipping checksum verification"
        return 0
    fi

    [ "$expected" = "$actual" ] || err "checksum verification failed"
}

main() {
    command -v uname >/dev/null 2>&1 || err "uname is required"
    command -v tar >/dev/null 2>&1 || err "tar is required"
    command -v mktemp >/dev/null 2>&1 || err "mktemp is required"

    platform="$(detect_platform)"
    url="$(resolve_url "$platform")"
    archive="${url##*/}"

    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT INT TERM

    info "downloading ${archive} for ${platform}"
    if ! download "$url" "${tmp}/${archive}"; then
        fallback=""
        for candidate in $(legacy_urls "$platform" "$ver"); do
            name="${candidate##*/}"
            info "downloading ${name} for ${platform}"
            if download "$candidate" "${tmp}/${name}"; then
                fallback="$candidate"
                archive="$name"
                break
            fi
        done
        [ -n "$fallback" ] || err "download failed for ${archive}"
        url="$fallback"
    fi

    if download "${url}.sha256" "${tmp}/${archive}.sha256" 2>/dev/null; then
        verify_checksum "${tmp}/${archive}" "${tmp}/${archive}.sha256"
    else
        warn "checksum file unavailable; skipping verification"
    fi

    tar -xzf "${tmp}/${archive}" -C "$tmp"

    mkdir -p "$INSTALL_DIR"
    mv -f "${tmp}/oxide" "${INSTALL_DIR}/oxide"
    chmod 0755 "${INSTALL_DIR}/oxide"

    # The version is read back from the binary that was just installed, so the
    # marker names what is here rather than what was asked for.
    installed_version="$("${INSTALL_DIR}/oxide" --version 2>/dev/null | awk '{print $2}')" \
        || installed_version=""
    printf 'source %s\nversion %s\n' "$REPO" "$installed_version" \
        > "${INSTALL_DIR}/${MARKER_NAME}"

    info "installed Oxide to ${INSTALL_DIR}/oxide"

    case ":${PATH:-}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            warn "${INSTALL_DIR} is not on your PATH"
            printf 'oxide-install: add it with: export PATH="%s:$PATH"\n' "$INSTALL_DIR" >&2
            ;;
    esac
}

main "$@"
