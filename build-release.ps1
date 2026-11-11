<#
.SYNOPSIS
  Builds the release client and copies it to dist\synctube.exe.
.EXAMPLE
  .\build-release.ps1 -ServerUrl wss://synctube-server-abcd.onrender.com/ws
#>
param(
    # Server URL prefilled on the connect screen. Defaults to the value in client\src\config.rs.
    [string]$ServerUrl = ""
)
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

if ($ServerUrl) {
    if ($ServerUrl -notmatch '^wss?://') { throw "ServerUrl must start with wss:// or ws://" }
    $env:SYNCTUBE_SERVER_URL = $ServerUrl
} else {
    Remove-Item Env:SYNCTUBE_SERVER_URL -ErrorAction SilentlyContinue
}

# cargo prints progress on stderr; don't let Windows PowerShell treat that as an error.
$ErrorActionPreference = "Continue"
cargo build --release --locked -p client --bin synctube
$ErrorActionPreference = "Stop"
if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

New-Item -ItemType Directory -Force dist | Out-Null
Copy-Item target\release\synctube.exe dist\synctube.exe -Force
$size = [math]::Round((Get-Item dist\synctube.exe).Length / 1MB, 1)
Write-Host "dist\synctube.exe ($size MB) is ready. Send just this file to your friends."
