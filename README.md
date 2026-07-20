# cross233

Lightweight TCP reverse tunnel. Compatible goal: frpc/frps use case, not wire-protocol compatible.

## Ports

| Port | Use |
| --- | --- |
| `7710` | TLS tunnel control |
| `7711` | Web management |
| `7712-7720` | Public TCP service ports |

## Server (Linux)

```bash
./cross233-server -password 'change-this-now'
```

First start creates `cross233-cert.pem` and `cross233-key.pem`. Copy certificate to clients and use `-ca`; this verifies server identity.

Web management: `https://SERVER:7711`. Login with password only. Default password is `root`; change it before exposing server. Accept or install generated certificate in browser.

### One-command install

Requires Linux with systemd. Downloads a manually built release, verifies SHA-256, installs `/usr/local/bin/cross233-server`, writes `/etc/cross233/server.env`, and enables `cross233.service`.

```bash
curl -fsSL https://raw.githubusercontent.com/neko233-com/cross233/main/scripts/install-server.sh | sudo env CROSS233_PASSWORD='change-this-now' bash
```

Use `CROSS233_VERSION=v0.1.0` to pin a release. Default installer password is `root`; only use it for local testing. Installer passwords support letters, digits, and `. _ ~ @ % + = , : ! -`.

## Client

```bash
./cross233-client \
  -server SERVER:7710 \
  -password 'change-this-now' \
  -ca cross233-cert.pem \
  -services 'web:7712:127.0.0.1:8080,ssh:7713:127.0.0.1:22'
```

Each service uses `name:public-port:local-host:local-port`. Public ports must be in `7712-7720`; each port can be claimed by one connected client. Use `-insecure` only for first-time local testing with self-signed certificates.

Optional client JSON config:

```json
{
  "server": "SERVER:7710",
  "password": "change-this-now",
  "ca_file": "cross233-cert.pem",
  "services": [
    {"name": "web", "remote_port": 7712, "local_addr": "127.0.0.1:8080"}
  ]
}
```

```bash
./cross233-client -config client.json
```

Start from [examples/client.json.example](examples/client.json.example).

## Manual packages

No GitHub Actions included. Run one command on a machine with Go 1.26:

```powershell
./build.ps1
```

```bash
./build.sh
```

Packages appear in `dist/`. Server builds Linux `amd64` and `arm64`; client builds Windows, macOS and Linux for `amd64` and `arm64`.

`checksums.txt` is emitted beside packages. Upload all `dist/` files as a GitHub Release manually; no CI or Actions are used.

## CLI maintenance

Both scripts are suitable for agent execution. API commands emit JSON. Management API uses same one password through `Authorization: Bearer PASSWORD`.

```bash
# Linux/macOS Bash: systemd lifecycle, server API, client daemon lifecycle
CROSS233_PASSWORD='change-this-now' CROSS233_INSECURE=1 ./scripts/cross233ctl.sh server-api-status
CROSS233_PASSWORD='change-this-now' CROSS233_INSECURE=1 ./scripts/cross233ctl.sh server-api-services
./scripts/cross233ctl.sh client-start --config client.json
./scripts/cross233ctl.sh client-status
./scripts/cross233ctl.sh client-stop
```

```powershell
# Windows PowerShell: server API plus client lifecycle
$env:CROSS233_PASSWORD = 'change-this-now'
./scripts/cross233ctl.ps1 -Command server-status -Insecure
./scripts/cross233ctl.ps1 -Command client-validate -Config client.json
./scripts/cross233ctl.ps1 -Command client-start -Config client.json
./scripts/cross233ctl.ps1 -Command client-status
./scripts/cross233ctl.ps1 -Command client-stop
```

Available Bash commands: `server-start`, `server-stop`, `server-restart`, `server-enable`, `server-disable`, `server-status`, `server-logs`, `server-health`, `server-api-status`, `server-api-services`, `server-api-logs`, `server-uninstall --yes`, `client-run`, `client-start`, `client-stop`, `client-restart`, `client-status`, `client-logs`.

API endpoints: `GET /healthz`, `GET /api/v1/status`, `GET /api/v1/services`, `GET /api/v1/logs`.
