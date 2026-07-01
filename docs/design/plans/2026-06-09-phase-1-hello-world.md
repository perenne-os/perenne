# Phase 1 — Hello World Kernel Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Boot our own freestanding `no_std` kernel on the QEMU riscv64 `virt` machine (OpenSBI firmware) and print "hello world" over the serial console — the Phase 1 milestone from the roadmap.

**Architecture:** The `kernel` package gains a bare-metal binary (`src/main.rs` + boot assembly + linker script) linked at `0x80200000`, where OpenSBI jumps after firmware init. Console output goes through SBI `console_putchar` ecalls, implemented in the `kernel-arch-riscv64` crate (arch-specific code lives under `arch/`, per the architecture docs). All bare-metal code is gated on `cfg(target_os = "none")` / `cfg(target_arch = "riscv64")`, so the host `cargo build` / `cargo test` stay green; the cross-build is an explicit `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Tech Stack:** Rust nightly (pinned), built-in `riscv64gc-unknown-none-elf` target (rustup ships precompiled `core` for it — **no `build-std`, no custom target JSON needed**, correcting a Phase 0 assumption), `rust-lld` linker, QEMU (`qemu-system-riscv64 -machine virt`), OpenSBI (`-bios default`), PowerShell tooling. Platform: Windows (native).

---

## Conventions for this plan

- **Commits:** Conventional Commits, SSH-signed (automated), **no** `Co-Authored-By` trailer. The executor may commit directly.
- **Paths** are relative to the repo root `D:\Projects\Kernel` unless absolute.
- **"Verify" steps** are exact commands with expected output. Do not skip them.
- **Background facts the executor needs:**
  - On QEMU riscv64 `virt` with `-bios default`, OpenSBI (machine-mode firmware) initializes the machine, then jumps to the `-kernel` ELF's entry point in **S-mode**, passing `a0` = booting hart id and `a1` = device-tree blob pointer. Supervisor RAM conventionally starts at `0x80200000`.
  - A freestanding Rust binary has no `main`, no std, no stack, and a non-zeroed `.bss` — the boot assembly must set up the stack and zero `.bss` before any Rust runs.
  - The **legacy SBI extension** `console_putchar` (EID `0x01`): put the byte in `a0`, EID in `a7`, execute `ecall`; OpenSBI prints the byte on the serial console. Deprecated in the SBI spec but universally supported by OpenSBI; the modern DBCN extension can replace it in a later phase.
  - Exit an interactive `-nographic` QEMU with `Ctrl-A` then `X`.
- **Kernel console output must be plain ASCII** (no em-dashes etc.) so serial output renders identically everywhere.

## File Structure

```
libs/common/src/lib.rs            Task 1 (modify: no_std-ready)
kernel/src/lib.rs                 Task 1 (modify: host-testable GREETING, drop String banner)
arch/riscv64/src/lib.rs           Task 2 (modify: declare gated modules)
arch/riscv64/src/sbi.rs           Task 2 (create: ecall wrapper)
arch/riscv64/src/console.rs       Task 2 (create: fmt::Write + print!/println!)
tools/test-qemu.ps1               Task 3 (create: boot smoke test — written FIRST, fails)
kernel/src/main.rs                Task 4 (create: kmain + panic handler + host stub)
kernel/src/boot.rs                Task 4 (create: _start assembly)
kernel/kernel.ld                  Task 4 (create: linker script)
kernel/build.rs                   Task 4 (create: applies linker script for riscv only)
kernel/Cargo.toml                 Task 4 (modify: add arch dependency)
.cargo/config.toml                Task 4 (modify: runner + qemu alias, corrected comments)
tools/build.ps1                   Task 5 (modify: add cross-build stage)
tools/run-qemu.ps1                Task 5 (modify: boot OUR kernel)
tools/README.md                   Task 5 (modify: document new behavior)
rust-toolchain.toml               Task 6 (modify: comment correction only)
README.md                         Task 6 (modify: status, getting started)
docs/roadmap/roadmap.md           Task 6 (modify: phase status)
docs/learning/0002-boot-and-hello-world.md  Task 6 (create)
docs/glossary.md                  Task 6 (modify: new terms)
CONTRIBUTING.md                   Task 6 (modify: test command, if it lists commands)
```

---

## Task 1: Make the shared crates freestanding-ready

`kernel` (bare-metal) will depend on `kernel-common`, so both must compile without std. The trick `#![cfg_attr(not(test), no_std)]` keeps host unit tests working. The kernel lib's Phase 0 `banner() -> String` needs an allocator (none exists bare-metal), so it is replaced by a `&'static str` greeting constant — the string the boot test will look for.

