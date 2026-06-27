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
# time-reads over a capability-checked endpoint via call/reply — a real server
# that loops recv->read->reply, returning the live clock to the caller (the
# client exits with the time it received). A 'deferrer' server holds TWO calls
# in flight (one one-shot reply capability per caller) and replies OUT OF ORDER
# (B before A) — dclientA/dclientB exit with their own distinct reply values.
# A rogue without the
# capability is refused; and the first cell of the self-healing knowledge
# organism (Phase 5a) — a deliberately faulty 'flaky' component crashes, is
# contained, and is deterministically diagnosed against the knowledge base
# (matched to KB-0005, with its fix playbook); and Phase 5b — the caged fix:
# a user-space healer, notified by the kernel of a contained crash, restarts a
# 'transient' component which then recovers and runs to completion, while the
# always-crashing 'flaky' is restarted only up to the bound and then flagged
# for triage; and a second user-space component (a virtio-rng driver) provides
# real hardware entropy that seeds a reseedable kernel entropy pool (a ChaCha20
# CSPRNG): the pool is seeded from the device, serves distinct draws on demand,
# is reseeded with fresh device entropy, and keys the ML-KEM-768 round-trip —
# replacing Phase 3c's one-shot fixed seed; and a U-mode component ('rnguser')
# draws from that pool through a capability-gated 'getrandom' syscall — refused
# when it asks without the capability, served (32 bytes) with it; and the
# entropy component is INTERRUPT-DRIVEN — it blocks on its device's IRQ via the
# PLIC (capability-gated 'wait_irq') instead of polling, and the kernel's
# external-interrupt handler wakes it; and a user-space virtio-blk driver
# (Phase 6a) reads and writes a disk sector — it writes a pattern to sector 0,
# reads it back, and verifies the round-trip, interrupt-driven.
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

# A filesystem disk image (Phase 6b) for the kernel to read a named file from,
# built by the host `mkfs` tool from the shared on-disk format.
cargo build --manifest-path "$repo/Cargo.toml" -p mkfs
$mkfs = Join-Path $repo "target/debug/mkfs.exe"
if (-not (Test-Path $mkfs)) {
    Write-Host "BOOT TEST FAIL: mkfs not produced at $mkfs" -ForegroundColor Red
    exit 1
}
$diskImg = Join-Path ([System.IO.Path]::GetTempPath()) "kernel-qemu-disk.img"
& $mkfs $diskImg
if ($LASTEXITCODE -ne 0) {
    Write-Host "BOOT TEST FAIL: mkfs failed to build the image" -ForegroundColor Red
    exit 1
}

