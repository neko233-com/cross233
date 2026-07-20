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

## Manual packages

No GitHub Actions included. Run one command on a machine with Go 1.26:

```powershell
./build.ps1
```

```bash
./build.sh
```

Packages appear in `dist/`. Server builds Linux `amd64` and `arm64`; client builds Windows, macOS and Linux for `amd64` and `arm64`.
