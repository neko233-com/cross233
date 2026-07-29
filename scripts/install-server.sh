#!/usr/bin/env bash
# Installs the Linux server binary and creates a systemd service. Run as root or with sudo.
set -Eeuo pipefail

REPOSITORY="${CROSS233_REPOSITORY:-neko233-com/cross233}"
VERSION="${CROSS233_VERSION:-latest}"
INSTALL_DIR="${CROSS233_INSTALL_DIR:-/usr/local/bin}"
CONFIG_DIR="${CROSS233_CONFIG_DIR:-/etc/cross233}"
DATA_DIR="${CROSS233_DATA_DIR:-/var/lib/cross233}"
AUTH_KEY="${CROSS233_AUTH_KEY:-}"
AUTH_KEY_PROVIDED=false
[ -n "$AUTH_KEY" ] && AUTH_KEY_PROVIDED=true
SYSTEM_USER="${CROSS233_SYSTEM_USER:-cross233}"
CONTROL_PORT="${CROSS233_CONTROL_PORT:-7710}"
WEB_PORT="${CROSS233_WEB_PORT:-7711}"
UNIT_FILE="/etc/systemd/system/cross233.service"

fail() { printf 'cross233 install: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "missing required command: $1"; }
download() {
  if command -v curl >/dev/null 2>&1; then
    curl -fL --retry 3 -o "$2" "$1"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$2" "$1"
  else
    fail "missing required command: curl or wget"
  fi
}
as_root() {
  if [ "$(id -u)" -eq 0 ]; then "$@"; elif command -v sudo >/dev/null 2>&1; then sudo "$@"; else fail "run as root or install sudo"; fi
}

case "$(uname -s)" in Linux) ;; *) fail "server installer supports Linux only" ;; esac
case "$(uname -m)" in
  x86_64|amd64) ARCH="x86_64" ;;
  aarch64|arm64) ARCH="aarch64" ;;
  armv7l) ARCH="armv7" ;;
  *) fail "unsupported architecture: $(uname -m)" ;;
esac
need systemctl
need getent
need groupadd
need useradd
need runuser
need sha256sum

if [ "$VERSION" = "latest" ]; then
  RELEASE_JSON="$(mktemp)"
  download "https://api.github.com/repos/$REPOSITORY/releases/latest" "$RELEASE_JSON"
  VERSION=$(grep '"tag_name"' "$RELEASE_JSON" | head -1 | sed -E 's/.*"tag_name":\s*"([^"]+)".*/\1/')
  rm -f "$RELEASE_JSON"
fi

if [ "$ARCH" = "armv7" ]; then
    TARGET="armv7-unknown-linux-musleabihf"
else
    TARGET="${ARCH}-unknown-linux-musl"
fi
ASSET="cross233-${VERSION}-${TARGET}.tar.gz"
BASE_URL="https://github.com/$REPOSITORY/releases/download/$VERSION"
TMP_DIR="$(mktemp -d)"
trap 'rm -rf "$TMP_DIR"' EXIT

if ! download "$BASE_URL/$ASSET" "$TMP_DIR/$ASSET" 2>/dev/null; then
  fail "failed to download $ASSET; build from source: cargo build --release -p cross233-server"
fi
download "$BASE_URL/$ASSET.sha256" "$TMP_DIR/$ASSET.sha256"
(cd "$TMP_DIR" && sha256sum -c "$ASSET.sha256") ||
  fail "release checksum verification failed"

tar -xzf "$TMP_DIR/$ASSET" -C "$TMP_DIR"
[ -f "$TMP_DIR/cross233-server" ] || fail "binary not found in archive"
chmod 0755 "$TMP_DIR/cross233-server"

if ! getent group "$SYSTEM_USER" >/dev/null 2>&1; then
  as_root groupadd --system "$SYSTEM_USER"
fi
if ! getent passwd "$SYSTEM_USER" >/dev/null 2>&1; then
  NOLOGIN_SHELL=$(command -v nologin || printf '/usr/sbin/nologin')
  as_root useradd --system --gid "$SYSTEM_USER" --home-dir "$DATA_DIR" \
    --shell "$NOLOGIN_SHELL" "$SYSTEM_USER"
fi

as_root install -d -m 0755 "$INSTALL_DIR"
as_root install -d -o root -g "$SYSTEM_USER" -m 0750 "$CONFIG_DIR"
as_root install -d -o "$SYSTEM_USER" -g "$SYSTEM_USER" -m 0750 "$DATA_DIR"

if [ -z "$AUTH_KEY" ] && as_root test -f "$DATA_DIR/auth.key"; then
  AUTH_KEY=$(as_root cat "$DATA_DIR/auth.key")