# Read a file QEMU may still hold open for writing (share ReadWrite).
function Read-LogText($path) {
    if (-not (Test-Path $path)) { return "" }
    $fs = [System.IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
    try { (New-Object System.IO.StreamReader($fs)).ReadToEnd() } finally { $fs.Dispose() }
}

# Run one QEMU boot against the shared $diskImg; return the patterns from
# $mustMatch not yet seen in $serialLog within 30s. Both boots use identical
# QEMU args and the SAME writable disk image, so boot 1's write-back persists
# into boot 2 (the cross-boot proof). $diskImg/$kernelElf are script-scoped.
function Invoke-Boot([string[]]$mustMatch, [string]$serialLog) {
    if (Test-Path $serialLog) { Remove-Item $serialLog -Force }
    $qemu = Start-Process qemu-system-riscv64 -PassThru -NoNewWindow -ArgumentList @(
        "-machine", "virt",
        "-m", "192M",
        "-global", "virtio-mmio.force-legacy=false",
        "-device", "virtio-rng-device",
        "-drive", "file=$diskImg,if=none,format=raw,id=blk0",
        "-device", "virtio-blk-device,drive=blk0",
        "-display", "none",
        "-serial", "file:$serialLog",
        "-bios", "default",
        "-kernel", $kernelElf
    )
    $missing = $mustMatch
    try {
        # 60s: boot 1's write-back adds ~5 blk ops, each gated to ~one-per-tick
        # by the blk IRQ-recovery constraint (see learning note 0023/0024), and
        # ticks run a few seconds each in this debug build. The loop exits as
        # soon as every pattern is seen, so this only bounds the failure case.
        $deadline = (Get-Date).AddSeconds(60)
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
    return $missing
}

# Boot 1: the fresh image (mkfs above) holds only KB-0005. Every existing
# milestone must appear AND the self-healer must RECORD the novel
# illegal-instruction crash (KB-0006) to the writable disk. Patterns are
# regexes: "tick: 2(?!\d)" uses a negative lookahead to avoid matching
# "tick: 20"/"tick: 21" (>=2 ticks).
$mustMatch1 = @(
    "hello world",
    "trap: breakpoint",
    "survived breakpoint",
    "paging: sv39 on",
    "wx: rodata write blocked",
    "frames: alloc/free ok",
    "ipc: 'rtc' blocks on recv",
    "sched: task 'client' exited \(code \d{15,}\)",
    "sched: task 'dclientA' exited \(code 417\)",
    "sched: task 'dclientB' exited \(code 433\)",
    "ipc: 'rogue' send rejected \(no capability\)",
    "irq: external IRQ \d+ woke",
    "heal: loaded \d+ KB entr",
    "heal: diagnosed KB-0005 \(User-space component terminated by a fatal fault\)",
    "playbook: Restart the component, up to a bounded number of retries",
    "entropy: pool seeded from virtio-rng",
    "entropy: pool serves on demand \(draws differ\)",
    "entropy: pool reseeded from virtio-rng",
    "pqc: ML-KEM-768 round-trip ok \(pool-seeded\)",
    "rng: request rejected \(no capability\)",
    "rng: served 32 bytes to 'rnguser'",
    "sched: task 'rnguser' exited \(code 0\)",
    "sched: task 'flaky' killed by LoadPageFault",
    "sched: task 'transient' killed by LoadPageFault",
    "heal: restarted 'transient' \(attempt 1\)",
    "sched: task 'transient' exited \(code 0\)",
    "heal: giving up on 'flaky' after 2 restarts \(flagged for triage\)",
    "sched: task 'novel' killed by IllegalInstruction",
    "heal: recorded KB-0006 \(illegal-instruction\) to disk",
    "console: ns16550a @ 0x10000000",
    "dt: 192 MiB RAM",
    "tick: 2(?!\d)"
)
$missing1 = Invoke-Boot $mustMatch1 $serialLog

# Boot 2: the SAME image (now KB-0005 + the KB-0006 written in boot 1), NOT
# rebuilt. The loader reads both entries, and the novel illegal-instruction
# crash is now diagnosed against the entry the organism wrote itself last boot
# — the cross-boot learning proof.
$serialLog2 = Join-Path ([System.IO.Path]::GetTempPath()) "kernel-qemu-serial-2.log"
$mustMatch2 = @(
    "heal: loaded 2 KB entries from disk",
    "heal: diagnosed KB-0006 \(Observed fault: illegal-instruction \(auto-recorded\)\) -> playbook: Restart the component, up to a bounded number of retries"
)
$missing2 = Invoke-Boot $mustMatch2 $serialLog2

if ($missing1.Count -eq 0 -and $missing2.Count -eq 0) {
    Write-Host "BOOT TEST PASS: 2a + 2b + 3c ML-KEM + 4a device-tree (192 MiB) + 4b ns16550 console + the first user-space component (ADR 0007): an RTC driver serves the live clock over capability-checked IPC; a rogue is refused; Phase 5a self-healing: a contained component crash is deterministically diagnosed (KB-0005); and Phase 5b the caged fix: a user-space healer restarts a 'transient' component (it recovers) while 'flaky' is bounded and flagged; and a virtio-rng entropy component feeds real device entropy into a reseedable kernel entropy pool (ChaCha20 CSPRNG) that keys ML-KEM on demand (retiring the fixed seed) and is drawn on by a U-mode component via a capability-gated getrandom syscall; the entropy component is interrupt-driven (it blocks on its device IRQ via the PLIC, woken by the kernel's external-interrupt handler); and a deferrer server holds two calls in flight and replies out of order via one-shot reply capabilities; and Phase 6c the living knowledge base: the self-healer reads knowledge-base/entries/*.md off the disk at boot, parses each entry, and diagnoses the contained crash against the on-disk KB-0005 (selected by its match-cause token); and Phase 7 write-back: on the first boot the self-healer meets a novel illegal-instruction crash with no KB entry and RECORDS a new entry (KB-0006) to the writable disk, then on a second boot of the SAME image it loads 2 entries and DIAGNOSES KB-0006 - the organism learned across reboots - all while the system keeps running." -ForegroundColor Green
    exit 0
} else {
    if ($missing1.Count -ne 0) {
        Write-Host "BOOT TEST FAIL (boot 1): missing within 60s: $($missing1 -join ', '). Serial output:" -ForegroundColor Red
        Read-LogText $serialLog | Write-Host
    }
    if ($missing2.Count -ne 0) {
        Write-Host "BOOT TEST FAIL (boot 2, cross-boot write-back): missing within 60s: $($missing2 -join ', '). Serial output:" -ForegroundColor Red
        Read-LogText $serialLog2 | Write-Host
    }
    exit 1
}
