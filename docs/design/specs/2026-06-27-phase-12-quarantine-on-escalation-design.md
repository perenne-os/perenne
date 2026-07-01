# Phase 12 — Act on escalation: quarantine a chronic fault (design)

**Status:** approved 2026-06-27 (user authorized completing the phase end-to-end)
**Priority served:** #2 (the self-healing organism). The organism now *acts* on
what it learned in Phase 11 — it stops a futile fix.

## The gap

Phase 11 made the organism *recognize* a chronically recurring fault (it
escalates an issue once its cross-boot `seen` count crosses a threshold). But it
still keeps applying the same futile fix: today (Phase 5b) the kernel notifies
the healer to restart a crashed component up to a per-boot bound, and that bound
resets every boot — so `flaky` is restarted and re-flagged *forever*, learning
nothing across reboots. Phase 12 closes the loop: when a crash diagnoses to an
**escalated** issue, the kernel **quarantines** the component — it suppresses the
restart and logs the decision. Detect → cage → … → count → escalate (recognize)
→ **quarantine (act)**.

## The idea (an action that requires cross-boot memory)

Quarantine is the *behavioral consequence* of Phase 11's persisted `escalated`
flag — re-derived on each crash, so **no new persistence is needed**. And it
inherits Phase 11's decisive property: the escalation that triggers quarantine
only fires because boot 1's `seen` carried over. Boot 2 *in isolation* would
never reach the threshold, never escalate, never quarantine. So the organism
stops restarting `flaky` only because it *remembers across reboots* that this
fault is chronic — an action driven by persistent memory.

A consequence of the existing timing makes the demo clean: in boot 2 the
escalation fires at `flaky`'s **first** crash (`seen 6`), **after** `transient`
has already recovered (its crash was at `seen 5`, pre-escalation). So Phase 5b's
"transient recovers" proof is untouched, while `flaky`'s
"restart-to-bound-then-flag" becomes "quarantined as chronic."

## Scope (YAGNI)

- **Per-issue-class** quarantine: a crash whose *diagnosed issue* is escalated is
  not restarted. Reuses the Phase 11 `escalated` flag; no new on-disk field, no
  new task.
- Quarantine = suppress the healer notification + log. The component stays down.
- Deferred: **per-component** crash ledgers (quarantine one specific component,
  not a whole fault class), a human de-quarantine/acknowledge flow, and skipping
  the launch of a known-quarantined component at boot.

## Architecture & components

### `arch/riscv64/src/sched.rs` — `exit_current` (the one behavioral change)

The `Killed(cause)` path already (5a) diagnoses, (11) bumps the counter and logs
escalation via `note_diagnosis`, then (5b) notifies a healer to restart any
component with a `relaunch`. Two small additions:

1. In the diagnosis `Some(issue)` arm, after `note_diagnosis`, check
   `issue.escalated()` (it reflects the just-updated state) and, if escalated,
   capture the issue id into a `quarantine_id: Option<&'static str>` (a
   `&'static str` into the runtime table — valid beyond the borrow).
2. In the existing healer-notify block (gated on `relaunch.is_some()`): if
   `quarantine_id` is set, **do not** notify the healer — log
   `heal: '<name>' quarantined (<id> chronic) -- not restarting` and leave the
   component `Exited`. Otherwise notify the healer to restart, exactly as today.

No other control flow changes. The healer simply receives no message for a
quarantined crash; it stays blocked on its recv (no deadlock — `idle` is the
net), ready for the next non-quarantined crash.

### `arch/riscv64/src/shell.rs` — `kb` (optional polish)

The `kb` command already shows `escalated`; render an escalated entry as
`(seen N, escalated, quarantined)` so a human sees the consequence (escalation
and quarantine are 1:1 in this design).

## Data flow (the proof)

- **Boot 1:** KB-0005 never escalates (`seen` maxes at 4 < `ESCALATE_AT` = 6) →
  `flaky` is restarted to the bound and flagged, exactly as today. No quarantine.
- **Boot 2** (same image, not rebuilt): loads `seen 4`; `transient` crashes
  (`seen 5`, not escalated) → restarted → recovers; `flaky` crash 1 (`seen 6`) →
  escalate **and** quarantine →
  `heal: 'flaky' quarantined (KB-0005 chronic) -- not restarting`. `flaky` is
  **not** restarted, so `seen` ends at **6** (not 8). The persist line becomes
  `heal: persisted KB-0005 (seen 6, escalated)`.

The proof the *action* changed: in boot 2 there is **no** `restarted 'flaky'`
line (present in boot 1) and **is** a quarantine line.

## Error handling / edge cases

| Situation | Behavior |
|---|---|
| Crash of a non-restartable component (`relaunch` is `None`) | Unaffected — quarantine only gates the restart path. |
| Escalated-issue crash | Healer not notified; component stays down; logged. |
| A boot that loads an already-escalated entry (e.g. boot 3) | Quarantines from the first matching crash (escalation persists). |
| Healer left with no message | Stays blocked on recv (no deadlock; `idle` is runnable). |

## Testing

**Boot test (existing two-boot harness):**
- Boot 1 unchanged — its `heal: restarted 'flaky' (attempt 1/2)` /
  `giving up on 'flaky' after 2 restarts` lines still hold (no escalation in
  boot 1).
- Boot 2 asserts `heal: 'flaky' quarantined (KB-0005 chronic) -- not restarting`,
  keeps `heal: KB-0005 escalated (seen 6) -- recurring; flag for triage`, and
  updates the persisted line from `(seen 8, escalated)` to `(seen 6, escalated)`
  (quarantine stops `flaky` after one crash).

No new pure host-tested units — the change is kernel control flow; the `escalated`
state it reads is already covered by the Phase 10/11 `heal`/`kb` tests.

## What this proves / what's next

The organism stops a futile fix based on what it learned across reboots — the
genuine self-healing payoff. Deferred: per-component crash ledgers (precise
per-component quarantine rather than per-issue-class), a human
acknowledge/de-quarantine flow, and not launching a known-quarantined component
at boot.