**Files:**
- Modify: `libs/common/src/lib.rs`
- Modify: `kernel/src/lib.rs`

- [x] **Step 1: Make `kernel-common` no_std**

Replace the top of `libs/common/src/lib.rs` so the crate is `no_std` outside tests (rest of the file, including the existing test, is unchanged):

```rust
#![cfg_attr(not(test), no_std)]
//! Shared types and utilities used across the Kernel project.
//!
//! `no_std`: this crate is used by the freestanding kernel, so it cannot
//! assume an operating system underneath (std is re-enabled for host
//! unit tests only). Real shared types (capabilities, error types, IDs)
//! arrive in later phases.
```

- [x] **Step 2: Replace the kernel lib's String banner with a no_std greeting**

Replace `kernel/src/lib.rs` entirely with:

```rust
#![cfg_attr(not(test), no_std)]
//! The microkernel (working title).
//!
//! Phase 1: the package's binary (`src/main.rs`) is a freestanding
//! `no_std` kernel that boots under QEMU on riscv64 and prints a
//! greeting. This library holds the host-testable parts.

/// The greeting the kernel prints at boot — the Phase 1 milestone
/// ("Done when: ./tools/run-qemu.ps1 loads our kernel and it prints
/// 'hello world'", docs/roadmap/roadmap.md). The boot smoke test
/// (tools/test-qemu.ps1) greps the serial log for exactly this string.
pub const GREETING: &str = "hello world";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greeting_is_hello_world() {
        assert_eq!(GREETING, "hello world");
    }
}
```

