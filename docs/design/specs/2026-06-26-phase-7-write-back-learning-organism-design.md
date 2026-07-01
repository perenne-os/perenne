# Phase 7 — Write-back & the learning organism (design)

**Status:** approved 2026-06-26
**Arc:** the *write half* of Phase 6's storage arc. Phase 6 was read-only
(blk read → FS read → KB load). Phase 7 adds the write spine (blk write → FS
append → KB write-back) and proves it **across a power-cycle**: the self-healer
meets a crash class it has never catalogued, records a new knowledge-base entry
to disk, and a *second boot of the same disk image* loads that entry and now
diagnoses the formerly-novel crash. The organism learns across reboots — the
payoff [ADR 0005](../../decisions/0005-self-healing-knowledge-organism.md) is
built around, and the half learning note 0024 named as deferred.

One combined spec (not sub-phased): the writable FS and the learning healer are
designed and built together.

## The organizing idea

A refinement of 6c's code/disk line. The kernel decodes a trap into a stable
token (`cause_token` — that *is* the kernel's job: it can **name** a symptom
class). The organism accumulates *knowledge* — KB entries keyed by those
tokens. A **novel crash** is therefore one the kernel can tokenize
(`cause_token → Some(token)`) but the KB has no entry for (`diagnose → None`):
a symptom the kernel can name but the organism has not catalogued. Recording it
= the organism writing down "I saw symptom X; my default response is a bounded,
caged restart" — a new entry it authors itself, which a later boot loads and
recognizes.

This keeps diagnosis exactly what 6c made it (a pure in-kernel lookup over a
disk-loaded table) and adds only the *write* of new knowledge.

## Scope (YAGNI)

- **Append-only** writable FS: a new file is appended at the end of the volume
  (a new directory entry + contiguous data blocks). No in-place rewrite, no
  delete, no free-list, no fragmentation. A new KB entry *is* a new file — this
  is exactly enough.
- **One auto-recorded entry per novel token**, carrying the generic restart
  playbook. The organism catalogs the *symptom class*, not a hand-authored fix.
- The on-disk **format is unchanged** (superblock + one directory block +
  contiguous extents). The single directory block (≤ 8 entries) is preserved.

Out of scope / deferred: in-place edits, deletion, multi-directory-block
volumes, a free-block allocator, a richer authored playbook.

## Architecture & components

### Pure, host-tested logic (`libs/common`)

- **`fs::append_plan(superblock, dir_bytes, name, byte_len) -> Option<AppendPlan>`**
  — given the current superblock and directory block plus a new file's name and
  length, compute placement with no I/O:
  - `start_block` = the current `total_blocks` (append at end-of-volume);
  - the updated directory block bytes (the existing entries plus one new
    `DirEntry { name, start_block, byte_len }` at index `dir_entries`);
  - the updated `Superblock` (`dir_entries + 1`, `total_blocks += block_count(byte_len)`).
  - Returns `None` (refuses) if the directory block is already full
    (`dir_entries == DIRENTS_PER_BLOCK`) or if appending would exceed the
    device capacity the caller passes in (see padding, below).
  Mirrors the read side's purity — the format logic lives in one place, used
  identically host-side and in-kernel, and is host-tested.

- **`kb::serialize(record) -> heapless/bytes`** — the inverse of `kb::parse`:
  emit a frontmatter document
  ```
  ---
  id: KB-0006
  title: "Observed fault: illegal-instruction (auto-recorded)"
  match-cause: illegal-instruction
  playbook:
    - "Restart the component, up to a bounded number of retries."
  ---
  ```
  that `kb::parse` round-trips. A host test asserts
  `parse(serialize(r)) == r` for the runtime fields — so what the writer emits
  is provably what the next boot reads. `no_std`, no allocation (serialize into
  a caller-provided buffer; the document fits in one 512-byte block).

### Host tool (`tools/mkfs`)

- `mkfs` pads the image with **spare capacity**: a few free blocks past
  `total_blocks` (room for at least one more KB entry). QEMU fixes the
  virtio-blk device capacity from the image *file size* at start; the in-kernel
  writer can only append *within* that capacity. The superblock's
  `total_blocks` still tracks the *used* blocks; the image simply carries slack
  (free space — as a real filesystem does). The spare amount is a named
  constant.

### Kernel (`arch/riscv64`, `kernel/src/main.rs`)

- **blk server write path.** The `blk_component` server currently does
  `recv(badge = block#) → read → reply(status)`. Extend the protocol: the badge
  encodes **direction** — a high bit set means *write* block N from the shared
  DMA data page, otherwise *read* into it. The low-level `blk_req(write=true)`
  (T_OUT) already exists from 6a; the server selects it from the badge. The DMA
  data page is the same identity-mapped frame the FS reads/writes directly.

