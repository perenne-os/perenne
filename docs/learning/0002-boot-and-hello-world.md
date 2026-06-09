# 0002 — Boot and "hello world" (Phase 1)

How our kernel goes from QEMU powering on to text on the screen, and what
finally clicked along the way.

## The boot chain

When QEMU's riscv64 `virt` machine starts, our kernel is *not* the first
thing that runs:

1. **QEMU powers on** the virtual machine with RAM starting at `0x80000000`.
2. **OpenSBI** (the firmware, loaded by `-bios default`) runs first in
   **M-mode** — machine mode, the most privileged RISC-V level. It
   initializes the hardware and prints its big ASCII banner.
3. OpenSBI then drops to **S-mode** (supervisor mode — where kernels live)
   and jumps to our ELF's entry point, `_start`, linked at `0x80200000`.
   By convention it passes two values: the id of the booting **hart**
   (RISC-V's word for a hardware thread, i.e. a CPU core) in register
   `a0`, and a pointer to the device tree in `a1`.

What clicked: the firmware/kernel split mirrors the kernel/user split one
level up. We sit *between* OpenSBI below us and (future) user programs
above us.

## Why assembly comes first

Rust code silently assumes two things exist: a **stack** (for function
calls and locals) and a zeroed **`.bss`** section (for static variables).
On bare metal, nobody has set those up — so the first ~10 instructions of
the kernel (`kernel/src/boot.rs`) are assembly that loads the stack
pointer, zeroes `.bss`, and only then calls the Rust `kmain`. Calling
Rust before that would be undefined behavior.

## What the linker script does

`kernel/kernel.ld` tells the linker the exact physical addresses where
each part of the binary must live (code at `0x80200000`, then read-only
data, data, `.bss`, and a 64 KiB boot stack). It also defines the symbols
the boot assembly needs: `__stack_top`, `__bss_start`, `__bss_end`. On a
normal OS the loader picks addresses for you; on bare metal *you* are the
loader, so addresses are fixed and explicit.

## How printing works with no OS

There's no `write()` syscall — we *are* below that level. Instead, an
`ecall` instruction from S-mode traps into OpenSBI (M-mode), which writes
the byte to the serial port. The SBI is effectively the "BIOS interface"
of RISC-V. We use the legacy `console_putchar` call (extension id `0x01`
in `a7`, the byte in `a0`).

The nice part: implementing `core::fmt::Write` on top of that single
putchar (`arch/riscv64/src/console.rs`) gives us the full `println!`
formatting machinery — with no allocator and no std.

After printing, the kernel parks in a `wfi` loop ("wait for interrupt"):
the hart sleeps until an interrupt arrives, and since none are enabled
yet, it sleeps forever.

## The build-std correction

Phase 0 staged config for `-Z build-std`, assuming we'd have to compile
`core` from source for the bare-metal target. **That turned out to be
unnecessary:** `riscv64gc-unknown-none-elf` is a *built-in* Rust target,
and rustup ships a precompiled `core` for it (installed via
`rust-toolchain.toml`'s `targets` list). The custom target JSON and
build-std flags were dropped.

Lesson: verify assumptions against the current toolchain before staging
configuration for them. (Nightly is still pinned — future phases may need
unstable features — but Phase 1 alone would have worked on stable.)

## Day-to-day commands

```powershell
./tools/run-qemu.ps1    # build + boot the kernel (or: cargo qemu)
./tools/test-qemu.ps1   # automated boot check (greps for "hello world")
```

Exit QEMU with `Ctrl-A` then `X`.
