#!/bin/sh
# shellcheck disable=SC2059
# install.sh — Cross-platform installer for lazyide
# Usage: curl -fsSL https://tysonlabs.dev/lazyide/install.sh | sh
#        sh install.sh [--prefix <dir>] [--version <tag>] [--with-deps] [--no-deps] [--dry-run] [--no-prompt]
set -eu

REPO="TysonLabs/lazyide"
GITHUB_API="https://api.github.com"
GITHUB_DL="https://github.com"

# --- State ---
INSTALL_DIR="${LAZYIDE_INSTALL_DIR:-}"
EXPLICIT_PREFIX=false
VERSION=""
WITH_DEPS=""
DRY_RUN=false
NO_PROMPT=false
PATH_MODIFIED=false
NEED_SUDO=false

# --- Colors (respect NO_COLOR) ---
if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
    BOLD=$(printf '\033[1m')
    GREEN=$(printf '\033[32m')
    YELLOW=$(printf '\033[33m')
    RED=$(printf '\033[31m')
    CYAN=$(printf '\033[36m')
    RESET=$(printf '\033[0m')
else
    BOLD='' GREEN='' YELLOW='' RED='' CYAN='' RESET=''
fi

# --- Helpers ---
info()  { printf "${GREEN}info${RESET}  %s\n" "$*"; }
warn()  { printf "${YELLOW}warn${RESET}  %s\n" "$*" >&2; }
err()   { printf "${RED}error${RESET} %s\n" "$*" >&2; exit 1; }

has_cmd() { command -v "$1" >/dev/null 2>&1; }

maybe_sudo() {
    if [ "$NEED_SUDO" = true ] || { [ ! -w "$INSTALL_DIR" ] && [ "$(id -u)" != "0" ]; }; then
        sudo "$@"
    else
        "$@"
    fi
}

download() {
    url="$1"
    dest="$2"
    if has_cmd curl; then
        curl -fsSL -o "$dest" "$url"
    elif has_cmd wget; then
        wget -qO "$dest" "$url"
    else
        err "Neither curl nor wget found. Install one and retry."
    fi
}

download_stdout() {
    url="$1"
    if has_cmd curl; then
        curl -fsSL "$url"
    elif has_cmd wget; then
        wget -qO- "$url"
    else
        err "Neither curl nor wget found. Install one and retry."
    fi
}

prompt_yn() {
    question="$1"
    default="${2:-n}"
    if [ "$NO_PROMPT" = true ]; then
        [ "$default" = "y" ] && return 0 || return 1
    fi
    if [ "$default" = "y" ]; then
        printf "%s [Y/n] " "$question"
    else
        printf "%s [y/N] " "$question"
    fi
    read -r answer </dev/tty || answer=""
    case "$answer" in
        [Yy]*) return 0 ;;
        [Nn]*) return 1 ;;
        "")    [ "$default" = "y" ] && return 0 || return 1 ;;
        *)     return 1 ;;
    esac
}

# --- Parse flags ---
while [ $# -gt 0 ]; do
    case "$1" in
        --prefix)
            [ $# -ge 2 ] || err "--prefix requires a path argument"
            INSTALL_DIR="$2"; EXPLICIT_PREFIX=true; shift 2 ;;
        --version)
            [ $# -ge 2 ] || err "--version requires a version argument"
            VERSION="$2"; shift 2 ;;
        --with-deps)
            WITH_DEPS="yes"; shift ;;
        --no-deps)
            WITH_DEPS="no"; shift ;;
        --dry-run)
            DRY_RUN=true; shift ;;
        --no-prompt)
            NO_PROMPT=true; shift ;;
        -h|--help)
            cat <<'USAGE'
lazyide installer

Usage:
  curl -fsSL https://tysonlabs.dev/lazyide/install.sh | sh
  sh install.sh [OPTIONS]

Options:
  --prefix <path>   Install directory (default: /usr/local/bin, or $LAZYIDE_INSTALL_DIR)
  --version <tag>   Install a specific version (e.g. v0.3.0; default: latest)
  --with-deps       Also install optional dependencies (ripgrep, rust-analyzer)
  --no-deps         Skip optional dependency installation
  --dry-run         Print actions without executing
  --no-prompt       Non-interactive mode (for CI)
  -h, --help        Show this help
USAGE
            exit 0 ;;
        *)
            err "Unknown option: $1 (see --help)" ;;
    esac
done

