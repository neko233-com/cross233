#!/usr/bin/env bash
# cross233ctl - cross233 control CLI for Linux/macOS
set -Eeuo pipefail

SCRIPT_DIR="$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)"
SERVER_URL="${CROSS233_SERVER:-${CROSS233_URL:-http://127.0.0.1:7711}}"
SERVER_TOKEN="${CROSS233_TOKEN:-${CROSS233_API_TOKEN:-${CROSS233_AUTH_KEY:-}}}"
SERVER_USER="${CROSS233_USER:-}"
SERVER_PASSWORD="${CROSS233_PASSWORD:-}"
SERVER_CA_FILE="${CROSS233_CA_FILE:-}"
SERVER_INSECURE="${CROSS233_INSECURE:-0}"
CLIENT_BIN="${CROSS233_CLIENT_BIN:-$SCRIPT_DIR/../cross233-client}"
STATE_DIR="${CROSS233_STATE_DIR:-$HOME/.cross233}"
JSON_OUTPUT=0
LIMIT=120
INTERVAL=2
NAME=""
TOGGLE_ENABLE=""

usage() {
  cat <<'EOF'
cross233ctl - cross233 control CLI

USAGE:
  cross233ctl.sh <command> [options]

SERVER COMMANDS (require CROSS233_TOKEN or CROSS233_USER+CROSS233_PASSWORD):
  health                       Check server health
  status | stats               Show server statistics
  services                     List all services
  service --name NAME          Show service detail
  service-metrics --name NAME [-n N]   Get service metrics history
  service-enable --name NAME   Enable a service
  service-disable --name NAME  Disable a service
  clients                      List connected clients
  client-kick --name ID        Kick a client
  logs [-n N]                  Show recent logs
  metrics [-n N]               Get server metrics history
  config                       Show server configuration
  config-reload                Reload server configuration
  watch [-i N]                 Stream real-time events via WebSocket (agent mode)
  agent [-i N]                 Agent mode: poll stats every N seconds (JSON lines)

CLIENT COMMANDS (local):
  client-run --config FILE        Run client in foreground
  client-start --config FILE      Start client as background daemon
  client-stop                     Stop background client
  client-restart --config FILE    Restart background client
  client-status                   Check client status
  client-logs [-n N]              View client logs

SYSTEM COMMANDS (Linux systemd):
  server-start | server-stop | server-restart
  server-enable | server-disable
  server-status | server-logs [-n N]

GLOBAL OPTIONS:
  --json              Output JSON (for automation/scripting)
  --name NAME         Resource name (service/client)
  -n N, --limit N     Number of history points (default: 120)
  -i N, --interval N  Polling interval seconds (default: 2)
  --server URL        Server URL (default http://127.0.0.1:7711)
  --token TOKEN       API bearer token
  --insecure          Skip TLS verification
  -h, --help          Show help

ENVIRONMENT VARIABLES:
  CROSS233_SERVER     Server URL
  CROSS233_TOKEN      API bearer token (recommended for automation)
  CROSS233_USER       Web username
  CROSS233_PASSWORD   Web password
  CROSS233_INSECURE=1 Skip TLS certificate check
EOF
}

fail() { printf 'cross233ctl: %s\n' "$*" >&2; exit 1; }

have_cmd() { command -v "$1" >/dev/null 2>&1; }

curl_base() {
  local args=(-fsS)
  if [ -n "$SERVER_CA_FILE" ]; then args+=(--cacert "$SERVER_CA_FILE"); fi
  if [ "$SERVER_INSECURE" = "1" ]; then args+=(-k); fi
  printf '%s ' "${args[@]}"
}

COOKIE_JAR=""
SESSION_FILE=""

cleanup() {
  if [ -n "$SESSION_FILE" ] && [ -f "$SESSION_FILE" ]; then rm -f "$SESSION_FILE"; fi
}
trap cleanup EXIT

ensure_auth() {
  if [ -n "$SERVER_TOKEN" ]; then return; fi
  if [ -n "$SESSION_FILE" ] && [ -f "$SESSION_FILE" ]; then return; fi
  if [ -z "$SERVER_USER" ] || [ -z "$SERVER_PASSWORD" ]; then
    fail "auth required: set CROSS233_TOKEN, or CROSS233_USER+CROSS233_PASSWORD"
  fi
  SESSION_FILE="$(mktemp)"
  local body="{\"user\":\"$SERVER_USER\",\"password\":\"$SERVER_PASSWORD\"}"
  local curl_args; curl_args="$(curl_base)"
  eval curl $curl_args -c "$SESSION_FILE" -H 'Content-Type: application/json' \
    -d "'$body'" "$SERVER_URL/api/login" >/dev/null || fail "login failed"
}

api_call() {
  local method="$1" path="$2" body="${3:-}"
  ensure_auth
  local curl_args; curl_args="$(curl_base)"
  local auth_args=()
  if [ -n "$SERVER_TOKEN" ]; then
    auth_args+=(-H "Authorization: Bearer $SERVER_TOKEN")
  elif [ -n "$SESSION_FILE" ]; then
    auth_args+=(-b "$SESSION_FILE")
  fi
  if [ "$method" = "POST" ]; then
    if [ -n "$body" ]; then
      eval curl $curl_args "${auth_args[@]}" -X POST -H 'Content-Type: application/json' \
        -d "'$body'" "$SERVER_URL$path"
    else
      eval curl $curl_args "${auth_args[@]}" -X POST "$SERVER_URL$path"
    fi
  else
    eval curl $curl_args "${auth_args[@]}" "$SERVER_URL$path"
  fi
}

api_get() { api_call GET "$1"; }
api_post() { api_call POST "$1" "$2"; }

format_bytes() {
  local b=$1
  if [ "$b" -lt 1024 ]; then echo "${b} B"; return; fi
  awk -v b="$b" 'BEGIN{
    split("KB MB GB TB PB", u);
    for(i=1;b>=1024;i++)b/=1024;
    printf "%.2f %s", b, u[i-1];
  }'
}

