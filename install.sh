#!/bin/sh
# Install the oxide CLI.
#
#   curl -fsSL https://github.com/jaysonwu991/oxide/releases/latest/download/install.sh | bash
#
# Environment overrides:
#   OXIDE_VERSION      version to install, with or without a leading "v"
#                      (default: latest release)
#   OXIDE_INSTALL_DIR  directory to install the binary into
#                      (default: $HOME/.local/bin)
#   OXIDE_REPO         GitHub repository slug (default: jaysonwu991/oxide)
set -eu

REPO="${OXIDE_REPO:-jaysonwu991/oxide}"
VERSION="${OXIDE_VERSION:-}"
INSTALL_DIR="${OXIDE_INSTALL_DIR:-$HOME/.local/bin}"

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

detect_target() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Darwin) os_part="apple-darwin" ;;
        Linux) os_part="unknown-linux-gnu" ;;
        *) err "unsupported operating system: $os" ;;
    esac

    case "$arch" in
        arm64 | aarch64) arch_part="aarch64" ;;
        x86_64 | amd64) arch_part="x86_64" ;;
        *) err "unsupported architecture: $arch" ;;
    esac

    target="${arch_part}-${os_part}"
    case "$target" in
        aarch64-apple-darwin | x86_64-unknown-linux-gnu) ;;
        *) err "no prebuilt binary available for $target" ;;
    esac

    printf '%s' "$target"
}

resolve_version() {
    if [ -n "$VERSION" ]; then
        printf '%s' "${VERSION#v}"
        return
    fi

    tag="$(fetch "https://api.github.com/repos/${REPO}/releases/latest" \
        | grep -m1 '"tag_name"' \
        | sed -E 's/.*"tag_name"[[:space:]]*:[[:space:]]*"([^"]+)".*/\1/')"
    [ -n "$tag" ] || err "could not determine the latest release for ${REPO}"
    printf '%s' "${tag#v}"
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

    target="$(detect_target)"
    version="$(resolve_version)"
    archive="oxide-${target}.tar.gz"
    url="https://github.com/${REPO}/releases/download/v${version}/${archive}"

    tmp="$(mktemp -d)"
    trap 'rm -rf "$tmp"' EXIT INT TERM

    info "downloading oxide ${version} for ${target}"
    download "$url" "${tmp}/${archive}"

    if download "${url}.sha256" "${tmp}/${archive}.sha256" 2>/dev/null; then
        verify_checksum "${tmp}/${archive}" "${tmp}/${archive}.sha256"
    else
        warn "checksum file unavailable; skipping verification"
    fi

    tar -xzf "${tmp}/${archive}" -C "$tmp"

    mkdir -p "$INSTALL_DIR"
    mv -f "${tmp}/oxide" "${INSTALL_DIR}/oxide"
    chmod 0755 "${INSTALL_DIR}/oxide"

    info "installed oxide to ${INSTALL_DIR}/oxide"

    case ":${PATH:-}:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            warn "${INSTALL_DIR} is not on your PATH"
            printf 'oxide-install: add it with: export PATH="%s:$PATH"\n' "$INSTALL_DIR" >&2
            ;;
    esac
}

main "$@"