# --- Resolve install directory ---
resolve_install_dir() {
    # If user explicitly set --prefix or LAZYIDE_INSTALL_DIR, use that
    if [ "$EXPLICIT_PREFIX" = true ] || [ -n "$INSTALL_DIR" ]; then
        return
    fi

    # Try /usr/local/bin first (already in PATH on most systems)
    if [ -w /usr/local/bin ] 2>/dev/null; then
        INSTALL_DIR="/usr/local/bin"
    elif [ "$(id -u)" = "0" ]; then
        INSTALL_DIR="/usr/local/bin"
    else
        # Check if we can sudo
        if has_cmd sudo && sudo -n true 2>/dev/null; then
            INSTALL_DIR="/usr/local/bin"
        elif has_cmd sudo; then
            if prompt_yn "Install to /usr/local/bin (requires sudo)?"; then
                INSTALL_DIR="/usr/local/bin"
                NEED_SUDO=true
            else
                INSTALL_DIR="${HOME}/.local/bin"
            fi
        else
            INSTALL_DIR="${HOME}/.local/bin"
        fi
    fi

    info "Install directory: ${BOLD}${INSTALL_DIR}${RESET}"
}

# --- Platform detection ---
detect_platform() {
    OS="$(uname -s)"
    ARCH="$(uname -m)"

    case "$OS" in
        Linux)  PLATFORM="linux" ;;
        Darwin) PLATFORM="macos" ;;
        MINGW*|MSYS*|CYGWIN*|Windows_NT)
            err "Windows detected. Use Scoop instead:
  scoop bucket add lazyide https://github.com/TysonLabs/scoop-bucket
  scoop install lazyide" ;;
        *) err "Unsupported OS: $OS" ;;
    esac

    case "$ARCH" in
        x86_64|amd64)   ARCH="x86_64" ;;
        aarch64|arm64)   ARCH="aarch64" ;;
        *) err "Unsupported architecture: $ARCH" ;;
    esac

    TARBALL="lazyide-${PLATFORM}-${ARCH}.tar.gz"
}

# --- Resolve version ---
resolve_version() {
    if [ -n "$VERSION" ]; then
        # Ensure version starts with v
        case "$VERSION" in
            v*) ;;
            *)  VERSION="v${VERSION}" ;;
        esac
        info "Using specified version: ${VERSION}"
        return
    fi

    info "Fetching latest release..."
    LATEST_JSON="$(download_stdout "${GITHUB_API}/repos/${REPO}/releases/latest")"
    VERSION="$(printf '%s' "$LATEST_JSON" | grep '"tag_name"' | head -1 | sed 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/')"

    [ -n "$VERSION" ] || err "Failed to determine latest version from GitHub API"
    info "Latest version: ${BOLD}${VERSION}${RESET}"
}

# --- Check existing install ---
check_existing() {
    if has_cmd lazyide; then
        EXISTING_PATH="$(command -v lazyide)"
        EXISTING_VERSION="$(lazyide --version 2>/dev/null | head -1 || echo "unknown")"
        info "Found existing install: ${EXISTING_VERSION} at ${EXISTING_PATH}"
        CLEAN_VERSION="$(printf '%s' "$VERSION" | sed 's/^v//')"
        case "$EXISTING_VERSION" in
            *"$CLEAN_VERSION"*)
                info "Already up to date (${VERSION})"
                if [ "$DRY_RUN" = false ]; then
                    exit 0
                fi
                ;;
            *)
                info "Will upgrade to ${VERSION}"
                ;;
        esac
    fi
}

# --- Checksum verification ---
verify_checksum() {
    tarball_path="$1"
    checksums_path="$2"

    expected="$(grep "$(basename "$tarball_path")" "$checksums_path" | awk '{print $1}')"
    [ -n "$expected" ] || { warn "No checksum found for $(basename "$tarball_path"), skipping verification"; return 0; }

    if has_cmd sha256sum; then
        actual="$(sha256sum "$tarball_path" | awk '{print $1}')"
    elif has_cmd shasum; then
        actual="$(shasum -a 256 "$tarball_path" | awk '{print $1}')"
    else
        warn "No sha256sum or shasum found, skipping checksum verification"
        return 0
    fi

    if [ "$expected" = "$actual" ]; then
        info "Checksum verified"
    else
        err "Checksum mismatch!
  Expected: ${expected}
  Got:      ${actual}
  The download may be corrupted. Aborting."
    fi
}

