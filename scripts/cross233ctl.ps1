[CmdletBinding()]
param(
    [ValidateSet('help','health','status','stats','services','service','service-metrics','service-toggle','clients','client-kick','logs','metrics','config','config-reload','watch','agent','client-validate','client-run','client-start','client-stop','client-restart','client-status','client-logs')]
    [string]$Command = 'help',
    [string]$Server = $(if ($env:CROSS233_SERVER) { $env:CROSS233_SERVER } else { 'http://127.0.0.1:7711' }),
    [string]$Token = $(if ($env:CROSS233_TOKEN) { $env:CROSS233_TOKEN } else { $env:CROSS233_API_TOKEN }),
    [string]$User = $env:CROSS233_USER,
    [string]$Password = $env:CROSS233_PASSWORD,
    [switch]$Insecure = ($env:CROSS233_INSECURE -eq '1'),
    [switch]$Json,
    [string]$Name,
    [switch]$Enable,
    [switch]$Disable,
    [int]$Limit = 120,
    [int]$Interval = 2,
    [string]$Config,
    [string]$ClientBinary = (Join-Path $PSScriptRoot '..\cross233-client.exe'),
    [string]$StateDirectory = (Join-Path $HOME '.cross233')
)

$ErrorActionPreference = 'Stop'
$script:SessionCookie = $null

