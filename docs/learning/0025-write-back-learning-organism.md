# 0025 — Write-back: the organism learns across reboots

**One-line:** the self-healer stops only *reading* its knowledge base and starts
*writing* to it — it meets a crash class it has never catalogued, records a new
entry to the disk, and a **second boot of the same image** diagnoses that
formerly-novel crash against the entry it wrote itself. The write half that
makes the knowledge base *real*.

## What changed
- The filesystem gained an **append-only** write path. A new file lands at the
  end of the volume with one new directory entry; the format is unchanged. Pure
  placement logic (`fs::append_plan`) and a serializer that is the exact inverse
  of the reader (`kb::serialize`, round-trip host-tested against `kb::parse`)
  define it; the kernel orchestrates the block writes through the existing `blk`
  call/reply server, now with a **direction bit in the IPC badge** (read vs
  write the shared DMA page).
- A new fault class, `IllegalInstruction` (scause 2), is decoded, contained in
  U-mode like a page fault, and given its own token (`illegal-instruction`).
- A **KB-writer** kernel task drains a single-slot *novel-cause mailbox* the
  crash path fills, mints the next `KB-NNNN` entry (default caged-restart
  playbook) for the unrecognized token, appends it to disk, and installs it.
- `mkfs` pads the image with **spare capacity** so the in-kernel writer has
  device room to append into.

## The idea worth keeping
**The kernel can *name* a symptom it has not *catalogued*.** 6c drew the
code/disk line at "the kernel decodes a trap into a token; the meaning keyed by
that token is on disk." Write-back is the natural other side: a *novel* crash is
one the kernel can tokenize (`cause_token → Some`) but the KB has no entry for
(`diagnose → None`). The kernel owns the symptom class; the organism accumulates
the knowledge. Recording = writing down "I saw symptom X; my default response is
a bounded, caged restart" — and next boot the organism recognizes X. Idempotency
falls out for free: once an entry is installed (or loaded), its token matches, so
the novel condition is never met again — the same crash is never recorded twice.

## The constraint that shaped it (a commit point, not a transaction)
There is no journal and no in-place mutation. Crash-consistency comes from
**write ordering**: data block → directory block → **superblock last**. The
superblock write is the single *commit point*, because it is what bumps
`dir_entries`/`total_blocks` to reference the new blocks. A crash before it
leaves the new blocks present but unreferenced — invisible, harmless. Capacity
overflow needs no pre-check either: a write past the device's end fails with a
device status, and the append aborts before the commit. The general lesson:
order your writes so the last one is the one that makes the rest *count*.

## Where the work happens (not the crash path)
`diagnose` runs in the contained-crash path with interrupts off — no I/O there
(the same rule 6c's loader obeyed). So the crash path only *latches* the novel
token; the actual disk append happens in the KB-writer task with interrupts on.
This cost a real bit of latency: the append is ~5 `blk` ops (two reads, three
writes), and each `blk` op is gated to roughly one-per-timer-tick by the QEMU
PLIC IRQ-recovery quirk from 6b/6c — so the first boot's write-back visibly
takes several ticks. Correct, just unhurried.

## Proof
Boot 1 (fresh image, KB-0005 only): `sched: task 'novel' killed by
IllegalInstruction`, then `heal: no known issue for IllegalInstruction
(recording for write-back)`, then `heal: recorded KB-0006 (illegal-instruction)
to disk`. Boot 2 (the *same* image, not rebuilt): `heal: loaded 2 KB entries
from disk`, then `heal: diagnosed KB-0006 (Observed fault: illegal-instruction
(auto-recorded)) -> playbook: Restart the component, up to a bounded number of
retries.` — the diagnosis is keyed by an entry that did not exist when boot 1
started. The organism learned across a reboot.

## What's next
Append-only is enough to *learn a new symptom* but not to *revise* one:
in-place updates (e.g. a "seen N times" counter on an existing entry),
deletion/compaction, a free-block allocator, and multi-block directories are all
deferred. The read loop (6c) plus this write loop are the two halves that make
the knowledge base a real, growing memory.
