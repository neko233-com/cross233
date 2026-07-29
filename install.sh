#!/usr/bin/env bash
# Cross233 One-Click Install Script for Linux/macOS
# Usage: curl -fsSL https://raw.githubusercontent.com/neko233-com/cross233/main/install.sh | bash
# Or: bash install.sh

set -euo pipefail

INSTALL_DIR="${INSTALL_DIR:-$HOME/.cross233}"
VERSION="${VERSION:-latest}"
COMPONENT="${COMPONENT:-both}"

REPO_URL="https://github.com/neko233-com/cross233"
RELEASES_URL="https://github.com/neko233-com/cross233/releases"

RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m'

info()  { echo -e "${CYAN}[*]${NC} $*"; }
ok()    { echo -e "${GREEN}[+]${NC} $*"; }
warn()  { echo -e "${YELLOW}[!]${NC} $*"; }
err()   { echo -e "${RED}[-]${NC} $*" >&2; }

echo ""
echo -e "${CYAN}=== Cross233 Installer ===${NC}"
echo "Install dir: $INSTALL_DIR"
echo "Component:   $COMPONENT"
echo ""

# Detect architecture and OS
detect_platform() {
    local os arch
    os="$(uname -s | tr '[:upper:]' '[:lower:]')"
    arch="$(uname -m)"
    case "$arch" in
        x86_64|amd64) arch="x86_64" ;;
        aarch64|arm64) arch="aarch64" ;;
        armv7l) arch="armv7" ;;
        *) err "Unsupported architecture: $arch"; exit 1 ;;
    esac
    case "$os" in
        linux)
            if [ "$arch" = "armv7" ]; then
                os="unknown-linux-musleabihf"
            else
                os="unknown-linux-musl"
            fi
            ;;
        darwin) os="apple-darwin" ;;
        *) err "Unsupported OS: $os"; exit 1 ;;
    esac
    TRIPLE="${arch}-${os}"
}

detect_platform

mkdir -p "$INSTALL_DIR"

# Check for Rust
has_rust=false
if command -v cargo >/dev/null 2>&1; then
    has_rust=true
    ok "Rust toolchain detected"
fi

download() {
    local url="$1"
    local out="$2"
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL -o "$out" "$url"
    elif command -v wget >/dev/null 2>&1; then
        wget -q -O "$out" "$url"
    else
        err "Neither curl nor wget found"
        return 1
    fi
}

verify_sha256() {
    local file="$1"
    local checksum_file="$2"
    local expected actual
    expected=$(awk '{print tolower($1); exit}' "$checksum_file")
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$file" | awk '{print tolower($1)}')
    elif command -v shasum >/dev/null 2>&1; then
        actual=$(shasum -a 256 "$file" | awk '{print tolower($1)}')
    else
        err "Neither sha256sum nor shasum is available"
        return 1
    fi
    [ -n "$expected" ] && [ "$expected" = "$actual" ]
}

install_prebuilt() {
    local bin_name="$1"
    local tag
    local tmpdir
    tmpdir=$(mktemp -d)

    if [ "$VERSION" = "latest" ]; then
        local release_json="$tmpdir/release.json"
        if ! download "https://api.github.com/repos/neko233-com/cross233/releases/latest" "$release_json"; then
            rm -rf "$tmpdir"
            return 1
        fi
        tag=$(grep '"tag_name"' "$release_json" | head -1 | sed -E 's/.*"tag_name":\s*"([^"]+)".*/\1/')
    else
        tag="$VERSION"
    fi

    local archive_name="cross233-${tag}-${TRIPLE}.tar.gz"
    local download_url="${RELEASES_URL}/download/${tag}/${archive_name}"

    info "Downloading $download_url ..."
    if ! download "$download_url" "$tmpdir/$archive_name"; then
        warn "Prebuilt binary not found, will build from source"
        rm -rf "$tmpdir"
        return 1
    fi
    if ! download "$download_url.sha256" "$tmpdir/$archive_name.sha256" ||
       ! verify_sha256 "$tmpdir/$archive_name" "$tmpdir/$archive_name.sha256"; then
        warn "Release checksum verification failed"
        rm -rf "$tmpdir"
        return 1
    fi

    tar -xzf "$tmpdir/$archive_name" -C "$tmpdir"
    local exe_name="cross233-${bin_name}"
    cp "$tmpdir/$exe_name" "$INSTALL_DIR/$exe_name"
    chmod +x "$INSTALL_DIR/$exe_name"
    rm -rf "$tmpdir"
    ok "Installed $exe_name to $INSTALL_DIR/$exe_name"
    return 0
}

