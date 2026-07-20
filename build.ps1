$ErrorActionPreference = 'Stop'
$root = Split-Path -Parent $MyInvocation.MyCommand.Path
$dist = Join-Path $root 'dist'
New-Item -ItemType Directory -Force -Path $dist | Out-Null

$targets = @(
  @{ OS = 'linux'; Arch = 'amd64'; Name = 'cross233-server' },
  @{ OS = 'linux'; Arch = 'arm64'; Name = 'cross233-server' },
  @{ OS = 'windows'; Arch = 'amd64'; Name = 'cross233-client'; Ext = '.exe' },
  @{ OS = 'windows'; Arch = 'arm64'; Name = 'cross233-client'; Ext = '.exe' },
  @{ OS = 'darwin'; Arch = 'amd64'; Name = 'cross233-client' },
  @{ OS = 'darwin'; Arch = 'arm64'; Name = 'cross233-client' },
  @{ OS = 'linux'; Arch = 'amd64'; Name = 'cross233-client' },
  @{ OS = 'linux'; Arch = 'arm64'; Name = 'cross233-client' }
)

foreach ($target in $targets) {
  $pkg = if ($target.Name -like '*server*') { './cross233-server' } else { './cross233-client' }
  $out = Join-Path $dist ("{0}-{1}-{2}{3}" -f $target.Name, $target.OS, $target.Arch, $target.Ext)
  Write-Host "Building $out"
  $env:CGO_ENABLED = '0'; $env:GOOS = $target.OS; $env:GOARCH = $target.Arch
  go build -trimpath -ldflags='-s -w' -o $out $pkg
}

Get-ChildItem -File $dist | Sort-Object Name | ForEach-Object {
  "{0}  {1}" -f (Get-FileHash -Algorithm SHA256 $_.FullName).Hash.ToLowerInvariant(), $_.Name
} | Set-Content -Encoding ascii (Join-Path $dist 'checksums.txt')
