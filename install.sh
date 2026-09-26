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
        Darwin) os_part="darwin" ;;
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
        darwin-arm64 | darwin-x64 | linux-x64 | linux-arm64) ;;
        *) err "no prebuilt binary available for ${platform}" ;;
    esac

    printf '%s' "$platform"
}

manifest_asset() {
    body="$1"
    platform="$2"
    name="$(printf '%s\n' "$body" | awk -F': ' -v k="$platform" '$1 == k {sub(/^[[:space:]]*/, "", $2); sub(/[[:space:]]*$/, "", $2); print $2; exit}')"
    [ -n "$name" ] || err "no asset for ${platform}; supported targets: darwin-arm64, darwin-x64, linux-x64, linux-arm64"
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
        printf '%s/releases/download/v%s/Oxide-v%s-%s.tar.gz' "$BASE_URL" "$ver" "$ver" "$platform"
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

# Pre-branding releases published only the lowercase archive names, so an
# explicit OXIDE_VERSION (pin or rollback) can still need the old name.
legacy_url() {
    case "$1" in
        */Oxide-v*) printf '%s' "$1" | sed 's#/Oxide-v#/oxide-v#' ;;
        *) return 1 ;;
    esac
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
        if fallback="$(legacy_url "$url")"; then
            archive="${fallback##*/}"
            info "downloading ${archive} for ${platform}"
            download "$fallback" "${tmp}/${archive}" \
                || err "download failed for ${archive}"
            url="$fallback"
        else
            err "download failed for ${archive}"
        fi
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
