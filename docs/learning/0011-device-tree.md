# 0011 — Discovering hardware from the device tree (Phase 4a)

**One-line:** the kernel learns its RAM size and timer frequency from the
device tree the firmware passes it, instead of hardcoding QEMU's numbers.

## What changed
- OpenSBI passes the address of a *flattened device tree* (FDT) in `a1` —
  the `dtb` argument `kmain` always received and ignored. A small hand-rolled
  `no_std` parser (`arch/riscv64/src/dt.rs`) reads the `/memory` node's `reg`
  (RAM base+size) and `timebase-frequency`.
- `mem::init` now takes the discovered `ram_end`; `timer::init` takes the
  discovered frequency. The hardcoded `RAM_END` (0x8800_0000) and
  `TIMEBASE_HZ` (10 MHz) constants are gone.

## The key idea
The FDT is a big-endian binary: a header, then a token stream
(BEGIN_NODE + name / PROP + len + name-offset + value / END_NODE / END) plus
a strings block. Walking it for two values is a few dozen lines — and it's
why the kernel can run on hardware that isn't QEMU.

## A consequence that bit us
Once RAM is discovered rather than fixed, the frame allocator's bitmap (a
fixed `.bss` array sized for 128 MiB) overflowed under `-m 192M`. Fix:
enlarge the cap to 4 GiB (a 128 KiB zeroed bitmap). A truly dynamic bitmap,
sized in RAM to the discovered frame count, is deferred until boards with
very large RAM arrive.

## Proof
Host tests parse a captured real QEMU DTB (128 MiB, 0x8000_0000, 10 MHz) and
reject a bad/truncated blob. The smoke test boots QEMU with `-m 192M` and the
kernel reports **192 MiB** — proving it read the device tree, not a constant.

## Still QEMU-only
This needs no board. Next (Phase 4b) is discovering the UART from the DTB and
driving real serial — the step that actually boots on a physical RISC-V board.
