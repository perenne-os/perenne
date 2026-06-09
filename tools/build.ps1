# Builds the workspace (host) and cross-builds the kernel (riscv64).
# Usage: ./tools/build.ps1
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
Write-Host "Building workspace (host)..." -ForegroundColor Cyan
cargo build --manifest-path "$repo/Cargo.toml" --workspace
cargo test --manifest-path "$repo/Cargo.toml" --workspace
Write-Host "Cross-building kernel (riscv64gc-unknown-none-elf)..." -ForegroundColor Cyan
cargo build --manifest-path "$repo/Cargo.toml" -p kernel --target riscv64gc-unknown-none-elf
Write-Host "Build + tests OK." -ForegroundColor Green