# --- Install ---
do_install() {
    DOWNLOAD_URL="${GITHUB_DL}/${REPO}/releases/download/${VERSION}/${TARBALL}"
    CHECKSUMS_URL="${GITHUB_DL}/${REPO}/releases/download/${VERSION}/checksums.sha256"

    if [ "$DRY_RUN" = true ]; then
        printf "\n${BOLD}Dry run — would perform:${RESET}\n"
        printf "  1. Download  %s\n" "$DOWNLOAD_URL"
        printf "  2. Download  %s\n" "$CHECKSUMS_URL"
        printf "  3. Verify    SHA256 checksum\n"
        printf "  4. Extract   lazyide to %s/lazyide\n" "$INSTALL_DIR"
        printf "  5. Health    lazyide --version\n"
        return
    fi

    TMPDIR_PATH="$(mktemp -d)"
    trap 'rm -rf "$TMPDIR_PATH"' EXIT

    info "Downloading ${TARBALL}..."
    download "$DOWNLOAD_URL" "${TMPDIR_PATH}/${TARBALL}"

    info "Downloading checksums..."
    if download "${CHECKSUMS_URL}" "${TMPDIR_PATH}/checksums.sha256" 2>/dev/null; then
        verify_checksum "${TMPDIR_PATH}/${TARBALL}" "${TMPDIR_PATH}/checksums.sha256"
    else
        warn "checksums.sha256 not found in release, skipping verification"
    fi

    info "Extracting..."
    tar xzf "${TMPDIR_PATH}/${TARBALL}" -C "$TMPDIR_PATH"

    maybe_sudo mkdir -p "$INSTALL_DIR"
    maybe_sudo cp "${TMPDIR_PATH}/lazyide" "${INSTALL_DIR}/lazyide"
    maybe_sudo chmod +x "${INSTALL_DIR}/lazyide"
    info "Installed to ${BOLD}${INSTALL_DIR}/lazyide${RESET}"
}

# --- PATH check ---
check_path() {
    case ":${PATH}:" in
        *":${INSTALL_DIR}:"*) return ;;
    esac

    warn "${INSTALL_DIR} is not in your PATH"

    SHELL_NAME="$(basename "${SHELL:-/bin/sh}")"
    case "$SHELL_NAME" in
        zsh)  SHELL_RC="$HOME/.zshrc" ;;
        fish) SHELL_RC="" ;;
        *)    SHELL_RC="$HOME/.bashrc" ;;
    esac

    PATH_LINE="export PATH=\"${INSTALL_DIR}:\$PATH\""

    if [ -n "$SHELL_RC" ]; then
        if prompt_yn "Add ${INSTALL_DIR} to PATH in ${SHELL_RC}?"; then
            printf '\n# Added by lazyide installer\n%s\n' "$PATH_LINE" >> "$SHELL_RC"
            PATH_MODIFIED=true
            info "Added to ${SHELL_RC}"
            return
        fi
    fi

    printf "\n  Add it manually:\n\n"
    case "$SHELL_NAME" in
        fish)
            printf "    %sfish_add_path %s%s\n" "$CYAN" "$INSTALL_DIR" "$RESET"
            ;;
        *)
            printf "    %secho '%s' >> %s%s\n" "$CYAN" "$PATH_LINE" "${SHELL_RC:-~/.bashrc}" "$RESET"
            ;;
    esac
    printf "\n  Then restart your shell or run: %s%s%s\n\n" "$CYAN" "$PATH_LINE" "$RESET"
}

# --- Optional dependencies ---
install_deps() {
    if [ "$WITH_DEPS" = "no" ]; then
        return
    fi

    MISSING_DEPS=""
    if ! has_cmd rg; then
        MISSING_DEPS="${MISSING_DEPS} ripgrep"
    fi
    if ! has_cmd rust-analyzer; then
        MISSING_DEPS="${MISSING_DEPS} rust-analyzer"
    fi

    [ -n "$MISSING_DEPS" ] || return 0

    if [ "$WITH_DEPS" != "yes" ]; then
        printf "\n"
        info "Optional dependencies not found:${MISSING_DEPS}"
        if ! prompt_yn "Install them now?"; then
            return
        fi
    fi

    if ! has_cmd rg; then
        install_ripgrep
    fi
    if ! has_cmd rust-analyzer; then
        install_rust_analyzer
    fi
}

install_ripgrep() {
    info "Installing ripgrep..."
    if [ "$DRY_RUN" = true ]; then
        printf "  Would install ripgrep\n"
        return
    fi

    if [ "$PLATFORM" = "macos" ] && has_cmd brew; then
        brew install ripgrep
    elif has_cmd apt-get; then
        sudo apt-get update -qq && sudo apt-get install -y -qq ripgrep
    elif has_cmd dnf; then
        sudo dnf install -y ripgrep
    elif has_cmd pacman; then
        sudo pacman -S --noconfirm ripgrep
    elif has_cmd apk; then
        sudo apk add ripgrep
    else
        install_ripgrep_binary
    fi
}