function Write-Usage {
    @'
cross233ctl - cross233 control CLI

USAGE:
  cross233ctl.ps1 <command> [options]

SERVER COMMANDS (require CROSS233_TOKEN or CROSS233_USER/CROSS233_PASSWORD):
  health                 Check server health
  status | stats         Show server statistics
  services               List all services
  service -Name NAME     Show service detail
  service-metrics -Name NAME [-Limit N]  Get service metrics history
  service-toggle -Name NAME -Enable|-Disable  Toggle service state
  clients                List connected clients
  client-kick -Name ID   Kick a client by ID
  logs [-Limit N]        Show recent logs
  metrics [-Limit N]     Get server metrics history
  config                 Show server configuration
  config-reload          Reload server configuration
  watch [-Interval N]    Stream real-time events via WebSocket (agent mode)
  agent [-Interval N]    Agent mode: poll stats every N seconds

CLIENT COMMANDS (local client management):
  client-validate -Config FILE   Validate client config
  client-run -Config FILE        Run client in foreground
  client-start -Config FILE      Start client as background process
  client-stop                    Stop background client
  client-restart -Config FILE    Restart background client
  client-status                  Check client process status
  client-logs                    View client logs

ENVIRONMENT VARIABLES:
  CROSS233_SERVER       Server URL (default: http://127.0.0.1:7711)
  CROSS233_TOKEN        API bearer token (recommended for automation)
  CROSS233_USER         Web username (for session login)
  CROSS233_PASSWORD     Web password
  CROSS233_INSECURE=1   Skip TLS certificate validation

OPTIONS:
  -Json                 Output raw JSON (for scripting/automation)
  -Limit N              Number of history points (default: 120)
  -Interval N           Polling interval in seconds (default: 2)
'@ | Write-Output
}

function Ensure-Session {
    if ($script:SessionCookie) { return }
    if (-not [string]::IsNullOrWhiteSpace($Token)) { return }
    if ([string]::IsNullOrWhiteSpace($User) -or [string]::IsNullOrWhiteSpace($Password)) {
        throw 'Authentication required. Set CROSS233_TOKEN, or CROSS233_USER+CROSS233_PASSWORD.'
    }
    $body = @{ user = $User; password = $Password } | ConvertTo-Json
    try {
        $resp = Invoke-WebRequest -Uri "$Server/api/login" -Method POST -Body $body -ContentType 'application/json' -UseBasicParsing -SessionVariable sess -SkipCertificateCheck:$Insecure
        $script:SessionCookie = $sess
    } catch {
        throw "Login failed: $_"
    }
}

function Invoke-Api {
    param([string]$Path, [string]$Method = 'GET', [string]$Body = $null)
    Ensure-Session
    $url = "$Server$Path"
    $headers = @{}
    if (-not [string]::IsNullOrWhiteSpace($Token)) {
        $headers['Authorization'] = "Bearer $Token"
    }
    $params = @{
        Uri = $url
        Method = $Method
        UseBasicParsing = $true
        SkipCertificateCheck = $Insecure
        Headers = $headers
    }
    if ($Body) {
        $params['Body'] = $Body
        $params['ContentType'] = 'application/json'
    }
    if ($script:SessionCookie) {
        $params['WebSession'] = $script:SessionCookie
    }
    try {
        $resp = Invoke-WebRequest @params
        return $resp.Content
    } catch {
        $status = 0
        if ($_.Exception.Response) { $status = [int]$_.Exception.Response.StatusCode }
        if ($status -eq 401) { throw 'Authentication failed (401). Check token or credentials.' }
        throw "API request to $Path failed: $_"
    }
}

function Format-Bytes([long]$Bytes) {
    if ($Bytes -lt 1024) { return "$Bytes B" }
    $units = @('KB','MB','GB','TB')
    $val = $Bytes / 1024.0
    foreach ($u in $units) {
        if ($val -lt 1024) { return ("{0:N2} {1}" -f $val, $u) }
        $val /= 1024.0
    }
    return ("{0:N2} PB" -f $val)
}

function Format-Rate([long]$Bps) {
    $bits = $Bps * 8
    if ($bits -lt 1000) { return "$bits bps" }
    $units = @('Kbps','Mbps','Gbps','Tbps')
    $val = $bits / 1000.0
    foreach ($u in $units) {
        if ($val -lt 1000) { return ("{0:N1} {1}" -f $val, $u) }
        $val /= 1000.0
    }
    return ("{0:N1} Pbps" -f $val)
}

function Get-PidFile { Join-Path $StateDirectory 'client.pid' }
function Get-LogFile { Join-Path $StateDirectory 'client.log' }
function Get-ErrorLogFile { Join-Path $StateDirectory 'client.err.log' }

function Require-Config {
    if ([string]::IsNullOrWhiteSpace($Config)) { throw 'Config is required. Use -Config FILE.' }
    return (Resolve-Path -LiteralPath $Config -ErrorAction Stop).Path
}

function Test-ClientConfig([string]$Path) {
    $ext = [System.IO.Path]::GetExtension($Path).ToLower()
    switch ($ext) {
        '.toml' {
            $content = Get-Content -Raw -LiteralPath $Path
            if ($content -notmatch 'server\s*=' -or $content -notmatch 'auth_key\s*=') { throw 'Config needs server and auth_key.' }
        }
        '.json' {
            $cfg = Get-Content -Raw -LiteralPath $Path | ConvertFrom-Json
            if ([string]::IsNullOrWhiteSpace($cfg.server) -or [string]::IsNullOrWhiteSpace($cfg.auth_key)) { throw 'Config needs server and auth_key.' }
            if ($null -eq $cfg.services -or $cfg.services.Count -eq 0) { throw 'Config needs at least one service.' }
        }
        default { throw "Unsupported config format: $ext" }
    }
    if ($Json) { [pscustomobject]@{ valid = $true; config = $Path } | ConvertTo-Json -Compress }
    else { Write-Host "Config valid: $Path" -ForegroundColor Green }
}

function Start-Client([string]$Path) {
    Test-ClientConfig $Path | Out-Null
    if (!(Test-Path -LiteralPath $ClientBinary)) { throw "Client binary not found: $ClientBinary" }
    New-Item -ItemType Directory -Force -Path $StateDirectory | Out-Null
    $pidFile = Get-PidFile
    if (Test-Path -LiteralPath $pidFile) {
        $oldPid = [int](Get-Content -Raw -LiteralPath $pidFile)
        if (Get-Process -Id $oldPid -ErrorAction SilentlyContinue) { throw "Client already running (pid $oldPid)." }
    }
    $p = Start-Process -FilePath $ClientBinary -ArgumentList @('-c', $Path) -PassThru -WindowStyle Hidden -RedirectStandardOutput (Get-LogFile) -RedirectStandardError (Get-ErrorLogFile)
    Set-Content -NoNewline -LiteralPath $pidFile -Value $p.Id
    if ($Json) { [pscustomobject]@{ started = $true; pid = $p.Id } | ConvertTo-Json -Compress }
    else { Write-Host "Client started (pid $($p.Id))" -ForegroundColor Green }
}

function Stop-Client {
    $pidFile = Get-PidFile
    if (!(Test-Path -LiteralPath $pidFile)) {
        if ($Json) { '{"running":false}' } else { Write-Host 'Client not running' -ForegroundColor Yellow }
        return
    }
    $pid = [int](Get-Content -Raw -LiteralPath $pidFile)
    Stop-Process -Id $pid -Force -ErrorAction SilentlyContinue
    Remove-Item -Force -LiteralPath $pidFile
    if ($Json) { [pscustomobject]@{ stopped = $true; pid = $pid } | ConvertTo-Json -Compress }
    else { Write-Host "Client stopped (pid $pid)" -ForegroundColor Yellow }
}

function Get-ClientStatus {
    $pidFile = Get-PidFile
    $running = $false; $pid = 0
    if (Test-Path -LiteralPath $pidFile) {
        $pid = [int](Get-Content -Raw -LiteralPath $pidFile)
        if (Get-Process -Id $pid -ErrorAction SilentlyContinue) { $running = $true }
    }
    if ($Json) { [pscustomobject]@{ running = $running; pid = $pid } | ConvertTo-Json -Compress }
    elseif ($running) { Write-Host "Client running (pid $pid)" -ForegroundColor Green }
    else { Write-Host 'Client not running' -ForegroundColor Yellow }
}

function Cmd-Health {
    $t = Invoke-Api '/healthz'
    if ($Json) { $t } else { Write-Host "Server healthy: $t" -ForegroundColor Green }
}

function Cmd-Stats {
    $t = Invoke-Api '/api/stats'
    if ($Json) { $t; return }
    $s = $t | ConvertFrom-Json
    Write-Host "=== Server Status ===" -ForegroundColor Cyan
    Write-Host ("  Services:       {0}" -f $s.total_services)
    Write-Host ("  Clients:        {0}" -f $s.total_clients)
    Write-Host ("  Connections:    {0}" -f $s.total_conns)
    Write-Host ("  Total TX:       {0}" -f (Format-Bytes $s.total_tx))
    Write-Host ("  Total RX:       {0}" -f (Format-Bytes $s.total_rx))
}

function Cmd-Services {
    $t = Invoke-Api '/api/services'
    if ($Json) { $t; return }
    $d = $t | ConvertFrom-Json
    Write-Host "=== Services ($($d.services.Count)) ===" -ForegroundColor Cyan
    foreach ($s in $d.services) {
        $st = if (-not $s.enabled) { 'OFF' } elseif ($s.healthy) { 'OK ' } else { 'BAD' }
        $color = if ($s.enabled -and $s.healthy) { 'Green' } elseif (-not $s.enabled) { 'DarkGray' } else { 'Yellow' }
        $ep = if ($s.subdomain) { "*.$($s.subdomain)" } elseif ($s.host) { $s.host } else { ":$($s.remote_port)" }
        $bw = if ($s.bandwidthLimit) { " [lim $(Format-Rate ([long]($s.bandwidthLimit * 1000 / 8)))]" } else { '' }
        Write-Host ("  [{0}] {1,-20} {2,-8} {3,-22} c={4,-3} tx={5,-10} rx={6,-10}{7}" -f $st, $s.name, $s.ty, $ep, $s.current_conns, (Format-Bytes $s.traffic_tx), (Format-Bytes $s.traffic_rx), $bw) -ForegroundColor $color
    }
}

function Cmd-Service {
    if ([string]::IsNullOrWhiteSpace($Name)) { throw '-Name required.' }
    $t = Invoke-Api "/api/services/$([Uri]::EscapeDataString($Name))"
    if ($Json) { $t } else { Write-Host "=== Service: $Name ===" -ForegroundColor Cyan; ($t | ConvertFrom-Json) | ConvertTo-Json -Depth 5 }
}

function Cmd-ServiceToggle {
    if ([string]::IsNullOrWhiteSpace($Name)) { throw '-Name required.' }
    if (-not $Enable -and -not $Disable) { throw 'Specify -Enable or -Disable.' }
    $body = @{ enabled = [bool]$Enable.IsPresent } | ConvertTo-Json
    $t = Invoke-Api "/api/services/$([Uri]::EscapeDataString($Name))/toggle" 'POST' $body
    if ($Json) { $t } else { Write-Host "Service '$Name' $(if($Enable){'enabled'}else{'disabled'})" -ForegroundColor Green }
}

function Cmd-ServiceMetrics {
    if ([string]::IsNullOrWhiteSpace($Name)) { throw '-Name required.' }
    $t = Invoke-Api "/api/services/$([Uri]::EscapeDataString($Name))/metrics?limit=$Limit"
    if ($Json) { $t; return }
    $d = $t | ConvertFrom-Json
    Write-Host "=== Metrics for $Name ($($d.metrics.Count) points) ===" -ForegroundColor Cyan
    if ($d.metrics.Count -gt 0) {
        $last = $d.metrics[-1]
        Write-Host ("  Conns:    {0}" -f $last.conns)
        Write-Host ("  TX rate:  {0}" -f (Format-Rate $last.bw_tx))
        Write-Host ("  RX rate:  {0}" -f (Format-Rate $last.bw_rx))
        Write-Host ("  TX total: {0}" -f (Format-Bytes $last.tx))
        Write-Host ("  RX total: {0}" -f (Format-Bytes $last.rx))
    }
}

function Cmd-Clients {
    $t = Invoke-Api '/api/clients'
    if ($Json) { $t; return }
    $d = $t | ConvertFrom-Json
    Write-Host "=== Clients ($($d.clients.Count)) ===" -ForegroundColor Cyan
    foreach ($c in $d.clients) {
        Write-Host ("  {0}  services={1}  conns={2}" -f $c.id, $c.service_count, $c.active_conns) -ForegroundColor Green
        foreach ($s in $c.services_detail) {
            $st = if ($s.healthy) { 'OK' } else { 'BAD' }
            Write-Host ("    [{0}] {1,-20} {2,-8} :{3,-5} c={4}" -f $st, $s.name, $s.type, $s.remote_port, $s.conns)
        }
    }
}

function Cmd-ClientKick {
    if ([string]::IsNullOrWhiteSpace($Name)) { throw '-Name (client ID) required.' }
    $t = Invoke-Api "/api/clients/$([Uri]::EscapeDataString($Name))/kick" 'POST'
    if ($Json) { $t } else { Write-Host "Client '$Name' kicked" -ForegroundColor Yellow }
}

function Cmd-Logs {
    $t = Invoke-Api '/api/logs'
    if ($Json) { $t; return }
    $d = $t | ConvertFrom-Json
    Write-Host "=== Recent Logs ===" -ForegroundColor Cyan
    foreach ($l in $d.logs | Select-Object -Last 50) {
        $ts = [DateTimeOffset]::FromUnixTimeSeconds($l.timestamp).LocalDateTime.ToString('HH:mm:ss')
        $color = switch ($l.level.ToUpper()) {
            'ERROR' { 'Red' }
            'WARN' { 'Yellow' }
            default { 'Gray' }
        }
        Write-Host "[$ts] " -NoNewline
        Write-Host ("{0,-5} " -f $l.level.ToUpper()) -ForegroundColor $color -NoNewline
        Write-Host $l.message
    }
}

function Cmd-Metrics {
    $t = Invoke-Api "/api/metrics?limit=$Limit"
    if ($Json) { $t; return }
    $d = $t | ConvertFrom-Json
    Write-Host "=== Server Metrics ($($d.metrics.Count) points) ===" -ForegroundColor Cyan
    if ($d.metrics.Count -gt 0) {
        $last = $d.metrics[-1]
        Write-Host ("  Active services: {0}" -f $last.active_services)
        Write-Host ("  Active clients:  {0}" -f $last.active_clients)
        Write-Host ("  Connections:     {0}" -f $last.total_conns)
        Write-Host ("  TX bandwidth:    {0}" -f (Format-Rate $last.bandwidth_tx))
        Write-Host ("  RX bandwidth:    {0}" -f (Format-Rate $last.bandwidth_rx))
        Write-Host ("  Total TX:        {0}" -f (Format-Bytes $last.total_tx))
        Write-Host ("  Total RX:        {0}" -f (Format-Bytes $last.total_rx))
    }
}

function Cmd-Config {
    $t = Invoke-Api '/api/config'
    if ($Json) { $t } else { Write-Host "=== Server Config ===" -ForegroundColor Cyan; ($t | ConvertFrom-Json) | ConvertTo-Json -Depth 5 }
}

function Cmd-ConfigReload {
    $t = Invoke-Api '/api/config/reload' 'POST'
    if ($Json) { $t } else { Write-Host 'Config reload requested' -ForegroundColor Green }
}

function Cmd-Watch {
    Ensure-Session
    $wsUrl = ($Server -replace '^http', 'ws') + '/api/ws'
    if (-not $Json) { Write-Host "Streaming events from $wsUrl (Ctrl+C to stop)..." -ForegroundColor Cyan }
    $ws = New-Object System.Net.WebSockets.ClientWebSocket
    if (-not [string]::IsNullOrWhiteSpace($Token)) {
        $ws.Options.SetRequestHeader('Authorization', "Bearer $Token")
    }
    $ws.Options.RemoteCertificateValidationCallback = { $true }
    $uri = [Uri]$wsUrl
    $ct = [Threading.CancellationToken]::None
    try {
        $ws.ConnectAsync($uri, $ct).GetAwaiter().GetResult()
        $buffer = New-Object byte[] 16384
        while ($ws.State -eq [System.Net.WebSockets.WebSocketState]::Open) {
            $ms = New-Object IO.MemoryStream
            do {
                $result = $ws.ReceiveAsync($buffer, $ct).GetAwaiter().GetResult()
                if ($result.MessageType -eq [System.Net.WebSockets.WebSocketMessageType]::Close) { break }
                $ms.Write($buffer, 0, $result.Count)
            } while (-not $result.EndOfMessage)
            if ($result.MessageType -eq [System.Net.WebSockets.WebSocketMessageType]::Close) { break }
            $text = [Text.Encoding]::UTF8.GetString($ms.ToArray())
            if ($Json) { $text } else {
                try {
                    $ev = $text | ConvertFrom-Json
                    $ts = Get-Date -Format 'HH:mm:ss'
                    switch ($ev.type) {
                        'ServiceUpdate' { Write-Host "[$ts] Services: $($ev.data.Count)" -ForegroundColor Cyan }
                        'Stats' {
                            $s = $ev.data
                            Write-Host "[$ts] svcs=$($s.total_services) clients=$($s.total_clients) conns=$($s.total_conns) tx=$(Format-Bytes $s.total_tx) rx=$(Format-Bytes $s.total_rx)" -ForegroundColor Gray
                        }
                        'Log' {
                            $l = $ev.data
                            $c = switch ($l.level.ToUpper()) { 'ERROR' { 'Red' } 'WARN' { 'Yellow' } default { 'DarkGray' } }
                            Write-Host "[$ts] " -NoNewline; Write-Host ("{0,-5}" -f $l.level.ToUpper()) -ForegroundColor $c -NoNewline; Write-Host " $($l.message)"
                        }
                    }
                } catch { Write-Host $text }
            }
        }
    } catch { Write-Host "Watch error: $_" -ForegroundColor Red }
    finally { $ws.Dispose() }
}

function Cmd-Agent {
    if (-not $Json) { Write-Host "Agent mode: polling every ${Interval}s (Ctrl+C to stop)..." -ForegroundColor Cyan }
    while ($true) {
        try {
            $t = Invoke-Api '/api/stats'
            if ($Json) {
                @{ ts = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds(); data = ($t | ConvertFrom-Json) } | ConvertTo-Json -Compress
            } else {
                $s = $t | ConvertFrom-Json
                Write-Host "[$(Get-Date -Format 'HH:mm:ss')] svcs=$($s.total_services) clients=$($s.total_clients) conns=$($s.total_conns) tx=$(Format-Bytes $s.total_tx) rx=$(Format-Bytes $s.total_rx)"
            }
        } catch {
            if ($Json) { @{ ts = [DateTimeOffset]::UtcNow.ToUnixTimeSeconds(); error = $_.Exception.Message } | ConvertTo-Json -Compress }
            else { Write-Host "[$(Get-Date -Format 'HH:mm:ss')] ERR: $_" -ForegroundColor Red }
        }
        Start-Sleep -Seconds $Interval
    }
}

switch ($Command) {
    'help' { Write-Usage }
    'health' { Cmd-Health }
    'status' { Cmd-Stats }
    'stats' { Cmd-Stats }
    'services' { Cmd-Services }
    'service' { Cmd-Service }
    'service-metrics' { Cmd-ServiceMetrics }
    'service-toggle' { Cmd-ServiceToggle }
    'clients' { Cmd-Clients }
    'client-kick' { Cmd-ClientKick }
    'logs' { Cmd-Logs }
    'metrics' { Cmd-Metrics }
    'config' { Cmd-Config }
    'config-reload' { Cmd-ConfigReload }
    'watch' { Cmd-Watch }
    'agent' { Cmd-Agent }
    'client-validate' { Test-ClientConfig (Require-Config) }
    'client-run' { & $ClientBinary '-c' (Require-Config); exit $LASTEXITCODE }
    'client-start' { Start-Client (Require-Config) }
    'client-stop' { Stop-Client }
    'client-restart' { Stop-Client; Start-Client (Require-Config) }
    'client-status' { Get-ClientStatus }
    'client-logs' { Get-Content -LiteralPath (Get-LogFile), (Get-ErrorLogFile) -Tail 100 -ErrorAction SilentlyContinue }
}
