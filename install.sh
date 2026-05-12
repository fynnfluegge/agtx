#!/bin/bash
set -e

# agtx installer
# Usage: curl -fsSL https://raw.githubusercontent.com/fynnfluegge/agtx/main/install.sh | bash

REPO="fynnfluegge/agtx"
BINARY_NAME="agtx"
INSTALL_DIR="${AGTX_INSTALL_DIR:-$HOME/.local/bin}"

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[0;33m'
BLUE='\033[0;34m'
NC='\033[0m' # No Color

info() {
    echo -e "${BLUE}==>${NC} $1"
}

success() {
    echo -e "${GREEN}==>${NC} $1"
}

warn() {
    echo -e "${YELLOW}==>${NC} $1"
}

error() {
    echo -e "${RED}==>${NC} $1"
    exit 1
}

# Detect OS
detect_os() {
    case "$(uname -s)" in
        Linux*)  echo "linux" ;;
        Darwin*) echo "darwin" ;;
        *)       error "Unsupported OS: $(uname -s)" ;;
    esac
}

# Detect architecture
detect_arch() {
    case "$(uname -m)" in
        x86_64)  echo "x86_64" ;;
        amd64)   echo "x86_64" ;;
        arm64)   echo "aarch64" ;;
        aarch64) echo "aarch64" ;;
        *)       error "Unsupported architecture: $(uname -m)" ;;
    esac
}

# Get latest release version
get_latest_version() {
    curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" | \
        grep '"tag_name"' | \
        sed -E 's/.*"([^"]+)".*/\1/'
}

# Main installation
main() {
    echo ""
    echo "  ╭──────────────────────────╮"
    echo "  │    agtx installer        │"
    echo "  │    Terminal Kanban       │"
    echo "  ╰──────────────────────────╯"
    echo ""

    OS=$(detect_os)
    ARCH=$(detect_arch)

    info "Detected: ${OS}/${ARCH}"

    # Check for required tools
    if ! command -v curl &> /dev/null; then
        error "curl is required but not installed"
    fi

    # Get latest version
    info "Fetching latest version..."
    VERSION=$(get_latest_version)

    if [ -z "$VERSION" ]; then
        error "Could not determine latest version. Check https://github.com/${REPO}/releases"
    fi

    info "Latest version: ${VERSION}"

    # Construct download URL
    ARCHIVE_NAME="${BINARY_NAME}-${VERSION}-${ARCH}-${OS}.tar.gz"
    DOWNLOAD_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE_NAME}"

    # Create temp directory
    TMP_DIR=$(mktemp -d)
    trap "rm -rf ${TMP_DIR}" EXIT

    # Download
    info "Downloading ${ARCHIVE_NAME}..."
    if ! curl -fsSL "${DOWNLOAD_URL}" -o "${TMP_DIR}/${ARCHIVE_NAME}"; then
        error "Download failed. Check if release exists: ${DOWNLOAD_URL}"
    fi

    # Verify checksum (if available)
    CHECKSUM_URL="https://github.com/${REPO}/releases/download/${VERSION}/${ARCHIVE_NAME}.sha256"
    info "Verifying checksum..."
    if curl -fsSL "${CHECKSUM_URL}" -o "${TMP_DIR}/${ARCHIVE_NAME}.sha256" 2>/dev/null; then
        PREV_DIR=$(pwd)
        cd "${TMP_DIR}"
        if command -v sha256sum &> /dev/null; then
            sha256sum -c "${ARCHIVE_NAME}.sha256" || error "Checksum verification failed"
        elif command -v shasum &> /dev/null; then
            shasum -a 256 -c "${ARCHIVE_NAME}.sha256" || error "Checksum verification failed"
        else
            warn "No sha256sum or shasum found, skipping checksum verification"
        fi
        cd "${PREV_DIR}"
        success "Checksum verified"
    else
        warn "Checksum file not found, skipping verification"
    fi

    # Extract
    info "Extracting..."
    tar -xzf "${TMP_DIR}/${ARCHIVE_NAME}" -C "${TMP_DIR}"

    # Create install directory if needed
    if [ ! -d "${INSTALL_DIR}" ]; then
        info "Creating ${INSTALL_DIR}..."
        mkdir -p "${INSTALL_DIR}"
    fi

    # Install binary
    info "Installing to ${INSTALL_DIR}/${BINARY_NAME}..."
    mv "${TMP_DIR}/${BINARY_NAME}" "${INSTALL_DIR}/${BINARY_NAME}"
    chmod +x "${INSTALL_DIR}/${BINARY_NAME}"

    # Check if install dir is in PATH
    if [[ ":$PATH:" != *":${INSTALL_DIR}:"* ]]; then
        warn "${INSTALL_DIR} is not in your PATH"
        echo ""
        echo "Add it to your shell config:"
        echo ""
        echo "  # For bash (~/.bashrc)"
        echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
        echo ""
        echo "  # For zsh (~/.zshrc)"
        echo "  export PATH=\"\$HOME/.local/bin:\$PATH\""
        echo ""
    fi

    echo ""
    success "agtx ${VERSION} installed successfully!"
    echo ""
    echo "  Get started:"
    echo "    cd your-project"
    echo "    agtx"
    echo ""
    echo "  Or run in dashboard mode:"
    echo "    agtx -g"
    echo ""

    # Check dependencies
    echo "  Checking dependencies..."

    if command -v tmux &> /dev/null; then
        echo -e "    ${GREEN}✓${NC} tmux"
    else
        echo -e "    ${RED}✗${NC} tmux (required)"
    fi

    if command -v git &> /dev/null; then
        echo -e "    ${GREEN}✓${NC} git"
    else
        echo -e "    ${RED}✗${NC} git (required)"
    fi

    if command -v gh &> /dev/null; then
        echo -e "    ${GREEN}✓${NC} gh"
    else
        echo -e "    ${YELLOW}○${NC} gh (optional, for PR operations)"
    fi

    if command -v claude &> /dev/null; then
        echo -e "    ${GREEN}✓${NC} claude"
    else
        echo -e "    ${YELLOW}○${NC} claude (optional, for Claude Code integration)"
    fi

    echo ""
}

main "$@"
