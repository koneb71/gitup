<#
.SYNOPSIS
    Build Gitup and package it as a portable Windows zip.

.DESCRIPTION
    Produces a zip containing gitup.exe, the licence and a readme. The exe is
    self-contained apart from Git itself, so "install" means unzip it anywhere.

    There is no MSI. An installer wants a code-signing certificate to be worth
    having — an unsigned one trips SmartScreen exactly like a bare exe does,
    while adding a build dependency on WiX — so that is a decision for whoever
    ships releases, not something this script should assume.

.PARAMETER Target
    Rust target triple. Defaults to the host.

.EXAMPLE
    .\scripts\package-windows.ps1
#>
[CmdletBinding()]
param(
    [string]$Target = ""
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version Latest

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
Push-Location $root
try {
    $version = (Select-String -Path "Cargo.toml" -Pattern '^version = "(.*)"' |
        Select-Object -First 1).Matches[0].Groups[1].Value
    $targetDir = if ($env:CARGO_TARGET_DIR) { $env:CARGO_TARGET_DIR } else { "target" }

    Write-Host "==> Building release binary"
    $buildArgs = @("build", "--release", "--locked")
    if ($Target) { $buildArgs += @("--target", $Target) }
    & cargo @buildArgs
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed" }

    $exe = if ($Target) {
        Join-Path $targetDir "$Target\release\gitup.exe"
    } else {
        Join-Path $targetDir "release\gitup.exe"
    }
    if (-not (Test-Path $exe)) { throw "no executable at $exe" }

    # PROCESSOR_ARCHITECTURE says AMD64; the rest of the project and the
    # release artifacts say x86_64. Normalize so the names line up.
    $arch = if ($Target) {
        ($Target -split "-")[0]
    } else {
        switch ($env:PROCESSOR_ARCHITECTURE) {
            "AMD64" { "x86_64" }
            "ARM64" { "aarch64" }
            default { $env:PROCESSOR_ARCHITECTURE.ToLower() }
        }
    }
    $name = "gitup-$version-windows-$arch"
    $stage = Join-Path $targetDir $name

    Write-Host "==> Assembling $stage"
    if (Test-Path $stage) { Remove-Item -Recurse -Force $stage }
    New-Item -ItemType Directory -Path $stage | Out-Null
    Copy-Item $exe (Join-Path $stage "gitup.exe")
    Copy-Item "LICENSE" (Join-Path $stage "LICENSE.txt")
    Copy-Item "README.md" (Join-Path $stage "README.md")

    # Windows has no equivalent of a .desktop file for a portable app, so the
    # zip carries its own instructions rather than assuming they were read
    # somewhere else.
    @"
Gitup $version

Unzip anywhere and run gitup.exe. No installation is needed.

Gitup calls the real git for anything that touches the network, so Git for
Windows must be installed and on your PATH: https://git-scm.com/download/win
Everything else is built in.

The executable is not code-signed, so SmartScreen will warn the first time you
run it. "More info" then "Run anyway" gets past it.

Source and issues: https://github.com/koneb71/gitup
"@ | Set-Content -Path (Join-Path $stage "README-FIRST.txt") -Encoding UTF8

    $zip = Join-Path $targetDir "$name.zip"
    Write-Host "==> Creating $zip"
    if (Test-Path $zip) { Remove-Item -Force $zip }
    Compress-Archive -Path "$stage\*" -DestinationPath $zip

    $size = "{0:N1} MB" -f ((Get-Item $zip).Length / 1MB)
    Write-Host "==> Built $zip ($size)"
}
finally {
    Pop-Location
}
