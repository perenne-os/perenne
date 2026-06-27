# Phase 10 — Revisable knowledge: the "seen N times" counter (design)

**Status:** approved 2026-06-27 (user authorized completing the phase end-to-end)
**Priority served:** #2 (the self-healing organism). Deepens the storage arc:
the organism's memory can now be **revised in place**, not just appended to.

## The gap

The organism reads its knowledge base from disk (6c), appends new entries across
reboots (7), and is interrogable (9) — but every entry is **write-once**. It
cannot update what it already knows. Phase 10 makes a record revisable: each time
the self-healer diagnoses a recurring issue, it increments a per-entry **"seen N
times" counter** and persists it *in place*, so the count **accumulates across
reboots**. This is the seed of the organism noticing recurrence (and later,
escalating chronic faults), and it introduces the one filesystem operation
Phase 7's append-only path deferred: **in-place update of an existing on-disk
record**.

## The idea (fixed-width fields make in-place mutation safe)

A counter that grows from `9` to `10` changes length, which would shift the rest
of the frontmatter and force a whole-file rewrite (destroying a rich entry's
other fields, since `kb::serialize` only emits the runtime subset). The fix is a
**fixed-width field**: `seen: 00000`, always 5 digits. Same width means an update
overwrites *only those bytes* in the entry's first block — no shifting, no
free-block allocator, no re-serialization. The teachable point: *in-place
mutation needs fixed-width fields.*

## Scope (YAGNI)

- **Only a fixed-width counter update.** No free-block allocator, no
  variable-length rewrite, no multi-block directories, no deletion. The
  block-write primitive (`fs_write_block`) already exists from Phase 7.
- The counter lives in the entry's **first block** (top of the frontmatter), so
  a single-block in-place write targets it.
- The persist happens off the crash path (interrupts on), like Phase 7's
  write-back, and **reuses the existing KB-writer task** (no new scheduler slot —
  we are near `MAX_TASKS`).

## Architecture & components

### Pure, host-tested logic (`libs/common/src/kb.rs`)

- **`parse`** gains a `seen: u32` field (read from a `seen:` line; default `0` if
  absent), added to `KbRecord`.
- **`serialize`** emits `seen: 00000` (the fixed-width initial counter).
- **`set_seen_in_block(block: &mut [u8], count: u32) -> bool`** — locate the
  fixed-width `seen: NNNNN` field (the 5 ASCII digits after `seen: `) in `block`
  and overwrite them with `count` zero-padded to 5 digits. Returns `false` if the
  field is absent or malformed (not 5 digits) — the in-place guard. Host-tested,
  including a round-trip: `set_seen_in_block` then `parse` reads the new count.

### Kernel (`arch/riscv64/src/heal.rs`)

- **`KnownIssue`** gains `seen: u32`, `start_block: u32` (the entry's on-disk
  location, so the persister knows which block to rewrite), and a `dirty: bool`
  (the count changed and needs persisting).
- **`install`** takes `start_block` and the parsed `seen`, storing them.
- **`note_diagnosis`** (called from the crash path, interrupts off — no I/O):
  increment the matched entry's `seen`, set its `dirty` flag, and record it as
  the last diagnosis (existing behavior).
- **`dirty_entry() -> Option<(u32, u32)>`** — return the `(start_block, seen)` of
  one entry needing persisting, for the writer task to drain; clears the `dirty`
  flag. (A scan over the table; single hart.)
- **`entry(i)`** extended so the shell's `kb` command can show the count
  (returns `(id, title, seen)`), and the loader can be logged.

### Kernel (the writer task, `kernel/src/main.rs`)

- The existing **KB-writer task** (Phase 7, interrupts on) gains a second job:
  after handling a novel-cause append, drain `heal::dirty_entry()` — for each
  dirty entry, read its `start_block` (`fs_read_block`), apply
  `kb::set_seen_in_block(block, seen)`, write the block back
  (`fs_write_block(start_block, block)`), and log
  `heal: persisted KB-0005 (seen N)`. Coalesces: it writes the *current* count,
  so several diagnoses collapse into few writes.
- The **loader** (`kb_loader_task`) passes `start_block` and the parsed `seen`
  into `install`.

### Schema/data

- `knowledge-base/entries/KB-0005.md` gains a `seen: 00000` line near the **top**
  of its frontmatter (within the first 512 bytes, so it sits in the entry's first
  block). `knowledge-base/schema/issue-record.md` documents the fixed-width
  convention. `mkfs` is unchanged (it packs whole entries).

## Data flow (the cross-boot proof)

Per boot, KB-0005 is diagnosed **4 times** deterministically: `transient` crashes
once (then recovers); `flaky` crashes 3 times across its bounded restarts. So
`seen` goes 0→4, persisted in place. **Boot 1** ends with `KB-0005 seen=4` on the
disk image. **Boot 2** (the same image, not rebuilt): the loader reads `seen=4`,
the same 4 crashes bring it to **8**, persisted. The counter accumulated across a
reboot — memory that *updates*, not just grows. Phase 9's shell ties in: `kb`
shows `KB-0005 (seen 8)  …`.

## Error handling

| Situation | Behavior |
|---|---|
| `seen:` field absent/malformed in the block | `set_seen_in_block` → `false`; logged, no write; the entry stays valid. |
| Device write error on persist | logged; the in-RAM count is correct and retried on the next dirty scan. |
| Counter reaches 99999 | saturates at the 5-digit width — fine at this scale. |
| Re-persisting the same count | idempotent (same bytes); harmless. |

## Testing

**Host unit tests (`kb.rs`):** `set_seen_in_block` overwrites the digits and
`parse` then reads the new count; rejects an absent or non-5-digit field; `parse`
reads `seen`; `serialize` emits a `seen` that `parse` round-trips.

**Boot test (existing two-boot harness):** boot 1 asserts
`heal: persisted KB-0005 (seen 4)`; boot 2 asserts the count carried over and
grew — `heal: persisted KB-0005 (seen 8)` (and the shell `kb` line
`KB-0005 (seen 8)`). The drive is already writable and shared across the two
boots (Phase 7).

## What this proves / what's next

The organism's memory is now revisable in place and accumulates across reboots —
it can track *how often* it has seen an issue, not just *that* it has seen one.
Deferred: variable-length/growable records (a free-block allocator,
multi-block directories), escalation policy driven by the counter (e.g. flag a
chronically recurring fault), and record deletion/compaction.