fi
if [ -z "$AUTH_KEY" ]; then
  AUTH_KEY=$(openssl rand -hex 32 2>/dev/null || head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n')
fi
if [ "$AUTH_KEY_PROVIDED" = true ] || ! as_root test -f "$DATA_DIR/auth.key"; then
  printf '%s\n' "$AUTH_KEY" | as_root tee "$DATA_DIR/auth.key" >/dev/null
fi
as_root chown "$SYSTEM_USER:$SYSTEM_USER" "$DATA_DIR/auth.key"
as_root chmod 0600 "$DATA_DIR/auth.key"

if [ ! -f "$CONFIG_DIR/server.toml" ]; then
  as_root tee "$CONFIG_DIR/server.toml" >/dev/null <<EOF
bind = "0.0.0.0"
proxy_bind = "0.0.0.0"
auth_key_file = "$DATA_DIR/auth.key"
cert_file = "$DATA_DIR/cert.pem"
key_file = "$DATA_DIR/key.pem"
control_port = $CONTROL_PORT
web_port = $WEB_PORT
http_vhost_port = 0
https_vhost_port = 0
tcpmux_port = 0
port_min = 7712
port_max = 7720
allow_privileged_ports = false
protected_ports = [22]
qcp_port = 7713
qcp_tunnel_port = 0
EOF
fi
as_root chown root:"$SYSTEM_USER" "$CONFIG_DIR/server.toml"
as_root chmod 0640 "$CONFIG_DIR/server.toml"

as_root install -m 0755 "$TMP_DIR/cross233-server" "$INSTALL_DIR/cross233-server.new"
if ! as_root runuser -u "$SYSTEM_USER" -- \
  "$INSTALL_DIR/cross233-server.new" --check-config -c "$CONFIG_DIR/server.toml" >/dev/null; then
  as_root rm -f "$INSTALL_DIR/cross233-server.new"
  fail "configuration validation failed; existing service was not changed"
fi

HAD_BINARY=false
HAD_UNIT=false
if as_root test -f "$INSTALL_DIR/cross233-server"; then
  HAD_BINARY=true
  as_root cp -p "$INSTALL_DIR/cross233-server" "$TMP_DIR/cross233-server.previous"
fi
if as_root test -f "$UNIT_FILE"; then
  HAD_UNIT=true
  as_root cp -p "$UNIT_FILE" "$TMP_DIR/cross233.service.previous"
fi

as_root mv -f "$INSTALL_DIR/cross233-server.new" "$INSTALL_DIR/cross233-server"

cat >"$TMP_DIR/cross233.service" <<EOF
[Unit]
Description=cross233 reverse tunnel server
After=network-online.target
Wants=network-online.target
StartLimitIntervalSec=60
StartLimitBurst=5

[Service]
Type=simple
User=$SYSTEM_USER
Group=$SYSTEM_USER
UMask=0027
WorkingDirectory=$DATA_DIR
ExecStartPre=$INSTALL_DIR/cross233-server --check-config -c $CONFIG_DIR/server.toml
ExecStart=$INSTALL_DIR/cross233-server -c $CONFIG_DIR/server.toml
Restart=on-failure
RestartSec=3
TimeoutStopSec=15
KillMode=mixed
NoNewPrivileges=true
PrivateTmp=true
PrivateDevices=true
ProtectHome=true
ProtectSystem=strict
ProtectClock=true
ProtectControlGroups=true
ProtectHostname=true
ProtectKernelLogs=true
ProtectKernelModules=true
ProtectKernelTunables=true
RestrictAddressFamilies=AF_UNIX AF_INET AF_INET6
RestrictNamespaces=true
RestrictRealtime=true
RestrictSUIDSGID=true
LockPersonality=true
CapabilityBoundingSet=
AmbientCapabilities=
SystemCallArchitectures=native
ReadWritePaths=$DATA_DIR
LimitNOFILE=65536
TasksMax=512
MemoryMax=512M
CPUQuota=80%
OOMScoreAdjust=500

[Install]
WantedBy=multi-user.target
EOF
as_root install -o root -g root -m 0644 "$TMP_DIR/cross233.service" "$UNIT_FILE"
as_root systemctl daemon-reload
as_root systemctl enable cross233

if ! as_root systemctl restart cross233 ||
   ! as_root systemctl is-active --quiet cross233; then
  printf '%s\n' "cross233 failed to start; rolling back" >&2
  if [ "$HAD_BINARY" = true ]; then
    as_root install -m 0755 "$TMP_DIR/cross233-server.previous" "$INSTALL_DIR/cross233-server.rollback"
    as_root mv -f "$INSTALL_DIR/cross233-server.rollback" "$INSTALL_DIR/cross233-server"
  fi
  if [ "$HAD_UNIT" = true ]; then
    as_root install -o root -g root -m 0644 "$TMP_DIR/cross233.service.previous" "$UNIT_FILE"
  fi
  as_root systemctl daemon-reload
  if [ "$HAD_BINARY" = true ] && [ "$HAD_UNIT" = true ]; then
    as_root systemctl restart cross233 || true
  else
    as_root systemctl stop cross233 || true
  fi
  fail "deployment rolled back; inspect: journalctl -u cross233 -n 100"
fi

printf '\n%s\n' "=== cross233 server installed ==="
printf '%s\n' "Binary:   $INSTALL_DIR/cross233-server"
printf '%s\n' "Config:   $CONFIG_DIR/server.toml"
printf '%s\n' "Data:     $DATA_DIR"
printf '%s\n' "Auth key: $DATA_DIR/auth.key (mode 0600; preserved on upgrades)"
printf '%s\n' "Web UI:   http://<server-ip>:$WEB_PORT"
printf '%s\n' "Control:  port $CONTROL_PORT"
printf '%s\n' ""
printf '%s\n' "Service status: systemctl status cross233"
printf '%s\n' "Logs:           journalctl -u cross233 -f"
printf '%s\n' "Isolation:      dedicated user $SYSTEM_USER; no firewall/sysctl/SSH changes"
