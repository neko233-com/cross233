# Cross233 One-Click Install Script for Windows
# Usage: irm https://raw.githubusercontent.com/neko233-com/cross233/main/install.ps1 | iex
# Or: powershell -ExecutionPolicy Bypass -File install.ps1

param(
    [string]$InstallDir = "$env:USERPROFILE\.cross233",
    [string]$Version = "latest",
    [ValidateSet("server","client","both")]
    [string]$Component = "both"
)

$ErrorActionPreference = "Stop"

$RepoUrl = "https://github.com/neko233-com/cross233"
$ReleasesUrl = "https://github.com/neko233-com/cross233/releases"

Write-Host "=== Cross233 Installer ===" -ForegroundColor Cyan
Write-Host "Install dir: $InstallDir"
Write-Host "Component:   $Component"
Write-Host ""

# Check architecture
$Arch = if ([Environment]::Is64BitOperatingSystem) { "x86_64" } else { "i686" }
$Os = "windows"
$Triple = "${Arch}-pc-windows-msvc"

# Create install directory
if (!(Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

# Check if Rust is installed (for building from source)
$HasRust = $false
try {
    $null = Get-Command cargo -ErrorAction Stop
    $HasRust = $true
    Write-Host "[+] Rust toolchain detected" -ForegroundColor Green
} catch {
    Write-Host "[!] Rust not found. Will download prebuilt binary." -ForegroundColor Yellow
}

function Download-File {
    param([string]$Url, [string]$OutFile)
    Write-Host "    Downloading $Url ..."
    try {
        Invoke-WebRequest -Uri $Url -OutFile $OutFile -UseBasicParsing
    } catch {
        Write-Host "    Failed to download: $_" -ForegroundColor Red
        return $false
    }
    return $true
}

function Install-Prebuilt {
    param([string]$BinName)

    if ($Version -eq "latest") {
        $Tag = (Invoke-RestMethod -Uri "https://api.github.com/repos/neko233-com/cross233/releases/latest" -UseBasicParsing).tag_name
    } else {
        $Tag = $Version
    }

    $ArchiveName = "cross233-${Tag}-${Triple}.zip"
    $DownloadUrl = "${ReleasesUrl}/download/${Tag}/${ArchiveName}"
    $TempDir = Join-Path $env:TEMP "cross233-install-$(Get-Random)"
    New-Item -ItemType Directory -Path $TempDir -Force | Out-Null
    $ArchivePath = Join-Path $TempDir $ArchiveName

    $ok = Download-File -Url $DownloadUrl -OutFile $ArchivePath
    if (!$ok) {
        Write-Host "[!] Prebuilt binary not found, falling back to source build" -ForegroundColor Yellow
        Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
        return $false
    }

    Expand-Archive -Path $ArchivePath -DestinationPath $TempDir -Force
    $ExeName = if ($BinName -eq "server") { "cross233-server.exe" } else { "cross233-client.exe" }
    $SrcPath = Join-Path $TempDir $ExeName
    $DstPath = Join-Path $InstallDir $ExeName
    Copy-Item -Path $SrcPath -Destination $DstPath -Force
    Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
    Write-Host "[+] Installed $ExeName to $DstPath" -ForegroundColor Green
    return $true
}

function Build-FromSource {
    param([string]$BinName)

    if (!$HasRust) {
        Write-Host "[!] Rust is required to build from source." -ForegroundColor Red
        Write-Host "    Install Rust from https://rustup.rs/ and retry."
        return $false
    }

    $TempDir = Join-Path $env:TEMP "cross233-src-$(Get-Random)"
    Write-Host "[*] Cloning source to $TempDir ..."

    if (Test-Path (Join-Path $PSScriptRoot "Cargo.toml")) {
        Copy-Item -Recurse -Force $PSScriptRoot $TempDir
    } else {
        git clone --depth 1 $RepoUrl $TempDir 2>&1 | Out-Null
    }

    Push-Location $TempDir
    try {
        Write-Host "[*] Building cross233-$BinName ..." -ForegroundColor Yellow
        cargo build --release -p "cross233-$BinName" 2>&1
        if ($LASTEXITCODE -ne 0) {
            Write-Host "[!] Build failed" -ForegroundColor Red
            return $false
        }
        $ExeName = "cross233-$BinName.exe"
        $SrcPath = Join-Path $TempDir "target\release\$ExeName"
        $DstPath = Join-Path $InstallDir $ExeName
        Copy-Item -Path $SrcPath -Destination $DstPath -Force
        Write-Host "[+] Built and installed $ExeName to $DstPath" -ForegroundColor Green
    } finally {
        Pop-Location
        Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
    }
    return $true
}

function Install-Binary {
    param([string]$BinName)

    Write-Host "[*] Installing cross233-$BinName ..." -ForegroundColor Yellow

    # Try prebuilt first, then source build
    $installed = Install-Prebuilt -BinName $BinName
    if (!$installed) {
        $installed = Build-FromSource -BinName $BinName
    }

    if ($installed) {
        $ExePath = Join-Path $InstallDir "cross233-$BinName.exe"
        # Create default config
        $ConfigName = if ($BinName -eq "server") { "server.toml" } else { "client.toml" }
        $ConfigPath = Join-Path $InstallDir $ConfigName
        if (!(Test-Path $ConfigPath)) {
            $samplePath = Join-Path $PSScriptRoot "examples\$ConfigName"
            if (Test-Path $samplePath) {
                Copy-Item $samplePath $ConfigPath
            }
        }
    }
}

if ($Component -eq "both" -or $Component -eq "server") {
    Install-Binary -BinName "server"
}
if ($Component -eq "both" -or $Component -eq "client") {
    Install-Binary -BinName "client"
}

# Add to PATH
$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
    $env:Path += ";$InstallDir"
    Write-Host "[+] Added $InstallDir to user PATH" -ForegroundColor Green
}

Write-Host ""
Write-Host "=== Installation Complete ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "Quick start:"
Write-Host "  Server: cross233-server.exe -c $InstallDir\server.toml"
Write-Host "  Client: cross233-client.exe -c $InstallDir\client.toml"
Write-Host ""
Write-Host "Web admin panel (server): http://127.0.0.1:7711"
Write-Host "Web admin panel (client): http://127.0.0.1:7721"
Write-Host ""
