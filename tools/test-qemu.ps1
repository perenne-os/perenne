# Boot smoke test: cross-builds the kernel, boots it headless under QEMU
# (riscv64 virt + OpenSBI), captures the serial console to a temp file,
# and asserts the Phase 1 greeting ("hello world") appears.
# Usage: ./tools/test-qemu.ps1     (exit code 0 = pass, 1 = fail)
$ErrorActionPreference = "Stop"
$repo = Split-Path -Parent $PSScriptRoot

cargo build --manifest-path "$repo/Cargo.toml" -p kernel --target riscv64gc-unknown-none-elf
$kernelElf = Join-Path $repo "target/riscv64gc-unknown-none-elf/debug/kernel"
if (-not (Test-Path $kernelElf)) {
    Write-Host "BOOT TEST FAIL: kernel binary not produced at $kernelElf" -ForegroundColor Red
    exit 1
}

$serialLog = Join-Path ([System.IO.Path]::GetTempPath()) "kernel-qemu-serial.log"
if (Test-Path $serialLog) { Remove-Item $serialLog -Force }

# Read a file QEMU may still hold open for writing (share ReadWrite).
function Read-LogText($path) {
    if (-not (Test-Path $path)) { return "" }
    $fs = [System.IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
    try { (New-Object System.IO.StreamReader($fs)).ReadToEnd() } finally { $fs.Dispose() }
}

$qemu = Start-Process qemu-system-riscv64 -PassThru -NoNewWindow -ArgumentList @(
    "-machine", "virt",
    "-display", "none",
    "-serial", "file:$serialLog",
    "-bios", "default",
    "-kernel", $kernelElf
)
$found = $false
try {
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 500
        if ((Read-LogText $serialLog) -match "hello world") { $found = $true; break }
    }
}
finally {
    if (-not $qemu.HasExited) { Stop-Process -Id $qemu.Id -Force }
}

if ($found) {
    Write-Host "BOOT TEST PASS: kernel printed 'hello world'." -ForegroundColor Green
    exit 0
} else {
    Write-Host "BOOT TEST FAIL: greeting not found within 30s. Serial output:" -ForegroundColor Red
    Read-LogText $serialLog | Write-Host
    exit 1
}
