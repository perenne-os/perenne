# Phase 11 — Counter-driven escalation: the organism flags a chronic fault (design)

**Status:** approved 2026-06-27 (user authorized completing the phase end-to-end)
**Priority served:** #2 (the self-healing organism). The organism's first
**adaptive** behavior — a decision driven by accumulated cross-boot history.

## The gap

Phase 10 gave the organism a persistent recurrence counter, but it does nothing
with it — `seen` just grows. Phase 11 makes the organism *act* on the count: when
an issue's cross-boot `seen` crosses a threshold, it **escalates** the issue —
latches an `escalated` flag, persists that decision in place, and surfaces
"chronic — flag for triage" wherever it reports (the crash diagnosis, the boot
loader, the shell). This is the first time the organism's response depends on
*history* rather than only the current fault.

## The idea (escalation that requires cross-boot memory)

The threshold is set so escalation can **only** happen because of persistent
memory. KB-0005 is diagnosed 4×/boot (transient once, flaky three times across
its bounded restarts), and `ESCALATE_AT = 6`:

- **Boot 1:** `seen` reaches 4 — below 6, **not** escalated.
- **Boot 2:** loads that 4; the 2nd diagnosis brings `seen` to 6 → **escalates**.

Boot 2 *alone* would only reach 4 and never escalate. So the escalation is
provably a consequence of the cross-boot counter Phase 10 built — the capstone of
the self-healing arc (detect → cage → read → learn → delegate → interrogate →
count → **escalate**).

It reuses Phase 10's in-place machinery: `escalated` is a second fixed-width
field (`escalated: 0`) flipped to `1` and written in the **same block** as
`seen` — no extra disk writes, no new persistence mechanism.

## Scope (YAGNI)

- Escalation changes the organism's **explanation and persisted knowledge** (a
  latched status), **not** the restart cage. Phase 5b's bounded-restart behavior
  is unchanged — escalation is about what the organism *knows/says*, not a
  control-flow change.
- No restart suppression, no per-component quarantine, no human
  acknowledge/de-escalate flow (all noted as future).
- Escalation is **latched and monotonic** (`seen` only grows), so once set it
  stays set; re-persisting is idempotent.

## Architecture & components

### Pure, host-tested logic (`libs/common/src/kb.rs`)

- **`KbRecord`** gains `escalated: bool` — `parse` reads an `escalated: 0/1`
  line (default `false` if absent); `serialize` emits `escalated: 0`.
- **In-place writer, generalized.** Factor the fixed-width digit overwrite into a
  shared private helper; `set_seen_in_block` (width 5) keeps its signature, and a
  new **`set_escalated_in_block(block: &mut [u8], escalated: bool) -> bool`**
  (width 1, writes `0`/`1`) is added. Both return `false` on an absent/malformed
  field (the in-place guard) and round-trip with `parse`.

### Schema/data

- `knowledge-base/entries/KB-0005.md` gains `escalated: 0` near `seen` (in the
  entry's **first block**). `knowledge-base/schema/issue-record.md` documents it
  as a latched, threshold-driven status the runtime self-healer sets in place
  once `seen` crosses the escalation threshold (Phase 11).

### Kernel (`arch/riscv64/src/heal.rs`)

- **`KnownIssue`** gains `escalated: bool`; **`const ESCALATE_AT: u32 = 6`**.
- **`install`** takes `escalated` and stores it.
- **`note_diagnosis`** (crash path, interrupts off): after bumping `seen`, if
  `seen >= ESCALATE_AT && !escalated`, set `escalated = true` (the entry is
  already `dirty` from the `seen` bump) and **return a "just escalated" signal**
  (e.g. `-> bool`) so the crash path can log the one-time escalation event.
- **`dirty_entry`** returns `escalated` too (so the persister writes both
  fields); **`entry`** returns it (for the shell).

### Kernel (crash path + writer, `arch/riscv64/src/sched.rs`, `kernel/src/main.rs`)

- **`exit_current`**: when `note_diagnosis` reports a just-escalated issue, log
  `heal: KB-0005 escalated (seen 6) — recurring; flag for triage`. Restart
  control flow is unchanged.
- The **KB-writer task** persists `escalated` alongside `seen` in the **same**
  in-place block write (`set_seen_in_block` then `set_escalated_in_block` on the
  block, then one `fs_write_block`).
- The **loader** reads `escalated` and passes it to `install`.

### Shell (`arch/riscv64/src/shell.rs`)

- `kb` shows escalation: `KB-0005 (seen 8, escalated)  …` (and just
  `(seen N)` when not escalated).

## Data flow (the proof)

**Boot 1:** `seen` 0→4, `escalated` stays 0; persisted (`seen 4, escalated 0`).
**Boot 2** (same image, not rebuilt): loads `seen=4, escalated=0`; the 2nd
diagnosis brings `seen` to 6 → escalates → `heal: KB-0005 escalated (seen 6) …`;
persists `escalated=1`; ends at `seen 8`. The shell `kb` shows
`KB-0005 (seen 8, escalated)`.

## Error handling

| Situation | Behavior |
|---|---|
| `escalated:` field absent/malformed | `set_escalated_in_block` → `false`; logged, no write; entry stays valid. |
| Re-persisting an already-escalated entry | idempotent (same bytes). |
| Persist targeting block 0 | guarded out (`start_block == 0` skip, from Phase 10). |
| Device write error | logged; in-RAM state correct, retried on the next dirty scan. |

## Testing

**Host unit tests (`kb.rs`):** `parse` reads `escalated`; `serialize` emits a
parseable `escalated`; `set_escalated_in_block` overwrites + round-trips +
rejects an absent/malformed field. **(`heal.rs`):** crossing `ESCALATE_AT` flips
`escalated` and `note_diagnosis` reports it once (a unit test over the table).

**Boot test (existing two-boot harness):** boot 1 ends at `seen 4` with no
escalation; boot 2 asserts `heal: KB-0005 escalated (seen 6)` and the shell `kb`
line containing `escalated` — the cross-boot threshold crossing. The drive is
shared/not-rebuilt across the two boots (Phase 7).

## What this proves / what's next

The organism adapts its behavior to accumulated history: it recognizes a
chronically recurring fault and escalates it, a decision that requires the
cross-boot memory of Phase 10. Deferred: acting on escalation (suppress futile
restarts / quarantine a component), per-component (not just per-issue-class)
tracking, and a human acknowledge/de-escalate flow.
