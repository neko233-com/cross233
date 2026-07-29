# Cross233

[![License: MIT](https://img.shields.io/badge/License-MIT-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Built%20with-Rust-orange.svg)](https://www.rust-lang.org/)

A fast, production-grade reverse proxy for exposing local services behind NATs and firewalls to the internet. Fully compatible with frp use cases, written in Rust for maximum performance and minimal resource usage.

## Features

- **Multi-Protocol Support**: TCP, UDP, HTTP(S) virtual hosts, STCP (secret TCP), SUDP, TCPMUX, QCP (reliable UDP)
- **TLS 1.3 Encryption**: All control connections secured with TLS 1.3, auto-generated self-signed certificates
- **Built-in Web Dashboard**: Apple-design inspired management UI with ECharts data visualization
- **Real-time Metrics**: Live bandwidth charts, connection trends, traffic distribution via ECharts
- **WebSocket Live Updates**: Real-time service status, stats, and log streaming
- **Agent CLI Automation**: Full-featured `cross233ctl` for scripting, monitoring, and CI/CD integration
- **API Token Auth**: Bearer token authentication for programmatic/agent access
- **Bandwidth Limiting**: Per-service bandwidth throttling via token bucket algorithm
- **Health Checks**: Active health checking for backend services (HTTP/TCP)
- **Proxy Protocol**: Support for PROXY protocol v1/v2 to preserve client IP addresses
- **Virtual Hosting**: HTTP/HTTPS vhost routing with subdomain and custom domain support
- **Header Manipulation**: Rewrite Host header and add/remove custom request/response headers
- **HTTP Basic Auth**: Password protection for HTTP vhost services
- **Access Control**: CIDR-based allow/deny rules for services
- **Traffic Compression**: Per-service compression option
- **Connection Groups**: Load balancing across multiple clients via group keys
- **Hot Reload**: Client config reload without restart
- **Multi-Format Config**: TOML (default), JSON, and YAML configuration support
- **QCP Transport**: Reliable UDP transport for low-latency, high-throughput scenarios
- **Cross Platform**: Windows, Linux, macOS (x86_64, aarch64)

## Architecture

```
 ┌─────────┐         TLS 1.3          ┌─────────┐
 │ Client  │ ◄──────────────────────► │ Server  │
 │ (frpc)  │   Control Connection     │ (frps)  │
 └────┬────┘                          └────┬────┘
      │                                    │
      │     Tunnel Connections             │
      │◄──────────────────────────────────►│
      │  (per-connection TLS streams)      │
      │                                    │
      │     QCP (reliable UDP)             │
      │◄══════════════════════════════════►│
      │                                    │
 ┌────┴────┐                         ┌─────┴─────┐
 │ Local   │                         │ Public    │
 │ Service │◄── traffic ────────────►│ Ports /   │
 │ :22,:80 │                         │ Vhosts    │
 └─────────┘                         └───────────┘
```

## Quick Start

### One-Click Installation

**Windows:**
```powershell
irm https://raw.githubusercontent.com/neko233-com/cross233/main/install.ps1 | iex
```

**Linux/macOS:**
```bash
curl -fsSL https://raw.githubusercontent.com/neko233-com/cross233/main/install.sh | bash
```

### Manual Build

Prerequisites: Rust 1.70+, Node.js 18+ (for web UI)

```bash
# Build web UI
cd web && npm install && npm run build && cd ..

# Build Rust binaries (release)
cargo build --release

# Binaries are at:
#   target/release/cross233-server
#   target/release/cross233-client
```

## Ports

| Port  | Protocol | Purpose                  |
|-------|----------|--------------------------|
| 7710  | TCP/TLS  | Control channel          |
| 7711  | TCP/HTTP | Server web dashboard     |
| 7712-7720 | TCP  | Auto-allocated TCP/UDP ports |
| 7713  | UDP      | QCP transport            |
| 7714  | UDP      | TLS-over-QCP tunnel data (optional) |
| 7721  | TCP/HTTP | Client web dashboard (local) |
| 80    | TCP      | HTTP vhost (configurable)|
| 443   | TCP/TLS  | HTTPS vhost (configurable)|

## Quick Start Guide

### 1. Start the Server

On your public server (e.g., VPS with IP `1.2.3.4`):

```bash
# Generate a random auth key (or set your own)
AUTH_KEY=$(openssl rand -hex 32)
echo "Auth key: $AUTH_KEY"

# Start server with minimal config
cat > server.toml << EOF
bind = "0.0.0.0"
auth_key = "$AUTH_KEY"
web_port = 7711
control_port = 7710
EOF

cross233-server -c server.toml
```

The server will auto-generate TLS certificates on first run.

### 2. Start the Client

On your local machine:

```bash
cat > client.toml << EOF
server = "1.2.3.4:7710"
auth_key = "$AUTH_KEY"

[[services]]
name = "ssh"
type = "tcp"
localAddr = "127.0.0.1:22"
remotePort = 7712

[[services]]
name = "web"
type = "http"
localAddr = "127.0.0.1:8080"
subdomain = "myapp"
EOF

cross233-client -c client.toml
```

### 3. Access Your Services

- **SSH**: `ssh -p 7712 user@1.2.3.4`
- **Web dashboard**: http://1.2.3.4:7711
- **HTTP vhost**: http://myapp.1.2.3.4 (if subdomain_host configured) or via custom domain

## Configuration

Cross233 supports TOML (default), JSON, and YAML configuration formats. The format is auto-detected from the file extension.

### Server Configuration (server.toml)

```toml
bind = "0.0.0.0"                    # Bind address for all ports
auth_key = "your-secret-key"        # Authentication key (required)
# auth_key_file = "/path/to/key"    # Read key from file instead
control_port = 7710                 # Control channel port
web_port = 7711                     # Web dashboard port
http_vhost_port = 80                # HTTP vhost port
https_vhost_port = 443              # HTTPS vhost port
qcp_port = 7713                     # QCP UDP port
qcp_tunnel_port = 7714              # TLS-over-QCP tunnel port (0 disables)
port_min = 7712                     # Min auto-allocated port
port_max = 7720                     # Max auto-allocated port
subdomain_host = "example.com"      # Domain for subdomain routing
max_connections = 256               # Max concurrent connections
web_user = "admin"                  # Web dashboard username (optional)
web_password = "admin"              # Web dashboard password (optional)
# api_token = "your-api-token"      # Bearer token for agent/CLI automation (optional)
```

See [examples/server.toml](examples/server.toml) for full configuration.

### Client Configuration (client.toml)

```toml
server = "1.2.3.4:7710"            # Server address
auth_key = "your-secret-key"       # Must match server
# server_name = "cross233"         # TLS server name
# insecure = false                 # Skip TLS verification (not recommended)
# ca_file = "/path/to/ca.pem"      # Custom CA certificate

# Data-plane selection. `auto` tries TLS-over-QCP first and falls back to TCP
# before application data is sent. Control stays TCP/TLS.
transport = "auto"
qcp_tunnel_port = 7714
# transport_cache_file = ".cross233/transport-cache.json"
# transport_cache_ttl_secs = 86400
# transport_probe_timeout_ms = 6000

[[services]]
name = "ssh"
type = "tcp"
localAddr = "127.0.0.1:22"
remotePort = 7712

[[services]]
name = "web"
type = "http"
localAddr = "127.0.0.1:8080"
subdomain = "myapp"
# hostHeaderRewrite = "internal.local"
# httpUser = "user"
# httpPassword = "pass"
# bandwidthLimitKbps = 1024
# compression = true
```

See [examples/client.toml](examples/client.toml) for full configuration with all service types.

`transport_cache_file` is local diagnostic state only. It contains endpoint,
timestamp, selected transport, and truncated connection errors; it never stores
the auth key, HMAC proof, or local service paths. The repository ignores
`.cross233/` by default. QCP uses an application UDP socket only; it does not
modify SSH, firewall, sysctl, or other operating-system settings.

### Service Types

| Type   | Description                          | Key Fields |
|--------|--------------------------------------|------------|
| `tcp`  | TCP port forwarding                  | `remotePort` |
| `udp`  | UDP port forwarding                  | `remotePort` |
| `http` | HTTP virtual host                    | `subdomain`, `host`, `locations` |
| `https`| HTTPS virtual host (TLS passthrough) | `subdomain`, `host` |
| `stcp` | Secret TCP (visitors need key)       | `secret` |
| `sudp` | Secret UDP                           | `secret` |
| `tcpmux`| TCP HTTP connect multiplexer        | `subdomain` |
| `qcp`  | Reliable UDP transport               | `remotePort` |

### Service Options

| Option | Type | Description |
|--------|------|-------------|
| `bandwidthLimitKbps` | int | Bandwidth limit in Kbps (0 = unlimited) |
| `maxConnections` | int | Max concurrent connections |
| `compression` | bool | Enable traffic compression |
| `proxyProtocol` | bool | Enable PROXY protocol |
| `proxyProtocolVersion` | string | "v1" or "v2" |
| `httpUser`/`httpPassword` | string | HTTP Basic auth for HTTP vhosts |
| `hostHeaderRewrite` | string | Rewrite Host header |
| `allowCIDRs`/`denyCIDRs` | string[] | IP CIDR access control |
| `healthCheck` | string | Health check type: "tcp", "http" |
| `healthInterval` | int | Health check interval (seconds) |
| `healthPath` | string | HTTP health check path |
| `requestHeaders`/`responseHeaders` | map | Custom headers |

## CLI Usage

### Server

```bash
cross233-server [OPTIONS]

Options:
  -c, --config <FILE>     Config file path (TOML/JSON/YAML)
      --bind <ADDR>       Bind address
      --port <PORT>       Control port
      --web-port <PORT>   Web management port
      --auth-key <KEY>    Auth key
      --cert-file <FILE>  TLS cert file
      --key-file <FILE>   TLS key file
  -h, --help              Print help
  -V, --version           Print version
```

### Client

```bash
cross233-client [OPTIONS]

Options:
  -c, --config <FILE>         Config file path (TOML/JSON/YAML)
      --server <ADDR>         Server address (host:port)
      --auth-key <KEY>        Auth key
      --client-id <ID>        Client identifier
  -s, --services <SPEC>       Quick services (name:type:host:port[:remotePort],...)
      --insecure              Skip TLS certificate verification
      --web-addr <ADDR>       Local web dashboard address
      --ca-file <FILE>        CA certificate file
      --server-name <NAME>    TLS server name for verification
  -h, --help                  Print help
  -V, --version               Print version
```

### Quick Examples

```bash
# Expose local SSH via TCP on port 7712
cross233-client --server 1.2.3.4:7710 --auth-key mykey -s ssh:tcp:127.0.0.1:22:7712

# Expose local web server via HTTP vhost
cross233-client --server 1.2.3.4:7710 --auth-key mykey -s web:http:127.0.0.1:8080
```

## Web Dashboard

Both server and client include a built-in web management dashboard with Apple-inspired design:

- **Server Dashboard** (port 7711): ECharts data dashboard with bandwidth charts, connection trends, and traffic distribution
- **Client Dashboard** (port 7721, localhost only): View connection status, service health, local metrics

The web UI features:
- Frosted glass UI with smooth spring animations (Framer Motion)
- Real-time ECharts visualizations (bandwidth, connections, traffic distribution)
- WebSocket-powered live updates (no polling overhead)
- Client kick and service toggle controls
- Log streaming and detailed client management
- Dark mode with Apple design language

### Dashboard Charts

| Chart | Description |
|-------|-------------|
| Bandwidth Monitor | Real-time TX/RX bandwidth area chart (up to 600 data points) |
| Connection Trends | Active services, clients, and connections over time |
| Traffic Distribution | Donut chart showing traffic share across services |
| Service Health | Live status table with bandwidth limits and uptime |

## cross233ctl (Agent CLI)

`cross233ctl` is a powerful command-line tool for automation, monitoring, and scripting. It supports both PowerShell (Windows) and Bash (Linux/macOS).

### Setup

```powershell
# Windows PowerShell
$env:CROSS233_SERVER = "http://1.2.3.4:7711"
$env:CROSS233_TOKEN = "your-api-token"    # Recommended for automation
# or: $env:CROSS233_USER = "admin"; $env:CROSS233_PASSWORD = "admin"
```

```bash
# Linux/macOS
export CROSS233_SERVER="http://1.2.3.4:7711"
export CROSS233_TOKEN="your-api-token"
chmod +x scripts/cross233ctl.sh
```

### Commands

```bash
# Server monitoring
cross233ctl health                  # Health check
cross233ctl stats                   # Server statistics
cross233ctl services                # List services
cross233ctl service --name web      # Service detail
cross233ctl service-metrics --name web -n 60  # Metrics history
cross233ctl clients                 # List clients
cross233ctl logs -n 100             # Recent logs
cross233ctl metrics -n 300          # Server metrics history

# Service control
cross233ctl service-enable --name ssh
cross233ctl service-disable --name ssh

# Client management
cross233ctl client-kick --name CLIENT_ID

# Configuration
cross233ctl config                  # View config
cross233ctl config-reload           # Reload config

# Agent / Automation modes
cross233ctl watch                   # Stream WebSocket events in real-time
cross233ctl agent -i 5              # Poll stats every 5s (JSON lines output)
cross233ctl stats --json            # JSON output for piping/jq

# Client lifecycle (local)
cross233ctl client-start --config client.toml
cross233ctl client-stop
cross233ctl client-status
cross233ctl client-logs
```

### Automation Examples

```bash
# Monitor with jq
cross233ctl stats --json | jq '.total_conns'

# Watch for high bandwidth
cross233ctl watch --json | jq -c 'select(.type=="Stats") | .data | select(.total_conns > 100)'

# Cron-style health check
*/1 * * * * cross233ctl health || echo "Server down!" | mail -s "cross233 alert" admin@example.com
```

### HTTP API Reference

All `/api/*` endpoints require authentication via:
1. **Bearer Token**: `Authorization: Bearer <api_token>` header
2. **Session Cookie**: After POST to `/api/login` with `{user, password}`
3. **Query Token**: `?token=<api_token>` (useful for WebSocket connections)

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/healthz` | GET | Health check (no auth) |
| `/api/login` | POST | Login with user/password, returns session cookie |
| `/api/stats` | GET | Server statistics |
| `/api/services` | GET | List all services |
| `/api/services/{name}` | GET | Service detail |
| `/api/services/{name}/metrics?limit=N` | GET | Service metrics history |
| `/api/services/{name}/toggle` | POST | Toggle service enabled/disabled (body: `{"enabled": true}`) |
| `/api/clients` | GET | List connected clients |
| `/api/clients/{id}/kick` | POST | Kick a client |
| `/api/logs` | GET | Recent logs |
| `/api/metrics?limit=N` | GET | Server metrics history |
| `/api/config` | GET | Server configuration (secrets redacted) |
| `/api/config/reload` | POST | Request config reload |
| `/api/ws` | WebSocket | Real-time event stream (ServiceUpdate, Stats, Log) |

Legacy endpoints `/api/v1/*` are preserved for backward compatibility.

## Security

- All control connections use **TLS 1.3** with auto-generated certificates
- HMAC-SHA256 challenge-response authentication
- Optional web dashboard basic authentication
- Optional API bearer token for automation/CLI access
- CIDR-based access control per service
- STCP/SUDP services require a secret key to access
- Certificate verification enabled by default (use `insecure = true` to skip for testing)

## Linux Server Deployment

For production Linux deployment, use the systemd installer:

```bash
sudo bash scripts/install-server.sh
```

This will:
1. Download the latest release binary
2. Generate a random auth key
3. Create systemd service with security hardening
4. Enable and start the service

```bash
# Check status
systemctl status cross233

# View logs
journalctl -u cross233 -f

# View auth key
sudo cat /var/lib/cross233/auth.key
```

## Comparison with frp

| Feature | cross233 | frp |
|---------|----------|-----|
| Language | Rust | Go |
| Binary size | ~8-12 MB | ~15-25 MB |
| Memory usage | Low | Moderate |
| TLS 1.3 | ✅ Built-in | ✅ |
| QCP (reliable UDP) | ✅ | ❌ (XTCP only) |
| Web dashboard | ✅ ECharts data大盘 | ✅ Basic |
| WebSocket real-time | ✅ Live event stream | ❌ |
| API Token auth | ✅ Bearer token | ❌ |
| Agent CLI (cross233ctl) | ✅ Full automation | ❌ (frpc only) |
| TOML config | ✅ Default | ❌ (INI/TOML limited) |
| JSON/YAML config | ✅ | ❌ |
| Bandwidth limiting | ✅ | ✅ |
| PROXY protocol | ✅ | ✅ |
| Health checks | ✅ | ✅ |
| HTTP vhost | ✅ | ✅ |
| STCP/SUDP | ✅ | ✅ |
| TCPMUX | ✅ | ✅ |
| Header manipulation | ✅ | ✅ |
| Hot reload | ✅ | ✅ |
| Compression | ✅ | ✅ |
| Connection groups | ✅ | ✅ |
| Access control (CIDR) | ✅ | ✅ |

## Project Structure

```
cross233/
├── cross233-protocol/    # Core protocol: message types, TLS, crypto
├── cross233-qcp/         # Reliable UDP transport (QCP)
├── cross233-server/      # Server implementation with embedded web UI
│   ├── src/
│   │   ├── server.rs     # Main server logic
│   │   ├── control.rs    # Control connection handling
│   │   ├── bandwidth.rs  # Token bucket rate limiting
│   │   ├── http_vhost.rs # HTTP virtual host proxy
│   │   ├── https_vhost.rs# HTTPS SNI routing
│   │   ├── service.rs    # Service registry
│   │   └── web/          # Web dashboard API + assets
│   └── webroot/          # Embedded web UI (auto-generated)
├── cross233-client/      # Client implementation
│   └── src/
│       ├── client.rs     # Main client logic
│       ├── tunnel.rs     # Tunnel connection management
│       ├── visitor.rs    # STCP visitor support
│       ├── health_check.rs
│       └── web.rs        # Local web dashboard
├── web/                  # React/Vite web UI source
│   └── src/
├── examples/             # Example configurations
├── scripts/              # Installation and management scripts
├── install.ps1           # Windows one-click installer
└── install.sh            # Linux/macOS one-click installer
```

## Building from Source

```bash
# Prerequisites
# - Rust (https://rustup.rs/)
# - Node.js 18+ (for web UI)
# - Perl (required by openssl-sys on some platforms)

# Clone and build
git clone https://github.com/neko233-com/cross233.git
cd cross233

# Build web UI
cd web && npm install && npm run build && cd ..

# Build release binaries
cargo build --release

# Run
./target/release/cross233-server -c examples/server.toml
./target/release/cross233-client -c examples/client.toml
```

## Environment Variables

| Variable | Description |
|----------|-------------|
| `RUST_LOG` | Log level (e.g., `info`, `debug,cross233_server=trace`) |
| `CROSS233_SERVER` | Server URL for ctl scripts (default: http://127.0.0.1:7711) |
| `CROSS233_TOKEN` | API bearer token for ctl scripts (recommended for automation) |
| `CROSS233_USER` / `CROSS233_PASSWORD` | Web credentials for ctl scripts |
| `CROSS233_INSECURE` | Skip TLS verification for ctl scripts (1 = yes) |

## License

MIT License - see [LICENSE](LICENSE) for details.

## Contributing

Contributions are welcome! Please feel free to submit issues and pull requests.