# Client management helpers
client_pid_file() { printf '%s/client.pid' "$STATE_DIR"; }
client_log_file() { printf '%s/client.log' "$STATE_DIR"; }
client_err_file() { printf '%s/client.err.log' "$STATE_DIR"; }

client_stop() {
  local pid_file; pid_file="$(client_pid_file)"
  if [ ! -f "$pid_file" ]; then echo '{"running":false}'; return; fi
  local pid; pid="$(cat "$pid_file")"
  if kill -0 "$pid" 2>/dev/null; then kill "$pid" || true; sleep 0.3; kill -9 "$pid" 2>/dev/null || true; fi
  rm -f "$pid_file"
  printf '{"stopped":true,"pid":%s}\n' "$pid"
}

client_start() {
  local config="$1"
  [ -f "$config" ] || fail "config not found: $config"
  mkdir -p "$STATE_DIR"
  local pid_file; pid_file="$(client_pid_file)"
  if [ -f "$pid_file" ] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
    fail "client already running (pid $(cat "$pid_file"))"
  fi
  [ -x "$CLIENT_BIN" ] || fail "client binary not executable: $CLIENT_BIN"
  nohup "$CLIENT_BIN" -c "$config" >"$(client_log_file)" 2>"$(client_err_file)" < /dev/null &
  local pid=$!; printf '%s\n' "$pid" > "$pid_file"
  printf '{"started":true,"pid":%s}\n' "$pid"
}

client_status() {
  local pid_file; pid_file="$(client_pid_file)"
  if [ -f "$pid_file" ] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
    printf '{"running":true,"pid":%s}\n' "$(cat "$pid_file")"
  else
    echo '{"running":false}'
  fi
}

# Parse options
COMMAND=""
CLIENT_CONFIG=""
shift_args=()

