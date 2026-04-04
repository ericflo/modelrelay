# ModelRelay — Install modelrelay-worker as a Windows Service
#
# Prerequisites:
#   - Download modelrelay-worker-windows-amd64.exe from a GitHub release
#   - Place it somewhere permanent (e.g. C:\ModelRelay\modelrelay-worker.exe)
#   - Run this script as Administrator
#
# Usage:
#   .\install-windows-service-worker.ps1
#
# The script installs a Windows Service named "ModelRelayWorker" that starts
# automatically on boot. After installation, configure your environment
# variables (PROXY_URL, WORKER_SECRET, BACKEND_URL, MODELS) and start the
# service.
#
# To run multiple workers on the same machine, copy this script, change
# $ServiceName to something unique (e.g. "ModelRelayWorkerGPU1"), and set
# different WORKER_NAME / BACKEND_URL / MODELS for each instance.

#Requires -RunAsAdministrator
Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

# ---------------------------------------------------------------------------
# Configuration — adjust these paths and arguments to match your setup.
# ---------------------------------------------------------------------------

# Path to the modelrelay-worker binary.
$BinaryPath = "C:\ModelRelay\modelrelay-worker.exe"

# Service display name and description. Change $ServiceName if you run
# multiple worker instances on the same machine.
$ServiceName = "ModelRelayWorker"
$DisplayName = "ModelRelay Worker"
$Description = "ModelRelay worker daemon — connects to the proxy and forwards requests to a local model server."

# Command-line arguments. Models are required; everything else can come from
# environment variables. Example:
#   $Arguments = "--models llama3-8b,codellama-13b --max-concurrent 2"
$Arguments = "--models llama3-8b"

# ---------------------------------------------------------------------------
# Install
# ---------------------------------------------------------------------------

if (-not (Test-Path $BinaryPath)) {
    Write-Error "Binary not found at $BinaryPath — download it from https://github.com/ericflo/modelrelay/releases"
}

if ($Arguments) {
    $BinPathValue = "`"$BinaryPath`" $Arguments"
} else {
    $BinPathValue = "`"$BinaryPath`""
}

Write-Host "Creating service '$ServiceName'..." -ForegroundColor Cyan

sc.exe create $ServiceName `
    binPath= $BinPathValue `
    start= auto `
    DisplayName= $DisplayName

if ($LASTEXITCODE -ne 0) {
    Write-Error "Failed to create service (exit code $LASTEXITCODE). Is it already installed?"
}

sc.exe description $ServiceName $Description | Out-Null

Write-Host ""
Write-Host "Service installed successfully." -ForegroundColor Green
Write-Host ""
Write-Host "Next steps:" -ForegroundColor Yellow
Write-Host "  1. Set required environment variables for the service:"
Write-Host ""
Write-Host "     # System-wide env vars (persist across reboots):"
Write-Host '     [Environment]::SetEnvironmentVariable("PROXY_URL", "http://your-proxy:8080", "Machine")'
Write-Host '     [Environment]::SetEnvironmentVariable("WORKER_SECRET", "your-secret-here", "Machine")'
Write-Host '     [Environment]::SetEnvironmentVariable("BACKEND_URL", "http://localhost:8000", "Machine")'
Write-Host '     [Environment]::SetEnvironmentVariable("WORKER_NAME", "gpu-box-1", "Machine")'
Write-Host ""
Write-Host "  2. Start the service:"
Write-Host "     Start-Service $ServiceName"
Write-Host ""
Write-Host "  3. Verify it is running:"
Write-Host "     Get-Service $ServiceName"
Write-Host ""
Write-Host "  4. Confirm it registered with the proxy:"
Write-Host '     curl http://your-proxy:8080/v1/models'
Write-Host ""

# ---------------------------------------------------------------------------
# Uninstall — uncomment the lines below to remove the service instead.
# ---------------------------------------------------------------------------
# Write-Host "Stopping and removing service '$ServiceName'..." -ForegroundColor Cyan
# Stop-Service $ServiceName -ErrorAction SilentlyContinue
# sc.exe delete $ServiceName
# Write-Host "Service removed." -ForegroundColor Green
