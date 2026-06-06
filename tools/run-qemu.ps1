# Phase 0: proves the RISC-V virtual machine + firmware boot, before our
# kernel exists. Boots QEMU's built-in OpenSBI firmware.
# Phase 1 will extend this to load our kernel binary with `-kernel`.
# Exit QEMU with: Ctrl-A then X
$ErrorActionPreference = "Stop"
Write-Host "Booting QEMU RISC-V (OpenSBI firmware). Exit with Ctrl-A X." -ForegroundColor Cyan
qemu-system-riscv64 -machine virt -nographic -bios default