install_ripgrep_binary() {
    info "Installing ripgrep from GitHub binary..."
    RG_VERSION="$(download_stdout "${GITHUB_API}/repos/BurntSushi/ripgrep/releases/latest" | grep '"tag_name"' | head -1 | sed 's/.*"\([^"]*\)".*/\1/')"
    RG_ARCH="$ARCH"
    if [ "$PLATFORM" = "linux" ]; then
        RG_TARGET="${RG_ARCH}-unknown-linux-musl"
    else
        RG_TARGET="${RG_ARCH}-apple-darwin"
    fi
    RG_URL="https://github.com/BurntSushi/ripgrep/releases/download/${RG_VERSION}/ripgrep-${RG_VERSION}-${RG_TARGET}.tar.gz"
    RG_TMP="$(mktemp -d)"
    download "$RG_URL" "${RG_TMP}/rg.tar.gz"
    tar xzf "${RG_TMP}/rg.tar.gz" -C "$RG_TMP" --strip-components=1
    maybe_sudo cp "${RG_TMP}/rg" "${INSTALL_DIR}/rg"
    maybe_sudo chmod +x "${INSTALL_DIR}/rg"
    rm -rf "$RG_TMP"
    info "ripgrep installed to ${INSTALL_DIR}/rg"
}

install_rust_analyzer() {
    info "Installing rust-analyzer..."
    if [ "$DRY_RUN" = true ]; then
        printf "  Would install rust-analyzer\n"
        return
    fi

    if [ "$PLATFORM" = "macos" ] && has_cmd brew; then
        brew install rust-analyzer
    elif has_cmd rustup; then
        rustup component add rust-analyzer
    else
        install_rust_analyzer_binary
    fi
}

install_rust_analyzer_binary() {
    info "Installing rust-analyzer from GitHub binary..."
    if [ "$PLATFORM" = "macos" ]; then
        RA_TARGET="${ARCH}-apple-darwin"
    else
        RA_TARGET="${ARCH}-unknown-linux-gnu"
    fi
    RA_URL="https://github.com/rust-lang/rust-analyzer/releases/latest/download/rust-analyzer-${RA_TARGET}.gz"
    RA_TMP="$(mktemp -d)"
    download "$RA_URL" "${RA_TMP}/rust-analyzer.gz"
    gunzip "${RA_TMP}/rust-analyzer.gz"
    maybe_sudo cp "${RA_TMP}/rust-analyzer" "${INSTALL_DIR}/rust-analyzer"
    maybe_sudo chmod +x "${INSTALL_DIR}/rust-analyzer"
    rm -rf "$RA_TMP"
    info "rust-analyzer installed to ${INSTALL_DIR}/rust-analyzer"
}

# --- Health check ---
health_check() {
    if [ "$DRY_RUN" = true ]; then
        return
    fi

    if [ -x "${INSTALL_DIR}/lazyide" ]; then
        INSTALLED_VERSION="$("${INSTALL_DIR}/lazyide" --version 2>/dev/null | head -1 || echo "")"
        if [ -n "$INSTALLED_VERSION" ]; then
            info "Verified: ${INSTALLED_VERSION}"
        else
            warn "Binary installed but --version returned no output"
        fi
    else
        warn "Binary not found at ${INSTALL_DIR}/lazyide after install"
    fi
}

# --- Main ---
main() {
    printf "\n  ${BOLD}lazyide installer${RESET}\n\n"

    detect_platform
    info "Detected: ${PLATFORM} ${ARCH}"

    resolve_install_dir
    resolve_version
    check_existing
    do_install

    if [ "$DRY_RUN" = false ]; then
        check_path
        install_deps
        health_check
    fi

    if [ "$DRY_RUN" = false ]; then
        printf "\n  ${GREEN}${BOLD}lazyide ${VERSION} installed successfully!${RESET}\n"
        printf "  Run ${CYAN}lazyide${RESET} to get started.\n"
        printf "  Run ${CYAN}lazyide --setup${RESET} to check optional tool status.\n\n"

        if [ "$PATH_MODIFIED" = true ]; then
            printf "  ${YELLOW}To use lazyide now, run:${RESET}\n\n"
            printf "    ${CYAN}export PATH=\"%s:\$PATH\"${RESET}\n\n" "$INSTALL_DIR"
            printf "  Or restart your terminal.\n\n"
        fi
    fi
}

main