(The Phase 0 `banner()` function and its test are intentionally removed: `String` requires an allocator the bare-metal kernel doesn't have.)

- [x] **Step 3: Verify host build and tests stay green**

Run:
```powershell
cargo build --workspace; cargo test --workspace
```
Expected: build `Finished`; all tests pass (`test result: ok`), including the new `greeting_is_hello_world`.

- [x] **Step 4: Commit**

```powershell
git add libs/common/src/lib.rs kernel/src/lib.rs
git commit -m "refactor: make common and kernel libs no_std-ready for bare metal"
```

---

## Task 2: SBI console in `arch/riscv64`

Arch-specific code lives under `arch/` (docs/architecture/hardware-abstraction.md). This adds the `ecall` wrapper and a `core::fmt::Write` console with `print!`/`println!` macros. Everything bare-metal is gated on `target_arch = "riscv64"`, so the crate still builds and tests on the host.

**Files:**
- Modify: `arch/riscv64/src/lib.rs`
- Create: `arch/riscv64/src/sbi.rs`
- Create: `arch/riscv64/src/console.rs`

- [x] **Step 1: Declare the gated modules in `arch/riscv64/src/lib.rs`**

Replace the file with:

```rust
#![cfg_attr(not(test), no_std)]
//! RISC-V (riscv64) architecture-specific code — the first target.
//!
//! Phase 1: the SBI call wrappers and the kernel console used by the
//! freestanding kernel binary. Bare-metal modules are gated to
//! `target_arch = "riscv64"` so this crate still builds and tests on
//! the host. Other architectures (x86-64, ARM64) get sibling crates
//! later; the HAL keeps them interchangeable.

#[cfg(target_arch = "riscv64")]
pub mod console;
#[cfg(target_arch = "riscv64")]
pub mod sbi;

/// The architecture identifier this crate targets.
pub const ARCH: &str = "riscv64";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arch_is_riscv64() {
        assert_eq!(ARCH, "riscv64");
    }
}
```

- [x] **Step 2: Create `arch/riscv64/src/sbi.rs`**

```rust
//! Minimal Supervisor Binary Interface (SBI) wrappers.
//!
//! Our kernel runs in S-mode (supervisor); OpenSBI runs below it in
//! M-mode (machine). An `ecall` instruction from S-mode traps into
//! OpenSBI, which performs the requested service and returns. The
//! extension id goes in register `a7`, arguments in `a0`/`a1`/...
//!
//! Phase 1 needs exactly one call: legacy `console_putchar` (EID 0x01).
//! It is deprecated in the SBI spec but universally supported by
//! OpenSBI; the modern replacement (the DBCN extension) can land in a
//! later phase.

use core::arch::asm;

/// Print one byte to the firmware console (legacy SBI EID 0x01).
pub fn console_putchar(c: u8) {
    unsafe {
        asm!(
            "ecall",
            in("a7") 0x01usize,
            inout("a0") c as usize => _, // a0 carries the argument in and the SBI return value out
            out("a1") _,
        );
    }
}
```

- [x] **Step 3: Create `arch/riscv64/src/console.rs`**

```rust
//! Kernel console: formatted text output over the SBI firmware console.
//!
//! `SbiConsole` implements `core::fmt::Write` by sending every byte
//! through [`crate::sbi::console_putchar`], which lets us reuse Rust's
//! normal formatting machinery (`write!`, `format_args!`) with no
//! allocator. The `print!`/`println!` macros mirror std's.

use core::fmt::{self, Write};

use crate::sbi;

/// Zero-sized writer that sends bytes to the SBI console.
pub struct SbiConsole;

impl Write for SbiConsole {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            sbi::console_putchar(b);
        }
        Ok(())
    }
}

/// Implementation detail of `print!`/`println!`. Not for direct use.
pub fn _print(args: fmt::Arguments) {
    // Writing to the SBI console cannot fail; ignore the fmt::Result.
    let _ = SbiConsole.write_fmt(args);
}

/// Prints to the SBI console.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::console::_print(core::format_args!($($arg)*)));
}

/// Prints to the SBI console, with a trailing newline.
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", core::format_args!($($arg)*)));
}
```

- [x] **Step 4: Verify — host tests still pass AND the crate typechecks for riscv64**

Run:
```powershell
cargo test -p kernel-arch-riscv64
cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf
```
Expected: host test `arch_is_riscv64` passes; the cross-build finishes `Finished` with no errors (this is the first time the gated modules actually compile — the precompiled `core` for the built-in target is used automatically, no `build-std`).

- [x] **Step 5: Commit**

```powershell
git add arch/riscv64/src
git commit -m "feat(arch): add SBI console_putchar and print!/println! console for riscv64"
```

---

## Task 3: Boot smoke test (written first — must fail)

The executable acceptance test for Phase 1: boot the kernel headless under QEMU, capture the serial console to a file, assert it contains `hello world`. Written before the kernel exists, so it must fail now.

**Files:**
- Create: `tools/test-qemu.ps1`

- [x] **Step 1: Create `tools/test-qemu.ps1`**

```powershell
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
```

- [x] **Step 2: Run it to verify it FAILS (no kernel binary exists yet)**

Run:
```powershell
./tools/test-qemu.ps1; "exit=$LASTEXITCODE"
```
Expected: the cross-build of `-p kernel` succeeds (lib only — no binary target exists yet), then **`BOOT TEST FAIL: kernel binary not produced ...`** and `exit=1`. This is the correct failing state.

- [x] **Step 3: Commit**

```powershell
git add tools/test-qemu.ps1
git commit -m "test: add QEMU boot smoke test for the Phase 1 greeting (failing)"
```

---

## Task 4: The freestanding kernel binary

The heart of Phase 1: linker script, boot assembly, `kmain`, panic handler. After this task the smoke test passes.

**Files:**
- Create: `kernel/kernel.ld`
- Create: `kernel/build.rs`
- Create: `kernel/src/boot.rs`
- Create: `kernel/src/main.rs`
- Modify: `kernel/Cargo.toml`
- Modify: `.cargo/config.toml`

- [x] **Step 1: Create the linker script `kernel/kernel.ld`**

```
/* Linker script for the QEMU riscv64 `virt` machine.
 *
 * OpenSBI (-bios default) occupies RAM from 0x80000000 and jumps to our
 * ELF entry point in S-mode; supervisor software conventionally starts
 * at 0x80200000. We link everything at that address (no virtual memory
 * yet — that is Phase 2).
 */
OUTPUT_ARCH(riscv)
ENTRY(_start)

BASE_ADDRESS = 0x80200000;

SECTIONS
{
    . = BASE_ADDRESS;

    .text : {
        *(.text.boot)            /* _start must be the first code */
        *(.text .text.*)
    }

    .rodata : {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }

    .data : {
        *(.data .data.*)
        *(.sdata .sdata.*)
    }

    /* boot.rs zeroes [__bss_start, __bss_end) in 8-byte steps. */
    .bss : ALIGN(8) {
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        . = ALIGN(8);
        __bss_end = .;
    }

    /* Boot stack for the single hart we run in Phase 1. */
    .stack (NOLOAD) : ALIGN(16) {
        . += 64K;
        __stack_top = .;
    }

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
    }
}
```

- [x] **Step 2: Create `kernel/build.rs`**

The linker script must apply ONLY to the riscv bare-metal build — passing `-T` to the host linker would break host builds/tests.

```rust
//! Applies the linker script to the bare-metal kernel binary.
//!
//! Gated on the riscv target: host builds (`cargo build`, `cargo test`)
//! must not receive `-T`, or the host linker would fail.

fn main() {
    println!("cargo:rerun-if-changed=kernel.ld");
    let target = std::env::var("TARGET").unwrap_or_default();
    if target == "riscv64gc-unknown-none-elf" {
        let dir = std::env::var("CARGO_MANIFEST_DIR").unwrap();
        println!("cargo:rustc-link-arg-bins=-T{dir}/kernel.ld");
    }
}
```

- [x] **Step 3: Create `kernel/src/boot.rs`**

```rust
//! Boot entry: the first instructions our kernel ever runs.
//!
//! OpenSBI jumps here in S-mode with `a0` = booting hart id and
//! `a1` = device-tree blob pointer. Rust code needs a stack and a
//! zeroed .bss before it can run, so the entry is a tiny piece of
//! assembly that does exactly that and then calls `kmain` (main.rs).
//! The symbols `__stack_top`, `__bss_start`, `__bss_end` come from the
//! linker script (kernel.ld).

use core::arch::global_asm;

global_asm!(
    r#"
    .section .text.boot
    .global _start
_start:
    # a0 (hartid) and a1 (dtb) are left untouched: they become kmain's
    # two arguments under the C calling convention.
    la   sp, __stack_top

    # Zero .bss. Rust assumes statics start zeroed; nobody has done it
    # for us. The linker script 8-aligns both symbols, so 8-byte stores
    # are safe.
    la   t0, __bss_start
    la   t1, __bss_end
1:
    bgeu t0, t1, 2f
    sd   zero, 0(t0)
    addi t0, t0, 8
    j    1b
2:
    call kmain
"#
);
```

- [x] **Step 4: Create `kernel/src/main.rs`**

```rust
//! Kernel entry point.
//!
//! Bare-metal (`target_os = "none"`, i.e. the riscv64 cross-build):
//! a freestanding binary — no std, no main. OpenSBI hands control to
//! `_start` (boot.rs), which calls [`bare::kmain`].
//!
//! On the host this compiles to a tiny stub `main` instead, so the
//! Phase 0 promise — `cargo build` / `cargo test` stay green on the
//! host — still holds.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
mod boot;

#[cfg(target_os = "none")]
mod bare {
    use core::panic::PanicInfo;

    use kernel::GREETING;
    use kernel_arch_riscv64::println;
    use kernel_common::PROJECT_NAME;

    /// Rust entry, called from the boot assembly with the arguments
    /// OpenSBI gave us. Never returns: a kernel has nowhere to return to.
    #[no_mangle]
    extern "C" fn kmain(hartid: usize, _dtb: usize) -> ! {
        println!();
        println!("{GREETING} from {PROJECT_NAME} - Phase 1 (hart {hartid})");
        println!("(kernel is idle; exit QEMU with Ctrl-A then X)");
        park()
    }

    /// Halt this hart forever: `wfi` sleeps until an interrupt, the loop
    /// goes back to sleep if one arrives (none are enabled yet).
    fn park() -> ! {
        loop {
            unsafe { core::arch::asm!("wfi") };
        }
    }

    /// Freestanding binaries must provide their own panic behavior:
    /// report on the console, then park. No unwinding (panic = abort).
    #[panic_handler]
    fn panic(info: &PanicInfo) -> ! {
        println!("KERNEL PANIC: {info}");
        park()
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    println!("kernel host stub - the real kernel runs under QEMU: ./tools/run-qemu.ps1");
}
```

- [x] **Step 5: Add the arch dependency to `kernel/Cargo.toml`**

Replace the `[dependencies]` section with:

```toml
[dependencies]
kernel-common = { path = "../libs/common" }
kernel-arch-riscv64 = { path = "../arch/riscv64" }
```

- [x] **Step 6: Update `.cargo/config.toml`**

Replace the whole file with:

```toml
# Cargo configuration.
#
# The default target stays UNSET so `cargo build` / `cargo test` run on
# the host and stay green. The freestanding kernel is cross-built
# explicitly:
#
#   cargo build -p kernel --target riscv64gc-unknown-none-elf
#
# Phase 0 staged `-Z build-std` here, but it turned out to be
# unnecessary: riscv64gc-unknown-none-elf is a BUILT-IN target and
# rustup ships a precompiled `core` for it (installed via
# rust-toolchain.toml). See docs/learning/0002-boot-and-hello-world.md.

[alias]
# Convenience aliases.
lint = "clippy --workspace --all-targets"
# Cross-build the kernel and boot it under QEMU in one command.
qemu = "run -p kernel --target riscv64gc-unknown-none-elf"

[target.riscv64gc-unknown-none-elf]
# `cargo run`/`cargo qemu` boots the produced ELF under QEMU. OpenSBI
# (-bios default) starts in M-mode, then jumps to our entry in S-mode.
# Exit with Ctrl-A then X.
runner = "qemu-system-riscv64 -machine virt -nographic -bios default -kernel"
```

- [x] **Step 7: Verify the host stays green**

Run:
```powershell
cargo build --workspace; cargo test --workspace
```
Expected: build `Finished`, all tests pass. (The kernel binary builds as the host stub here.)

- [x] **Step 8: Run the boot smoke test — it must now PASS**

Run:
```powershell
./tools/test-qemu.ps1; "exit=$LASTEXITCODE"
```
Expected: `BOOT TEST PASS: kernel printed 'hello world'.` and `exit=0`.

If it fails, debug systematically; typical causes: linker script not applied (check `build.rs` ran — `cargo build -p kernel --target riscv64gc-unknown-none-elf -v` shows the `-T` arg), or QEMU not on PATH.

- [x] **Step 9: Commit**

```powershell
git add kernel .cargo/config.toml
git commit -m "feat: boot a freestanding no_std kernel under QEMU and print hello world"
```

---

## Task 5: Developer tooling catches up

`build.ps1` gains the cross-build stage; `run-qemu.ps1` now boots OUR kernel (the Phase 1 "done" signal); `tools/README.md` documents all three scripts.

**Files:**
- Modify: `tools/build.ps1`
- Modify: `tools/run-qemu.ps1`
- Modify: `tools/README.md`

- [x] **Step 1: Replace `tools/build.ps1`**

```powershell
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
```

- [x] **Step 2: Replace `tools/run-qemu.ps1`**

```powershell
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
```

- [x] **Step 3: Update `tools/README.md`**

Update it to describe (keep the existing folder-purpose intro):
- `build.ps1` — host build + tests, then riscv64 cross-build of the kernel.
- `run-qemu.ps1` — cross-builds and boots **our kernel** under QEMU; expect the OpenSBI banner followed by `hello world from Kernel (working title) - Phase 1 (hart 0)`; exit with `Ctrl-A` then `X`. (No longer firmware-only — that was Phase 0.)
- `test-qemu.ps1` — non-interactive boot smoke test; exit code 0 when the greeting appears on the serial console.
- `check-references.ps1` — keep its existing description unchanged.
- The `cargo qemu` alias as the one-liner equivalent of `run-qemu.ps1`.

- [x] **Step 4: Verify both scripts**

Run:
```powershell
./tools/build.ps1
```
Expected: host build + tests OK, cross-build OK, green "Build + tests OK."

Then run `./tools/run-qemu.ps1` (interactive — if the executor cannot interact, run `./tools/test-qemu.ps1` instead and have the user confirm `run-qemu.ps1` manually later):
Expected: OpenSBI banner, then:
```
hello world from Kernel (working title) - Phase 1 (hart 0)
(kernel is idle; exit QEMU with Ctrl-A then X)
```

- [x] **Step 5: Commit**

```powershell
git add tools/build.ps1 tools/run-qemu.ps1 tools/README.md
git commit -m "chore(tools): build script cross-builds kernel; run-qemu boots our kernel"
```

---

## Task 6: Documentation catches up

**Files:**
- Modify: `rust-toolchain.toml` (comment only)
- Modify: `README.md`
- Modify: `docs/roadmap/roadmap.md`
- Create: `docs/learning/0002-boot-and-hello-world.md`
- Modify: `docs/learning/README.md` (link the new note)
- Modify: `docs/glossary.md`
- Modify: `CONTRIBUTING.md` (only if it lists build/run commands)

- [x] **Step 1: Correct the `rust-toolchain.toml` comment**

Replace the comment block (keep the `[toolchain]` table unchanged):

```toml
# Pins the Rust toolchain for reproducible builds across machines.
# Phase 0 assumed nightly was needed for `-Z build-std`; Phase 1 showed
# the built-in riscv64gc-unknown-none-elf target ships a precompiled
# `core`, so build-std is NOT needed yet. Nightly is kept for upcoming
# phases (custom target specs, unstable features) — revisit if stable
# suffices. See docs/learning/0002-boot-and-hello-world.md.
```

- [x] **Step 2: Update `README.md`**

- Status line: replace the Phase 0 status with: **"Phase 1 — hello world. Our own freestanding kernel boots under QEMU/riscv64 and prints `hello world` via the SBI console. Phase 0 (foundation docs + toolchain) is complete."**
- Getting started: keep prerequisites; commands become:
  - `./tools/build.ps1` — build + test everything (host + riscv64 cross-build)
  - `./tools/run-qemu.ps1` — boot the kernel in QEMU (exit: `Ctrl-A` then `X`), with the expected output snippet (OpenSBI banner, then the greeting lines as in Task 5 Step 4)
  - `./tools/test-qemu.ps1` — automated boot check
- Repository layout: adjust the `kernel/` line to say "the microkernel — freestanding `no_std` binary (boots in Phase 1)".

- [x] **Step 3: Update `docs/roadmap/roadmap.md`**

- Phase 0 heading: change `*(in progress)*` to `*(done — 2026-06-08)*` (use the date of the Phase 0 completion commit `1b4bdf7`, retrievable via `git log -1 --format=%as 1b4bdf7`).
- Phase 1 heading: append `*(done — 2026-06-09)*` and update the "You learn" line to: "the boot process, freestanding Rust, linker scripts, SBI calls (and why `build-std` wasn't needed yet — see learning note 0002)."

- [x] **Step 4: Create `docs/learning/0002-boot-and-hello-world.md`**

Content (write as flowing beginner-friendly prose, ~1 page, covering exactly these points):
1. **The boot chain:** QEMU `virt` machine powers on → OpenSBI firmware (M-mode, machine mode — most privileged) initializes hardware and prints its banner → jumps to our ELF entry `_start` at `0x80200000` in S-mode (supervisor mode — where kernels live), passing hart id in `a0` and a device-tree pointer in `a1`. "hart" = hardware thread, RISC-V's word for a CPU core.
2. **Why assembly first:** Rust assumes a stack and zeroed `.bss`; on bare metal nobody has set those up, so ~10 instructions of assembly do it before `kmain`.
3. **What the linker script does:** places code/data at the exact physical addresses QEMU's RAM expects and defines the symbols (`__stack_top`, `__bss_start`, `__bss_end`) the boot code needs.
4. **How printing works with no OS:** an `ecall` from S-mode traps into OpenSBI, which writes the byte to the UART — the SBI is the "BIOS interface" of RISC-V. Implementing `core::fmt::Write` on top of one putchar gives us full `println!` formatting without an allocator.
5. **The build-std correction:** Phase 0 assumed cross-building `core` from source (`-Z build-std`) was required; actually rustup ships precompiled `core` for the built-in `riscv64gc-unknown-none-elf` target, so the custom target JSON + build-std were dropped. Lesson: verify assumptions against the current toolchain before staging config.
6. **Day-to-day commands:** `./tools/run-qemu.ps1` (or `cargo qemu`), exit QEMU with `Ctrl-A` then `X`; `./tools/test-qemu.ps1` for the automated check.

Also add a link line for note 0002 in `docs/learning/README.md`.

- [x] **Step 5: Add new terms to `docs/glossary.md`**

Check which of these are missing (`Select-String -Path docs/glossary.md -Pattern "SBI","ecall","hart","S-mode","M-mode","linker script","wfi"`) and add the missing ones, one or two sentences each, in the file's existing alphabetical/format style:
- **SBI (Supervisor Binary Interface):** the standard interface a RISC-V kernel (S-mode) uses to request services from firmware (M-mode), e.g. "print this byte". OpenSBI implements it.
- **ecall:** the RISC-V instruction that traps into the next-more-privileged mode; how the kernel calls the SBI (and how user programs will call the kernel later).
- **hart:** RISC-V's term for a hardware thread (a CPU core as the software sees it).
- **M-mode / S-mode / U-mode:** RISC-V privilege levels — machine (firmware, most privileged), supervisor (the kernel), user (applications).
- **linker script:** instructions to the linker about where in memory each part of a binary must be placed; essential on bare metal where addresses are physical and fixed.
- **wfi (wait for interrupt):** RISC-V instruction that sleeps the hart until an interrupt arrives; our idle loop.

- [x] **Step 6: Check `CONTRIBUTING.md`**

Run `Select-String -Path CONTRIBUTING.md -Pattern "build.ps1","run-qemu","cargo"`. If it lists build/run commands, add `./tools/test-qemu.ps1` next to them and ensure nothing still says "firmware only". If it only links to other docs, no change needed.

- [x] **Step 7: Verify cross-references**

Run:
```powershell
./tools/check-references.ps1
```
Expected: no broken references reported (the new learning-note path is referenced from rust-toolchain.toml, .cargo/config.toml, roadmap, and learning README — all must resolve).

- [x] **Step 8: Commit**

```powershell
git add rust-toolchain.toml README.md docs CONTRIBUTING.md
git commit -m "docs: update for Phase 1 (boot learning note, roadmap, README, glossary)"
```

---

## Task 7: Final acceptance & push

- [x] **Step 1: Full clean-ish rebuild + all tests**

Run:
```powershell
./tools/build.ps1
./tools/test-qemu.ps1; "exit=$LASTEXITCODE"
cargo lint
```
Expected: "Build + tests OK."; "BOOT TEST PASS"; `exit=0`; clippy finishes with no errors (warnings acceptable but fix what's trivial).

- [x] **Step 2: Acceptance against the roadmap "done" signal**

`docs/roadmap/roadmap.md` Phase 1: "Done when: `./tools/run-qemu.ps1` loads our kernel and it prints 'hello world'." — confirmed in Task 5 Step 4 (interactive) and continuously by `test-qemu.ps1`.

- [x] **Step 3: Clean tree and push**

Run:
```powershell
git status -s
git log --oneline -8
git push
```
Expected: clean tree; the Phase 1 commit sequence; push succeeds to `origin/main`.

- [x] **Step 4: Knowledge-base check**

If any real issue was hit and fixed during execution (toolchain, QEMU flags, linker errors...), record it as the next `knowledge-base/entries/KB-000N.md` per `knowledge-base/schema/issue-record.md` and commit it (`docs: record KB-000N (<short title>)`). If nothing went wrong, skip — do not invent entries.

---

## Self-Review (completed)

**Spec coverage:** Roadmap Phase 1 goal (boot tiny `no_std` kernel in QEMU, print to screen) → Tasks 3–4; "done" signal (`run-qemu.ps1` prints hello world) → Task 5; learning goals (boot process, freestanding Rust, linker scripts; build-std correction) → Task 6 learning note; spec §9 open question (bootloader approach) resolved as OpenSBI `-bios default` + `-kernel` ELF, documented in `.cargo/config.toml` and the learning note.

**Placeholder scan:** No TBDs. All code complete. Doc-prose steps enumerate the exact required content points (Phase 0 plan precedent).

**Type consistency:** `GREETING` (kernel lib, Task 1) used by `main.rs` (Task 4) and grepped by `test-qemu.ps1` (Task 3) — all "hello world". Symbols `__stack_top`/`__bss_start`/`__bss_end` defined in `kernel.ld` (Task 4 Step 1) and consumed in `boot.rs` (Step 3). `kmain(hartid, _dtb)` matches the asm `call kmain` with untouched `a0`/`a1`. Crate names `kernel-arch-riscv64` / import `kernel_arch_riscv64` consistent. `console::_print` matches the macro path `$crate::console::_print`.

**Scope:** Phase 1 only — no interrupts, no memory management, no DBCN, no multi-hart support (all Phase 2+). Host build stays green throughout.
