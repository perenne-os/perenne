# Builds the workspace. Phase 0: host build.
# Usage: ./tools/build.ps1
$ErrorActionPreference = "Stop"
Write-Host "Building workspace (host)..." -ForegroundColor Cyan
cargo build
cargo test
Write-Host "Build + tests OK." -ForegroundColor Green
