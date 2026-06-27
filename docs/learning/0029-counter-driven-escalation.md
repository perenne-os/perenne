# 0029 — Counter-driven escalation: the organism flags a chronic fault

**One-line:** the self-healer does its first thing that depends on *accumulated
history*: when an issue's cross-boot `seen` count crosses a threshold, it
**escalates** the issue — latches an `escalated` flag, persists it in place, and
reports "chronic — flag for triage." The organism's first **adaptive** behavior.

## What changed
- A fixed-width `escalated: 0` flag in each entry's frontmatter, alongside `seen`.
  `kb::parse` reads it; `kb::serialize` emits it; a new `kb::set_escalated_in_block`
  (and `set_seen_in_block`) now share one generic fixed-width writer.
- `heal` gained `ESCALATE_AT = 6` and a per-entry `escalated` flag. On each
  diagnosis, `note_diagnosis` bumps `seen` and — if it crosses the threshold and
  isn't already escalated — latches `escalated` and returns the count so the
  crash path logs `heal: KB-0005 escalated (seen 6) -- recurring; flag for
  triage` once.
- The KB-writer persists `escalated` alongside `seen` in the *same* in-place
  block write (no extra I/O). The shell `kb` shows `KB-0005 (seen 8, escalated)`.

## The idea worth keeping: a decision that requires persistent memory
The point of the threshold is to make escalation **provably** depend on
cross-boot history. KB-0005 is diagnosed 4×/boot, and the threshold is **6** —
above one boot's count. So:

- **Boot 1** reaches `seen 4` < 6 → never escalates.
- **Boot 2** loads that 4 and crosses 6 → escalates.

Boot 2 *in isolation* would only reach 4 and never escalate. The escalation only
happens because Phase 10's counter carried over. That's the capstone of the
self-healing arc: detect → cage → read → learn → delegate → interrogate → count →
**escalate**. The organism's response now adapts to what it has lived through.

## Reused machinery (and a scope line)
Escalation needed almost no new plumbing — it's a *second* fixed-width field
riding Phase 10's in-place update path (generalized to any
`key: NNN…` field). And it deliberately changes only what the organism *knows
and says* (a latched status, surfaced in the diagnosis, the persisted entry, and
the shell), **not** the restart cage — Phase 5b's bounded-restart behavior is
untouched. Acting on escalation (suppressing futile restarts, quarantining a
component) is a deliberate next step, kept out of scope here.

## Proof
Boot 1: `heal: persisted KB-0005 (seen 4)` (not escalated). Boot 2 of the same
image: `heal: KB-0005 escalated (seen 6) -- recurring; flag for triage`, then
`heal: persisted KB-0005 (seen 8, escalated)`. A live `kb` after escalation shows
`KB-0005 (seen 8, escalated)`.

## What's next
Act on escalation (suppress the futile bounded-restart / quarantine a chronically
crashing component); per-component (not just per-issue-class) tracking; and a
human acknowledge / de-escalate flow.