while [ $# -gt 0 ]; do
  case "$1" in
    health|status|stats|services|service|service-metrics|service-enable|service-disable|clients|client-kick|logs|metrics|config|config-reload|watch|agent|client-run|client-start|client-stop|client-restart|client-status|client-logs|server-start|server-stop|server-restart|server-enable|server-disable|server-logs|server-uninstall|help)
      COMMAND="$1"; shift
      ;;
    --json) JSON_OUTPUT=1; shift ;;
    --server) SERVER_URL="$2"; shift 2 ;;
    --token) SERVER_TOKEN="$2"; shift 2 ;;
    --user) SERVER_USER="$2"; shift 2 ;;
    --password) SERVER_PASSWORD="$2"; shift 2 ;;
    --insecure|-k) SERVER_INSECURE=1; shift ;;
    --name) NAME="$2"; shift 2 ;;
    --config) CLIENT_CONFIG="$2"; shift 2 ;;
    -n|--limit) LIMIT="$2"; shift 2 ;;
    -i|--interval) INTERVAL="$2"; shift 2 ;;
    -h|--help) COMMAND="help"; shift ;;
    *) shift_args+=("$1"); shift ;;
  esac
done

if [ -z "$COMMAND" ]; then COMMAND="help"; fi

run_root() {
  if [ "$(id -u)" -eq 0 ]; then "$@"
  elif have_cmd sudo; then sudo "$@"
  else fail "root or sudo required"; fi
}

case "$COMMAND" in
  help|-h) usage ;;

  health) api_get /healthz ;;

  status|stats)
    if [ "$JSON_OUTPUT" = "1" ]; then api_get /api/stats; else
      s=$(api_get /api/stats)
      echo "=== Server Status ==="
      echo "  Services:    $(echo "$s" | sed -n 's/.*"total_services":\([0-9]*\).*/\1/p')"
      echo "  Clients:     $(echo "$s" | sed -n 's/.*"total_clients":\([0-9]*\).*/\1/p')"
      echo "  Connections: $(echo "$s" | sed -n 's/.*"total_conns":\([0-9]*\).*/\1/p')"
      tx=$(echo "$s" | sed -n 's/.*"total_tx":\([0-9]*\).*/\1/p')
      rx=$(echo "$s" | sed -n 's/.*"total_rx":\([0-9]*\).*/\1/p')
      echo "  Total TX:    $(format_bytes "${tx:-0}")"
      echo "  Total RX:    $(format_bytes "${rx:-0}")"
    fi
    ;;

  services) api_get /api/services ;;

  service)
    [ -n "$NAME" ] || fail "--name NAME required"
    api_get "/api/services/$(python3 -c "import urllib.parse;print(urllib.parse.quote('$NAME'))" 2>/dev/null || echo "$NAME")"
    ;;

  service-metrics)
    [ -n "$NAME" ] || fail "--name NAME required"
    encoded=$(python3 -c "import urllib.parse;print(urllib.parse.quote('$NAME'))" 2>/dev/null || echo "$NAME")
    api_get "/api/services/$encoded/metrics?limit=$LIMIT"
    ;;

  service-enable)
    [ -n "$NAME" ] || fail "--name NAME required"
    encoded=$(python3 -c "import urllib.parse;print(urllib.parse.quote('$NAME'))" 2>/dev/null || echo "$NAME")
    api_post "/api/services/$encoded/toggle" '{"enabled":true}'
    ;;

  service-disable)
    [ -n "$NAME" ] || fail "--name NAME required"
    encoded=$(python3 -c "import urllib.parse;print(urllib.parse.quote('$NAME'))" 2>/dev/null || echo "$NAME")
    api_post "/api/services/$encoded/toggle" '{"enabled":false}'
    ;;

  clients) api_get /api/clients ;;

  client-kick)
    [ -n "$NAME" ] || fail "--name CLIENT_ID required"
    encoded=$(python3 -c "import urllib.parse;print(urllib.parse.quote('$NAME'))" 2>/dev/null || echo "$NAME")
    api_post "/api/clients/$encoded/kick"
    ;;

  logs) api_get "/api/logs?limit=$LIMIT" ;;

  metrics) api_get "/api/metrics?limit=$LIMIT" ;;

  config) api_get /api/config ;;

  config-reload) api_post /api/config/reload ;;

  watch)
    ensure_auth
    ws_url=$(echo "$SERVER_URL" | sed 's|^http|ws|')/api/ws
    echo "Streaming events from $ws_url (Ctrl+C to stop)..." >&2
    auth_header=""
    if [ -n "$SERVER_TOKEN" ]; then auth_header="Authorization: Bearer $SERVER_TOKEN"; fi
    if have_cmd websocat; then
      if [ -n "$auth_header" ]; then websocat -H="$auth_header" "$ws_url"
      else websocat "$ws_url"; fi
    elif have_cmd wscat; then
      if [ -n "$auth_header" ]; then wscat -c "$ws_url" -H "$auth_header"
      else wscat -c "$ws_url"; fi
    elif have_cmd python3; then
      python3 -c "
