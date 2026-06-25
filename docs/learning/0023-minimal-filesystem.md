# 0023 — A minimal filesystem

**One-line:** the kernel can read a file **by name** off the disk — a tiny
read-only filesystem over the Phase 6a block driver, the layer between raw
sectors and the on-disk knowledge base.

## What changed
- A shared `no_std` format (`kernel-common::fs`): a **superblock** (block 0)
  locating a flat **directory** (block 1) of fixed-size entries, each mapping a
  name to a contiguous **extent** (start block + byte length); file data in
  block 2+. Defined once, host-tested, used by both the kernel and `mkfs`.
- A host `mkfs` tool (`tools/mkfs`) builds the disk image from named files using
  that same format.
- The `blk` component stopped self-testing and became a **call/reply server**:
  receive a block number, read that sector into its identity-mapped DMA page,
  reply. A new kernel-side `sched::call_message` lets a kernel task be the IPC
  client (the S-mode counterpart to `recv_message`).
- The kernel filesystem reads through one door — `read_block(n)` (with a
  one-block cache) — then parses the superblock, scans the directory, and copies
  the file's extent out of the DMA page.

## The idea worth keeping
**The FS speaks only `read_block(n)`; the driver does the real I/O.** That one
boundary is the whole relationship between a filesystem and a block device: the
FS reasons about superblocks, directories, and extents in terms of numbered
blocks and never touches virtqueues or interrupts. Underneath, one
`call_message` to the unprivileged driver fetches the block into shared,
identity-mapped DMA memory the kernel reads directly — no copy through IPC.

## The bug worth remembering (a preemption livelock)
The `blk` server is the first **long-lived, preemptible U-mode task doing
repeated interrupt-driven I/O**, and it exposed a latent scheduler bug. A timer
tick that came due while interrupts were masked for a context switch
(`yield_now`/`park_current`) was delivered at the switched-to task's *first*
instruction — preempting it before it ran a single instruction. With the
round-robin landing `blk` there every cycle, `blk` made zero progress (a
livelock; it sat forever on the instruction right after `wait_irq` returned).
The fix: **re-arm the timer when switching to a task** (write `mtimecmp` to the
future, which also clears the pending `sip.STIP`), giving the new task a fresh
quantum. The general lesson: a pending interrupt must not be carried across a
context switch onto an unrelated task.

## Why this is step two of a real knowledge base
6a was the disk; 6b is the filesystem over it. 6c points the self-healer's
loader at this FS so it reads `knowledge-base/entries/*.md` from disk and
diagnoses against the real, runtime knowledge base instead of the compiled-in
`KB-0005` stub. The format already carries a length per file (multi-block
extents work today) and reserves dirent space for the write/append path 6c may
add.

## Proof
`fs: read 'KB-0005' (584 bytes)` followed by the file's contents — including a
marker that lives in the file's **second** block — read off a virtio-blk image
built by `mkfs`, through the unprivileged driver.
