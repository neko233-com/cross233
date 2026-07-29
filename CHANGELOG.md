# Changelog

## v0.2.0

### Production safety

- Port `22` is unconditionally protected from tunnel registration.
- Privileged proxy ports are disabled by default.
- Server-owned ports and duplicate service ports cannot be claimed by clients.
- Proxy listeners honor `proxy_bind` and are released when clients disconnect.
- Configuration errors and listener bind failures now fail visibly instead of
  silently falling back or pretending a service is ready.
- Linux installation uses a dedicated unprivileged account, strict systemd
  isolation, resource limits, atomic replacement and automatic rollback.
- Server upgrades preserve the existing authentication key.

### Reliability

- Automatic ports are reused safely across reconnects without allocation or
  service-group leaks.
- Stale sessions cannot unregister a newer client session.
- One Ctrl+C cleanly stops the client instead of reconnecting once.
- Source-only server builds work even when the optional dashboard has not yet
  been built with Node.js.

### Static and Docker publishing

- Added built-in static-directory forwarding.
- Added an installable Docker/nginx service template and verification page.
- Added a complete `60080` Docker-to-public-server verification workflow.

### Distribution

- Release archives now include binaries, checksums, installers, maintenance
  scripts, example configurations, documentation and the Docker template.
- Added Linux ARMv7 release assets.