- **`fs_write_block(n)`** — publish the DMA data page to block `n` via the blk
  server (write badge). **`fs_append_file(name, bytes)`** — orchestrate the
  append:
  1. read the superblock (block 0) and directory (block 1);
  2. compute the plan via `fs::append_plan` (refuse/log if it returns `None`);
  3. write the **data block(s)** at `start_block` (fill the DMA page, write);
  4. write the **updated directory block**;
  5. write the **updated superblock LAST**.
  The superblock write is the single **commit point**: a crash before step 5
  leaves the new data + dirent present but unreferenced (`dir_entries` still
  old) and therefore invisible — crash-consistency for free, with no in-place
  mutation of existing data. A device write error (status ≠ 0) aborts before
  the commit; the volume stays at its prior consistent state.

- **Novel-cause mailbox + KB-writer task.** The crash path runs with interrupts
  off and can do no I/O (the 6c constraint), so the write is deferred to a task
  with interrupts on — exactly as 6c deferred the *read* to a boot task:
  - In the containment path, when `cause_token(cause) = Some(token)` **and**
    `diagnose(cause) = None`, enqueue `token` into a small fixed-size
    kernel mailbox.
  - A **KB-writer task** (interrupts on) waits on the mailbox; on a token it
    mints `KnownIssue { id: <next KB number>, title, playbook: <default
    restart>, match_cause: token }`, serializes it (`kb::serialize`), appends
    it to disk (`fs_append_file("KB-NNNN", bytes)`), and `heal::install`s it
    into the runtime table. The id is deterministic: the max KB-number in the
    runtime table plus one (boot 1 loads KB-0005 → writes KB-0006).
  - **Idempotency is automatic.** Once an entry is installed (or loaded at
    boot), its token *matches*, so `diagnose` is `Some` and the writer condition
    is never met again — the same token is never recorded twice. This is exactly
    what stops boot 2 from re-appending KB-0006.

- **`trap.rs`.** Add `Cause::IllegalInstruction` (scause exception code 2) with
  a `from_user` containment arm
  (→ `sched::exit_current(Killed(IllegalInstruction))`, like the existing
  user page-fault containment). `heal::cause_token` maps it →
  `"illegal-instruction"`. (Page-fault → `"page-fault"` is unchanged.)

- **Novel patient.** A new U-mode component that executes an illegal instruction
  (`unimp`) — a contained crash whose token has no KB entry at first boot. Its
  code is identical across both boots; only the disk differs.

## Data flow — the cross-boot proof

**Boot 1** (fresh `mkfs` image: KB-0005 only, plus spare capacity):
- the page-fault patient is contained and diagnosed **KB-0005** (6c regression
  holds);
- the novel patient runs `unimp` → contained → `cause_token =
  "illegal-instruction"`, `diagnose = None` → enqueued → the KB-writer appends
  **KB-0006** (`match-cause: illegal-instruction`) to the disk image and
  installs it.
- Marker: `heal: recorded KB-0006 (illegal-instruction) to disk`.

**Boot 2** (the *same* image — now KB-0005 + KB-0006 — with **no rebuild**):
- the loader reads **both** entries (`heal: loaded 2 KB entries from disk`);
- the same novel patient crashes, and now `diagnose` matches **KB-0006**, the
  entry the organism wrote itself.
- Marker: `heal: diagnosed KB-0006 (Observed fault: illegal-instruction ...)
  -> playbook: Restart the component, up to a bounded number of retries.`

That line — a correct diagnosis of a crash class boot 1 had never catalogued,
keyed by an entry written to disk in the previous boot — is the cross-boot
learning proof.

## Error handling

| Situation | Behavior |
|---|---|
| Append refused (dir block full / past device capacity) | `append_plan → None`; logged, no write; system continues. The writer flags rather than corrupts. |
| Device write error (status ≠ 0) | Abort the append before the superblock commit; volume stays consistent. |
| Crash mid-append | Superblock-last ordering: new blocks unreferenced and invisible; old state intact. |
| Novel token already known (boot 2) | `diagnose = Some`; writer condition never met; no duplicate. |

## Testing

**Host unit tests:**
- `fs::append_plan`: placement (start_block, dir update, superblock update);
  refusal when the directory block is full; refusal past capacity.
- `kb::serialize`: `parse(serialize(r)) == r` round-trip for the runtime fields.
- `heal::cause_token`: `IllegalInstruction → "illegal-instruction"`;
  page-fault mapping unchanged; the novel-recordable condition
  (`Some(token)` with `diagnose = None`).

**Boot test (`tools/test-qemu.ps1`) — now two boots over one image:**
1. build mkfs image fresh;
2. **boot 1**: assert the existing milestone set **plus**
   `heal: recorded KB-0006 (illegal-instruction) to disk`; then stop QEMU;
3. **boot 2** against the *same* image (no rebuild): assert
   `heal: loaded 2 KB entries from disk` and the **KB-0006** diagnosis line.

The single-boot run/match block is refactored into a helper invoked twice (boot
N → pattern set N). The drive is already `format=raw` (writable, not snapshot),
so boot 1's append persists into boot 2.

## What this proves / what's next

Proves the organism **learns across reboots**: it writes new knowledge to
persistent storage and consults it after a power-cycle — closing the
self-healing loop ADR 0005 set out. Naturally deferred beyond append-only:
in-place updates (e.g. a "seen N times" counter), deletion/compaction, a
free-block allocator, multi-block directories, and a richer authored playbook.
