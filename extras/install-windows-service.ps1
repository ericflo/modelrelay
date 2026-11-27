# ModelRelay — Install modelrelay-server as a Windows Service
#
# Prerequisites:
#   - Download modelrelay-server-windows-amd64.exe from a GitHub release
#   - Place it somewhere permanent (e.g. C:\ModelRelay\modelrelay-server.exe)
#   - Install NSSM (https://nssm.cc) or WinSW — modelrelay-server is a standard
#     console executable rather than a native Win32 SCM service binary, so a
#     service wrapper such as NSSM is required to avoid SCM Error 1053 on start.
#   - Run this script as Administrator
#
# Usage:
#   .\install-windows-service.ps1
#
# The script installs a Windows Service named "ModelRelayServer" via NSSM that
# starts automatically on boot.

#Requires -RunAsAdministrator
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ---------------------------------------------------------------------------
# Configuration — adjust these paths and arguments to match your setup.
# ---------------------------------------------------------------------------

# Path to the modelrelay-server binary.
$BinaryPath = "C:\ModelRelay\modelrelay-server.exe"

# Service display name and description.
$ServiceName = "ModelRelayServer"
$DisplayName = "ModelRelay Server"
$Description = "ModelRelay central proxy server — routes LLM requests to remote workers."

# Command-line arguments passed to the binary. Environment variables are read
# from the system environment, so you typically only need arguments here if you
# want to override defaults. Example:
#   $Arguments = "--listen 0.0.0.0:8080"
$Arguments = ""

# ---------------------------------------------------------------------------
# Install
# ---------------------------------------------------------------------------

if (-not (Test-Path $BinaryPath)) {
    Write-Error "Binary not found at $BinaryPath — download it from https://github.com/ericflo/modelrelay/releases"
}

$Nssm = Get-Command nssm -ErrorAction SilentlyContinue
if (-not $Nssm) {
    Write-Error "NSSM is required to run modelrelay-server as a Windows Service (avoids SCM Error 1053). Install via 'winget install NSSM.NSSM' or https://nssm.cc and re-run."
}

Write-Host "Creating service '$ServiceName' via NSSM..." -ForegroundColor Cyan

if ($Arguments) {
    & $Nssm.Source install $ServiceName $BinaryPath $Arguments
} else {
    & $Nssm.Source install $ServiceName $BinaryPath
}

if ($LASTEXITCODE -ne 0) {
    Write-Error "Failed to create service (exit code $LASTEXITCODE). Is it already installed?"
}

& $Nssm.Source set $ServiceName DisplayName $DisplayName | Out-Null
& $Nssm.Source set $ServiceName Description $Description | Out-Null
& $Nssm.Source set $ServiceName Start SERVICE_AUTO_START | Out-Null

Write-Host ""
Write-Host "Service installed successfully." -ForegroundColor Green
Write-Host ""
Write-Host "Next steps:" -ForegroundColor Yellow
Write-Host "  1. Set required environment variables for the service:"
Write-Host ""
Write-Host "     # System-wide env vars (persist across reboots):"
Write-Host '     [Environment]::SetEnvironmentVariable("WORKER_SECRET", "your-secret-here", "Machine")'
Write-Host '     [Environment]::SetEnvironmentVariable("LISTEN_ADDR", "0.0.0.0:8080", "Machine")'
Write-Host ""
Write-Host "  2. Start the service:"
Write-Host "     Start-Service $ServiceName"
Write-Host ""
Write-Host "  3. Verify it is running:"
Write-Host "     Get-Service $ServiceName"
Write-Host ""

# ---------------------------------------------------------------------------
# Uninstall — uncomment the lines below to remove the service instead.
# ---------------------------------------------------------------------------
# Write-Host "Stopping and removing service '$ServiceName'..." -ForegroundColor Cyan
# Stop-Service $ServiceName -ErrorAction SilentlyContinue
# sc.exe delete $ServiceName
# Write-Host "Service removed." -ForegroundColor Green
