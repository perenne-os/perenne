# Boots OUR kernel under QEMU (riscv64 `virt` machine, OpenSBI firmware).
# Phase 1: the kernel prints "hello world" on the serial console, then
# idles. Exit QEMU with: Ctrl-A then X
# Usage: ./tools/run-qemu.ps1
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot
Write-Host "Cross-building the kernel (riscv64)..." -ForegroundColor Cyan
cargo build --manifest-path "$repo/Cargo.toml" -p kernel --target riscv64gc-unknown-none-elf
Write-Host "Booting under QEMU (exit with Ctrl-A then X)..." -ForegroundColor Cyan
qemu-system-riscv64 -machine virt -nographic -bios default `
    -kernel "$repo/target/riscv64gc-unknown-none-elf/debug/kernel"
