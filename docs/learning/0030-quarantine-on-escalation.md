# 0030 — Act on escalation: quarantine a chronic fault

**One-line:** the organism does the thing self-healing is *for* — it stops a
futile fix. When a crash diagnoses to an escalated (chronic) issue, the kernel
**quarantines** the component instead of restarting it. Recognize chronic (11) →
stop the futile fix (12).

## What changed
- One branch in the crash path (`exit_current`). After diagnosing and (Phase 11)
  bumping/escalating the issue, the kernel checks `issue.escalated()`; if so, it
  records the issue id as a `quarantine_id`.
- The existing Phase 5b "notify the healer to restart" block is now gated: if
  `quarantine_id` is set, the kernel logs
  `heal: 'flaky' quarantined (KB-0005 chronic) -- not restarting` and leaves the
  component down; otherwise it restarts exactly as before.
- The shell `kb` renders an escalated entry as `(seen N, escalated, quarantined)`.

## The idea worth keeping: the action needs no new persistence
Quarantine is the *behavioral consequence* of Phase 11's already-persisted
`escalated` flag — re-derived on each crash. There is **no new on-disk field, no
new task**. And it inherits Phase 11's decisive property: the escalation that
triggers quarantine only fires because boot 1's `seen` count carried over. Boot 2
*in isolation* would never reach the threshold, never escalate, never quarantine.
So the organism stops restarting `flaky` only because it *remembers across
reboots* that this fault is chronic — an **action driven by persistent memory**.

This is the whole point of the self-healing cage finally paying off across time:
Phase 5b's restart bound resets every boot, so without memory the OS would
restart-and-reflag `flaky` forever, learning nothing. Now it learns.

## A clean timing accident
In boot 2 the escalation fires at `flaky`'s **first** crash (`seen 6`), *after*
`transient` has already recovered (its crash was at `seen 5`, pre-escalation). So
Phase 5b's "transient recovers" proof is untouched, while `flaky`'s
"restart-to-bound-then-flag" becomes "quarantined as chronic." And because `flaky`
is no longer restarted, `seen` ends at 6 (not 8) — the counter itself reflects
that the futile retries stopped.

## Scope line
Per-**issue-class** quarantine (a crash whose issue is escalated is not
restarted), reusing the escalated flag. Per-**component** precision (quarantine
one specific component, not a whole fault class) needs a per-component crash
ledger — deferred, along with a human de-quarantine flow and skipping the launch
of a known-quarantined component at boot.

## Proof
Boot 1: `heal: restarted 'flaky' (attempt 1/2)` then `giving up on 'flaky' after
2 restarts` (normal — KB-0005 not yet escalated). Boot 2 of the same image:
`heal: KB-0005 escalated (seen 6) ...` then
`heal: 'flaky' quarantined (KB-0005 chronic) -- not restarting` — and **no**
`restarted 'flaky'` line. The action changed because the organism remembered.

## What's next
Per-component crash ledgers (precise quarantine); a human acknowledge /
de-quarantine flow; and not launching a known-quarantined component at boot at
all.