build_from_source() {
    local bin_name="$1"

    if [ "$has_rust" != "true" ]; then
        err "Rust is required to build from source. Install from https://rustup.rs/"
        return 1
    fi

    local tmpdir
    tmpdir=$(mktemp -d)

    info "Cloning source ..."
    if [ -f "$(dirname "$0")/Cargo.toml" ]; then
        cp -r "$(dirname "$0")"/* "$tmpdir/"
    else
        git clone --depth 1 "$REPO_URL" "$tmpdir"
    fi

    pushd "$tmpdir" >/dev/null
    info "Building cross233-${bin_name} ..."
    if ! cargo build --release -p "cross233-${bin_name}"; then
        popd >/dev/null
        rm -rf "$tmpdir"
        return 1
    fi
    local exe_name="cross233-${bin_name}"
    cp "target/release/$exe_name" "$INSTALL_DIR/$exe_name"
    chmod +x "$INSTALL_DIR/$exe_name"
    popd >/dev/null
    rm -rf "$tmpdir"
    ok "Built and installed $exe_name to $INSTALL_DIR/$exe_name"
}

install_binary() {
    local bin_name="$1"
    info "Installing cross233-${bin_name} ..."

    local installed=false
    if install_prebuilt "$bin_name" 2>/dev/null; then
        installed=true
    elif build_from_source "$bin_name"; then
        installed=true
    fi

    if [ "$installed" = "true" ]; then
        local config_name
        if [ "$bin_name" = "server" ]; then
            config_name="server.toml"
        else
            config_name="client.toml"
        fi
        if [ ! -f "$INSTALL_DIR/$config_name" ]; then
            local sample
            sample="$(dirname "$0")/examples/$config_name"
            if [ -f "$sample" ]; then
                cp "$sample" "$INSTALL_DIR/$config_name"
            else
                download \
                    "https://raw.githubusercontent.com/neko233-com/cross233/main/examples/$config_name" \
                    "$INSTALL_DIR/$config_name" ||
                    warn "Could not install the sample $config_name"
            fi
        fi
    fi
}

install_client_templates() {
    local destination="$INSTALL_DIR/templates/docker-static"
    local source_dir
    source_dir="$(dirname "$0")/examples/docker-static"
    mkdir -p "$destination/site"

    if [ -d "$source_dir" ]; then
        cp -R "$source_dir/." "$destination/"
    else
        local base="https://raw.githubusercontent.com/neko233-com/cross233/main/examples/docker-static"
        local file
        for file in Dockerfile nginx.conf client.toml.example README.md; do
            download "$base/$file" "$destination/$file" ||
                warn "Could not install Docker template file: $file"
        done
        download "$base/site/index.html" "$destination/site/index.html" ||
            warn "Could not install Docker template HTML"
    fi
    ok "Installed Docker service template to $destination"
}

case "$COMPONENT" in
    both|server) install_binary "server" ;;
esac
case "$COMPONENT" in
    both|client)
        install_binary "client"
        install_client_templates
        ;;
esac

# Add to PATH
SHELL_NAME="$(basename "${SHELL:-/bin/bash}")"
RC_FILE=""
case "$SHELL_NAME" in
    zsh) RC_FILE="$HOME/.zshrc" ;;
    bash) RC_FILE="$HOME/.bashrc" ;;
    fish) RC_FILE="$HOME/.config/fish/config.fish" ;;
esac

if [ -n "$RC_FILE" ]; then
    if ! grep -q "$INSTALL_DIR" "$RC_FILE" 2>/dev/null; then
        echo "export PATH=\"\$PATH:$INSTALL_DIR\"" >> "$RC_FILE"
        ok "Added $INSTALL_DIR to PATH in $RC_FILE"
    fi
fi
export PATH="$PATH:$INSTALL_DIR"

echo ""
echo -e "${CYAN}=== Installation Complete ===${NC}"
echo ""
echo "Quick start:"
echo "  Server: $INSTALL_DIR/cross233-server -c $INSTALL_DIR/server.toml"
echo "  Client: $INSTALL_DIR/cross233-client -c $INSTALL_DIR/client.toml"
echo ""
echo "Web admin panel (server): http://127.0.0.1:7711"
echo "Web admin panel (client): http://127.0.0.1:7721"
echo "Docker template: $INSTALL_DIR/templates/docker-static"
echo ""
echo "Restart your shell or run: source $RC_FILE"
echo ""
