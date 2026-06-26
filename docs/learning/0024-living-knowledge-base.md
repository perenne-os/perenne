# 0024 — The living knowledge base

**One-line:** the self-healer stops consulting a compiled-in stub and
diagnoses a contained crash against a knowledge-base entry **read and parsed
from disk at boot** — the organism is no longer hardcoded.

## What changed
- A `match-cause` token in the KB schema (`page-fault` today) makes an entry
  machine-matchable. `KB-0005.md` declares it; the *knowledge* ("this symptom
  class → this issue + playbook") now lives on disk.
- A pure `no_std` frontmatter parser (`kernel-common::kb`) turns an entry's
  bytes into `{ id, title, playbook, match_cause }`. Host-tested, like `fs`.
- `heal` gained a runtime table (`install` / `diagnose`): a boot-time **KB
  loader** task enumerates the on-disk directory, reads each entry's
  frontmatter through the `blk` server, and installs the tokened ones. The
  compiled-in `KB_0005` record is gone.
- `mkfs` packs only the **runtime-matchable** entries (those with a token) —
  filtering by token, not by name, keeps both sides data-driven.

## The idea worth keeping
**Diagnosis stays a pure, in-kernel table lookup; only its data moved to
disk.** `diagnose` runs in the crash path (interrupts off — no I/O allowed),
so the read + parse happen *earlier*, at boot, caching a parsed table; the
lookup itself is unchanged (ADR 0005's deterministic, explainable core holds).
And the line between code and disk is drawn deliberately: the kernel still owns
decoding a raw trap into a stable token (`cause_token` — that *is* the kernel's
job), while the meaning keyed by that token is data the organism reads.

## The constraint that shaped it (one device-IRQ wait per block)
`blk` waits for a completion interrupt per block read, and on this QEMU PLIC a
fast completion that asserts while the source is masked in-service is only
recovered on the next timer tick — so reads effectively run **one block per
tick**. 6b's handful of reads hid this; 6c's loader would not. The fix kept the
read count tiny: read only each entry's **frontmatter head** (the first two
blocks — frontmatter sits at the top), and pack only runtime-matchable entries.
The general lesson: when each I/O is expensive, read the least that answers the
question — here, the frontmatter, not the whole file.

## Ordering: don't diagnose before you've learned
The crash-prone "patient" tasks would otherwise fault before the table exists.
They block on a **KB-ready gate** (a one-shot rendezvous) on their first run;
the loader releases them only after the table is built. A restart skips the
gate — by then the organism has already read its memory.

## Proof
`heal: loaded 1 KB entry from disk`, then on the contained crash
`heal: diagnosed KB-0005 (User-space component terminated by a fatal fault) ->
playbook: Restart the component, up to a bounded number of retries.` — the
playbook text came off `KB-0005.md` on the disk, not a string in `heal.rs`.

## What's next
Write-back (record a newly-seen issue to disk) needs a writable FS layer —
deferred to a later phase. The read path is the half that makes the knowledge
base *real*.
