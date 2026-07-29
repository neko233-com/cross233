param(
    [Parameter(Mandatory = $true)]
    [ValidatePattern('^v[0-9]+\.[0-9]+\.[0-9]+([-.][0-9A-Za-z.-]+)?$')]
    [string]$Version,

    [Parameter(Mandatory = $true)]
    [string]$Target,

    [Parameter(Mandatory = $true)]
    [string]$BinaryDir,

    [string]$OutDir = "dist\release"
)

$ErrorActionPreference = "Stop"
$RepoRoot = Split-Path -Parent $PSScriptRoot
$ResolvedBinaryDir = [IO.Path]::GetFullPath((Join-Path $RepoRoot $BinaryDir))
$ResolvedOutDir = [IO.Path]::GetFullPath((Join-Path $RepoRoot $OutDir))
$IsWindowsTarget = $Target -like "*-windows-*"
$ServerName = if ($IsWindowsTarget) { "cross233-server.exe" } else { "cross233-server" }
$ClientName = if ($IsWindowsTarget) { "cross233-client.exe" } else { "cross233-client" }

foreach ($Binary in @($ServerName, $ClientName)) {
    $BinaryPath = Join-Path $ResolvedBinaryDir $Binary
    if (!(Test-Path -LiteralPath $BinaryPath -PathType Leaf)) {
        throw "Required binary does not exist: $BinaryPath"
    }
}

New-Item -ItemType Directory -Force -Path $ResolvedOutDir | Out-Null
$StageRoot = Join-Path ([IO.Path]::GetTempPath()) "cross233-package-$([guid]::NewGuid().ToString('N'))"
$StageRoot = [IO.Path]::GetFullPath($StageRoot)
$ExpectedTempRoot = [IO.Path]::GetFullPath([IO.Path]::GetTempPath())
if (!$StageRoot.StartsWith($ExpectedTempRoot, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Refusing unsafe staging path: $StageRoot"
}

try {
    New-Item -ItemType Directory -Force -Path $StageRoot | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $StageRoot "scripts") | Out-Null
    New-Item -ItemType Directory -Force -Path (Join-Path $StageRoot "examples") | Out-Null

    Copy-Item -LiteralPath (Join-Path $ResolvedBinaryDir $ServerName) -Destination $StageRoot
    Copy-Item -LiteralPath (Join-Path $ResolvedBinaryDir $ClientName) -Destination $StageRoot
    foreach ($File in @("README.md", "CHANGELOG.md", "LICENSE", "install.sh", "install.ps1")) {
        Copy-Item -LiteralPath (Join-Path $RepoRoot $File) -Destination $StageRoot
    }
    foreach ($File in @("install-server.sh", "cross233ctl.sh", "cross233ctl.ps1")) {
        Copy-Item -LiteralPath (Join-Path $RepoRoot "scripts\$File") -Destination (Join-Path $StageRoot "scripts")
    }
    foreach ($File in @("server.toml", "client.toml")) {
        Copy-Item -LiteralPath (Join-Path $RepoRoot "examples\$File") -Destination (Join-Path $StageRoot "examples")
    }
    Copy-Item -LiteralPath (Join-Path $RepoRoot "examples\docker-static") -Destination (Join-Path $StageRoot "examples") -Recurse

    $BaseName = "cross233-$Version-$Target"
    if ($IsWindowsTarget) {
        $ArchivePath = Join-Path $ResolvedOutDir "$BaseName.zip"
        if (Test-Path -LiteralPath $ArchivePath) {
            Remove-Item -LiteralPath $ArchivePath -Force
        }
        Compress-Archive -Path (Join-Path $StageRoot "*") -DestinationPath $ArchivePath
    } else {
        $ArchivePath = Join-Path $ResolvedOutDir "$BaseName.tar.gz"
        if (Test-Path -LiteralPath $ArchivePath) {
            Remove-Item -LiteralPath $ArchivePath -Force
        }
        & tar.exe -C $StageRoot -czf $ArchivePath .
        if ($LASTEXITCODE -ne 0) {
            throw "tar failed with exit code $LASTEXITCODE"
        }
    }

    $Hash = (Get-FileHash -LiteralPath $ArchivePath -Algorithm SHA256).Hash.ToLowerInvariant()
    $HashPath = "$ArchivePath.sha256"
    "$Hash  $([IO.Path]::GetFileName($ArchivePath))" | Set-Content -LiteralPath $HashPath -Encoding ascii

    [pscustomobject]@{
        Archive = $ArchivePath
        Sha256 = $Hash
        Bytes = (Get-Item -LiteralPath $ArchivePath).Length
    }
} finally {
    if (Test-Path -LiteralPath $StageRoot) {
        Remove-Item -LiteralPath $StageRoot -Recurse -Force
    }
}
