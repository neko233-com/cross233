[CmdletBinding()]
param(
    [ValidateSet('help', 'server-health', 'server-status', 'server-services', 'server-logs', 'client-validate', 'client-run', 'client-start', 'client-stop', 'client-restart', 'client-status', 'client-logs')]
    [string]$Command = 'help',
    [string]$Url = $(if ($env:CROSS233_URL) { $env:CROSS233_URL } else { 'https://127.0.0.1:7711' }),
    [string]$Password = $env:CROSS233_PASSWORD,
    [string]$CAFile = $env:CROSS233_CA_FILE,
    [switch]$Insecure = ($env:CROSS233_INSECURE -eq '1'),
    [string]$Config,
    [string]$ClientBinary = (Join-Path $PSScriptRoot '..\cross233-client.exe'),
    [string]$StateDirectory = (Join-Path $HOME '.cross233')
)

$ErrorActionPreference = 'Stop'

function Write-Usage {
    @'
cross233ctl.ps1 commands:
  server-health | server-status | server-services | server-logs
  client-validate -Config FILE | client-run -Config FILE | client-start -Config FILE
  client-stop | client-restart -Config FILE | client-status | client-logs

Remote API: set CROSS233_PASSWORD and optionally CROSS233_URL, CROSS233_CA_FILE, CROSS233_INSECURE=1.
'@ | Write-Output
}

function Invoke-Cross233Api([string]$Path) {
    if ([string]::IsNullOrWhiteSpace($Password)) { throw 'Set CROSS233_PASSWORD for remote API commands.' }
    $arguments = @('-fsS', '-H', "Authorization: Bearer $Password")
    if ($CAFile) { $arguments += @('--cacert', $CAFile) }
    if ($Insecure) { $arguments += '-k' }
    $arguments += "$Url$Path"
    & curl.exe @arguments
    if ($LASTEXITCODE -ne 0) { throw "API request failed with exit code $LASTEXITCODE" }
}

function Get-PidFile { Join-Path $StateDirectory 'client.pid' }
function Get-LogFile { Join-Path $StateDirectory 'client.log' }
function Get-ErrorLogFile { Join-Path $StateDirectory 'client.err.log' }

function Require-Config {
    if ([string]::IsNullOrWhiteSpace($Config)) { throw 'Config is required.' }
    $resolved = Resolve-Path -LiteralPath $Config -ErrorAction Stop
    return $resolved.Path
}

function Test-ClientConfig([string]$Path) {
    $configData = Get-Content -Raw -LiteralPath $Path | ConvertFrom-Json
    if ([string]::IsNullOrWhiteSpace($configData.server) -or [string]::IsNullOrWhiteSpace($configData.password)) { throw 'Config needs server and password.' }
    if ($null -eq $configData.services -or $configData.services.Count -eq 0) { throw 'Config needs at least one service.' }
    foreach ($service in $configData.services) {
        if ([string]::IsNullOrWhiteSpace($service.name) -or $service.remote_port -lt 7712 -or $service.remote_port -gt 7720 -or [string]::IsNullOrWhiteSpace($service.local_addr)) { throw "Invalid service '$($service.name)'." }
    }
    [pscustomobject]@{ valid = $true; server = $configData.server; service_count = $configData.services.Count } | ConvertTo-Json -Compress
}

function Start-Client([string]$Path) {
    Test-ClientConfig $Path | Write-Output
    if (!(Test-Path -LiteralPath $ClientBinary)) { throw "Client binary not found: $ClientBinary" }
    New-Item -ItemType Directory -Force -Path $StateDirectory | Out-Null
    $pidFile = Get-PidFile
    if (Test-Path -LiteralPath $pidFile) {
        $oldPid = [int](Get-Content -Raw -LiteralPath $pidFile)
        if (Get-Process -Id $oldPid -ErrorAction SilentlyContinue) { throw "Client already running (pid $oldPid)." }
    }
    $process = Start-Process -FilePath $ClientBinary -ArgumentList @('-config', $Path) -PassThru -WindowStyle Hidden -RedirectStandardOutput (Get-LogFile) -RedirectStandardError (Get-ErrorLogFile)
    Set-Content -NoNewline -LiteralPath $pidFile -Value $process.Id
    [pscustomobject]@{ started = $true; pid = $process.Id } | ConvertTo-Json -Compress
}

function Stop-Client {
    $pidFile = Get-PidFile
    if (!(Test-Path -LiteralPath $pidFile)) { '{"running":false}' | Write-Output; return }
    $pid = [int](Get-Content -Raw -LiteralPath $pidFile)
    Stop-Process -Id $pid -Force -ErrorAction SilentlyContinue
    Remove-Item -Force -LiteralPath $pidFile
    [pscustomobject]@{ stopped = $true; pid = $pid } | ConvertTo-Json -Compress
}

function Get-ClientStatus {
    $pidFile = Get-PidFile
    if ((Test-Path -LiteralPath $pidFile) -and (Get-Process -Id ([int](Get-Content -Raw -LiteralPath $pidFile)) -ErrorAction SilentlyContinue)) {
        [pscustomobject]@{ running = $true; pid = [int](Get-Content -Raw -LiteralPath $pidFile) } | ConvertTo-Json -Compress
    } else { '{"running":false}' | Write-Output }
}

switch ($Command) {
    'help' { Write-Usage }
    'server-health' { Invoke-Cross233Api '/healthz' }
    'server-status' { Invoke-Cross233Api '/api/v1/status' }
    'server-services' { Invoke-Cross233Api '/api/v1/services' }
    'server-logs' { Invoke-Cross233Api '/api/v1/logs' }
    'client-validate' { Test-ClientConfig (Require-Config) }
    'client-run' { & $ClientBinary '-config' (Require-Config); exit $LASTEXITCODE }
    'client-start' { Start-Client (Require-Config) }
    'client-stop' { Stop-Client }
    'client-restart' { Stop-Client; Start-Client (Require-Config) }
    'client-status' { Get-ClientStatus }
    'client-logs' { Get-Content -LiteralPath (Get-LogFile), (Get-ErrorLogFile) -Tail 100 -ErrorAction SilentlyContinue }
}
