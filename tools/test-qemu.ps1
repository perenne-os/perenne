# Boot smoke test: cross-builds the kernel, boots it headless under QEMU
# (riscv64 virt + OpenSBI), captures the serial console, and asserts the
# Phase 2a milestones (greeting, survived breakpoint, >= 2 timer ticks),
# the Phase 2b milestones (Sv39 paging on, W^X blocking a rodata write, a
# frame alloc/free round-trip), and the Phase 3b-iii milestone — two isolated
# U-mode components communicate only through capability-checked synchronous
# IPC (the server blocks on recv; the client sends a value the server
# receives across address spaces and exits with), and a rogue task lacking
# the endpoint capability is rejected — and the Phase 3c milestone: an
# ML-KEM-768 post-quantum key-encapsulation round-trip runs on the bare
# kernel (shared secret agreed) — and the Phase 4a milestone: the kernel
# discovers RAM (192 MiB, proving it read the device tree, not a hardcoded
# 128) and the timer frequency from the device tree; the Phase 4b milestone:
# a direct ns16550 UART driver carries all console output; and the first
# user-space component (ADR 0007): an RTC driver running as an unprivileged
# component that owns the clock (its MMIO mapped only into it) and serves
# time-reads over a capability-checked endpoint — a rogue without the
# capability is refused; and the first cell of the self-healing knowledge
# organism (Phase 5a) — a deliberately faulty 'flaky' component crashes, is
# contained, and is deterministically diagnosed against the knowledge base
# (matched to KB-0005, with its fix playbook); and Phase 5b — the caged fix:
# a user-space healer, notified by the kernel of a contained crash, restarts a
# 'transient' component which then recovers and runs to completion, while the
# always-crashing 'flaky' is restarted only up to the bound and then flagged
# for triage; and a second user-space component (a virtio-rng driver) provides
# real hardware entropy — it draws two differing samples from the device and a
# kernel consumer seeds ML-KEM-768 with them, replacing Phase 3c's fixed seed.
# The rest of the system keeps running throughout.
# (Earlier IPC/isolation proofs are subsumed by this component demo, which
# still runs each task in its own address space.)
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
    "-m", "192M",
    "-global", "virtio-mmio.force-legacy=false",
    "-device", "virtio-rng-device",
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
    "ipc: 'rtc' blocks on recv",
    "sched: task 'rtc' exited \(code \d{15,}\)",
    "ipc: 'rogue' send rejected \(no capability\)",
    "entropy: virtio-rng live \(two draws differ\)",
    "pqc: ML-KEM-768 round-trip ok \(entropy-seeded\)",
    "sched: task 'flaky' killed by LoadPageFault",
    "heal: diagnosed KB-0005",
    "sched: task 'transient' killed by LoadPageFault",
    "heal: restarted 'transient' \(attempt 1\)",
    "sched: task 'transient' exited \(code 0\)",
    "heal: giving up on 'flaky' after 2 restarts \(flagged for triage\)",
    "console: ns16550a @ 0x10000000",
    "dt: 192 MiB RAM",
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
    Write-Host "BOOT TEST PASS: 2a + 2b + 3c ML-KEM + 4a device-tree (192 MiB) + 4b ns16550 console + the first user-space component (ADR 0007): an RTC driver serves the live clock over capability-checked IPC; a rogue is refused; Phase 5a self-healing: a contained component crash is deterministically diagnosed (KB-0005); and Phase 5b the caged fix: a user-space healer restarts a 'transient' component (it recovers) while 'flaky' is bounded and flagged; and a virtio-rng entropy component feeds real device entropy into ML-KEM (retiring the fixed seed) - all while the system keeps running." -ForegroundColor Green
    exit 0
} else {
    Write-Host "BOOT TEST FAIL: missing within 30s: $($missing -join ', '). Serial output:" -ForegroundColor Red
    Read-LogText $serialLog | Write-Host
    exit 1
}
