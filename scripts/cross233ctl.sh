#!/usr/bin/env bash
# Linux/macOS maintenance CLI. Environment variables make it easy for agents to call.
set -Eeuo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
SERVER_URL="${CROSS233_URL:-https://127.0.0.1:7711}"
SERVER_PASSWORD="${CROSS233_PASSWORD:-}"
SERVER_CA_FILE="${CROSS233_CA_FILE:-}"
SERVER_INSECURE="${CROSS233_INSECURE:-0}"
CLIENT_BIN="${CROSS233_CLIENT_BIN:-$SCRIPT_DIR/../cross233-client}"
STATE_DIR="${CROSS233_STATE_DIR:-$HOME/.cross233}"

usage() {
  cat <<'EOF'
Usage: cross233ctl.sh <command> [options]

Server local:  server-start | server-stop | server-restart | server-enable | server-disable | server-status | server-logs | server-uninstall --yes
Server remote: server-health | server-api-status | server-api-services | server-api-logs
Client:         client-run --config FILE | client-start --config FILE | client-stop | client-restart --config FILE | client-status | client-logs

Remote API environment: CROSS233_URL, CROSS233_PASSWORD, CROSS233_CA_FILE, CROSS233_INSECURE=1.
EOF
}
fail() { printf 'cross233ctl: %s\n' "$*" >&2; exit 1; }
run_root() { if [ "$(id -u)" -eq 0 ]; then "$@"; elif command -v sudo >/dev/null 2>&1; then sudo "$@"; else fail "root or sudo required"; fi; }
api_get() {
  [ -n "$SERVER_PASSWORD" ] || fail "set CROSS233_PASSWORD for remote API commands"
  local args=(-fsS -H "Authorization: Bearer $SERVER_PASSWORD")
  [ -n "$SERVER_CA_FILE" ] && args+=(--cacert "$SERVER_CA_FILE")
  [ "$SERVER_INSECURE" = 1 ] && args+=(-k)
  curl "${args[@]}" "$SERVER_URL$1"
}
config_arg() {
  [ "${1:-}" = "--config" ] && [ -n "${2:-}" ] || fail "expected --config FILE"
  [ -f "$2" ] || fail "config not found: $2"
  printf '%s' "$2"
}
client_pid_file() { printf '%s/client.pid' "$STATE_DIR"; }
client_log_file() { printf '%s/client.log' "$STATE_DIR"; }
client_err_file() { printf '%s/client.err.log' "$STATE_DIR"; }
client_stop() {
  local pid_file; pid_file="$(client_pid_file)"
  [ -f "$pid_file" ] || { printf '{"running":false}\n'; return; }
  local pid; pid="$(cat "$pid_file")"
  if kill -0 "$pid" 2>/dev/null; then kill "$pid"; fi
  rm -f "$pid_file"
  printf '{"stopped":true,"pid":%s}\n' "$pid"
}
client_start() {
  local config; config="$(config_arg "$@")"
  mkdir -p "$STATE_DIR"
  local pid_file; pid_file="$(client_pid_file)"
  if [ -f "$pid_file" ] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then fail "client already running (pid $(cat "$pid_file"))"; fi
  [ -x "$CLIENT_BIN" ] || fail "client binary not executable: $CLIENT_BIN"
  nohup "$CLIENT_BIN" -config "$config" >"$(client_log_file)" 2>"$(client_err_file)" < /dev/null &
  local pid=$!; printf '%s\n' "$pid" > "$pid_file"
  printf '{"started":true,"pid":%s}\n' "$pid"
}
client_status() {
  local pid_file; pid_file="$(client_pid_file)"
  if [ -f "$pid_file" ] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then printf '{"running":true,"pid":%s}\n' "$(cat "$pid_file")"; else printf '{"running":false}\n'; fi
}

command="${1:-help}"; shift || true
case "$command" in
  help|-h|--help) usage ;;
  server-start) run_root systemctl start cross233 ;;
  server-stop) run_root systemctl stop cross233 ;;
  server-restart) run_root systemctl restart cross233 ;;
  server-enable) run_root systemctl enable cross233 ;;
  server-disable) run_root systemctl disable cross233 ;;
  server-status) systemctl show cross233 --no-page -p LoadState -p ActiveState -p SubState -p MainPID ;;
  server-logs) run_root journalctl -u cross233 --no-pager -n "${1:-100}" ;;
  server-health) api_get /healthz ;;
  server-api-status) api_get /api/v1/status ;;
  server-api-services) api_get /api/v1/services ;;
  server-api-logs) api_get /api/v1/logs ;;
  server-uninstall)
    [ "${1:-}" = "--yes" ] || fail "server uninstall requires --yes"
    run_root systemctl disable --now cross233 || true
    run_root rm -f /etc/systemd/system/cross233.service /usr/local/bin/cross233-server
    run_root rm -rf /etc/cross233 /var/lib/cross233
    run_root systemctl daemon-reload
    ;;
  client-run) config="$(config_arg "$@")"; exec "$CLIENT_BIN" -config "$config" ;;
  client-start) client_start "$@" ;;
  client-stop) client_stop ;;
  client-restart) client_stop; client_start "$@" ;;
  client-status) client_status ;;
  client-logs) tail -n "${1:-100}" "$(client_log_file)" "$(client_err_file)" ;;
  *) usage; exit 2 ;;
esac