import asyncio, json, ssl, sys
try:
    import websockets
except ImportError:
    print('websocat or python3 websockets required for watch', file=sys.stderr); sys.exit(1)
async def run():
    headers = {}
    token = '$SERVER_TOKEN'
    if token: headers['Authorization'] = f'Bearer {token}'
    ssl_ctx = ssl.create_default_context()
    if '$SERVER_INSECURE' == '1': ssl_ctx.check_hostname = False; ssl_ctx.verify_mode = ssl.CERT_NONE
    url = '$ws_url'
    kw = {'extra_headers': headers}
    if url.startswith('wss'): kw['ssl'] = ssl_ctx
    async with websockets.connect(url, **kw) as ws:
        async for msg in ws:
            print(msg, flush=True)
asyncio.run(run())
"
    else
      fail "websocat, wscat, or python3 with websockets is required for watch"
    fi
    ;;

  agent)
    ensure_auth
    echo "Agent mode: polling every ${INTERVAL}s (Ctrl+C to stop)..." >&2
    while true; do
      ts=$(date +%s)
      if data=$(api_get /api/stats 2>/dev/null); then
        printf '{"ts":%s,"data":%s}\n' "$ts" "$data"
      else
        printf '{"ts":%s,"error":"request failed"}\n' "$ts"
      fi
      sleep "$INTERVAL"
    done
    ;;

  client-run)
    [ -n "$CLIENT_CONFIG" ] || fail "--config FILE required"
    exec "$CLIENT_BIN" -c "$CLIENT_CONFIG"
    ;;
  client-start)
    [ -n "$CLIENT_CONFIG" ] || fail "--config FILE required"
    client_start "$CLIENT_CONFIG"
    ;;
  client-stop) client_stop ;;
  client-restart)
    [ -n "$CLIENT_CONFIG" ] || fail "--config FILE required"
    client_stop; sleep 0.5; client_start "$CLIENT_CONFIG"
    ;;
  client-status) client_status ;;
  client-logs)
    n="${LIMIT}"
    tail -n "$n" "$(client_log_file)" "$(client_err_file)" 2>/dev/null || true
    ;;

  server-start) run_root systemctl start cross233 ;;
  server-stop) run_root systemctl stop cross233 ;;
  server-restart) run_root systemctl restart cross233 ;;
  server-enable) run_root systemctl enable cross233 ;;
  server-disable) run_root systemctl disable cross233 ;;
  server-status) systemctl show cross233 --no-page -p LoadState -p ActiveState -p SubState -p MainPID ;;
  server-logs) run_root journalctl -u cross233 --no-pager -n "${LIMIT}" ;;
  server-uninstall)
    fail "server-uninstall is intentionally removed in this version; do it manually."
    ;;

  *) usage; exit 2 ;;
esac
