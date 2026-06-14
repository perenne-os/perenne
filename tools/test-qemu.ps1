# Boot smoke test: cross-builds the kernel, boots it headless under QEMU
# (riscv64 virt + OpenSBI), captures the serial console, and asserts the
# Phase 2a milestones (greeting, survived breakpoint, >= 2 timer ticks),
# the Phase 2b milestones (Sv39 paging on, W^X blocking a rodata write, a
# frame alloc/free round-trip), the Phase 2c milestones (three tasks
# round-robin cooperatively, then a non-yielding task is preempted), and
# the Phase 3a milestones (a U-mode task makes a print syscall, exits
# cleanly, and a second U-mode task touching kernel memory is contained).
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
    "-m", "128M",
    "-display", "none",
    "-serial", "file:$serialLog",
    "-bios", "default",
    "-kernel", $kernelElf
)
# Every pattern must appear in one boot. Patterns are regexes: "tick: 2(?!\d)"
# uses a negative lookahead to prevent matching "tick: 20" or "tick: 21" (>=2 ticks).
$mustMatch = @(
    "hello world",
    "trap: breakpoint",
    "survived breakpoint",
    "paging: sv39 on",
    "wx: rodata write blocked",
    "frames: alloc/free ok",
    "user: hello from user mode",
    "user: task exited with code 0",
    "user: task killed by load page fault",
    "(?s)sched: A step 0.*sched: B step 0.*sched: C step 0.*sched: A step 1.*sched: B step 1.*sched: C step 1",
    "preempted the hog",
    "tick: 2(?!\d)"
)
$missing = $mustMatch
try {
    $deadline = (Get-Date).AddSeconds(30)
    while ((Get-Date) -lt $deadline) {
        Start-Sleep -Milliseconds 500
        $text = Read-LogText $serialLog
        $missing = @($mustMatch | Where-Object { $text -notmatch $_ })
        if ($missing.Count -eq 0) { break }
    }
}
finally {
    if (-not $qemu.HasExited) { Stop-Process -Id $qemu.Id -Force }
}

if ($missing.Count -eq 0) {
    Write-Host "BOOT TEST PASS: 2a + 2b + 2c milestones plus the U-mode print round-trip, clean exit, and contained fault all observed." -ForegroundColor Green
    exit 0
} else {
    Write-Host "BOOT TEST FAIL: missing within 30s: $($missing -join ', '). Serial output:" -ForegroundColor Red
    Read-LogText $serialLog | Write-Host
    exit 1
}
