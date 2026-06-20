# Kernel — Phase 5a Design: Self-healing — detect + deterministic diagnosis

- **Date:** 2026-06-20
- **Status:** Draft — awaiting review
- **Scope of this document:** Phase 5a only — the first runtime cell of the
  self-healing "knowledge organism": when the microkernel *contains* a
  crashed component, consult a deterministic, explainable rule engine over a
  compiled-in knowledge base, match the crash to a known issue, and **log
  the diagnosis**. No healing action yet (that is 5b). Fully QEMU-testable.

---

## 0. Where 5a sits

The self-healing knowledge organism is the project's #2 priority and
"defining feature" ([north-star](../vision/north-star.md),
[ADR 0005](../decisions/0005-self-healing-knowledge-organism.md),
[architecture/self-healing.md](../architecture/self-healing.md)): the OS
diagnoses and fixes its *own* issues instead of depending on a human
community, with a **deterministic, explainable core** and every action in a
**safety cage** (capability-checked, logged, reversible, auditable). Phase 0
seeded only the schema + store (`knowledge-base/`); Phase 5 builds the
runtime logic.

Phase 5 is decomposed (2026-06-20), in the order that preserves trust
("deterministic core first"):

- **5a (this doc) — detect + diagnose.** Recognize a contained component
  crash and match it, deterministically, to a known issue + its playbook;
  log it. No action.
- **5b — the caged fix.** An isolated, capability-gated **user-space**
  healer that applies the playbook — a **bounded, reversible, logged**
  restart — recovering the crashed component.

5a builds directly on what exists: components (the RTC driver), and the
microkernel's containment of faults (`exit_current(Killed{cause})` — a
faulting component is terminated while the kernel and other components keep
running). ADR 0002 names restartable user-space components "the natural
foundation for self-healing"; 5a is its first runtime expression.

## 1. Goal

When a U-mode component is contained after a fatal fault, the OS **consults
itself first**: a pure rule engine matches the crash's cause against a
compiled-in knowledge base and logs the matched issue and its fix playbook
(or notes an unknown issue for triage). This is steps 1–2 of the
self-healing loop (detect → consult). The diagnosis is **deterministic and
explainable** — a table lookup, never a black box — so it is safe even
though it runs in the kernel; the part that *acts* (and therefore must be
isolated and caged) is deferred to 5b.

**You learn (kept brief):** how the kernel's existing fault-containment path
is the detection point for self-healing, and how a deterministic,
host-testable rule engine turns a raw fault into an explainable diagnosis
matched to a knowledge-base entry — the trustworthy core that any later
(caged, or even AI-advised) healing must sit on top of.

**Done when** `./tools/test-qemu.ps1` observes, alongside every existing
milestone (2a/2b, console, dt, pqc, the RTC component, ticks):

1. **A contained crash is detected and diagnosed** — a deliberately faulty
   `flaky` component touches memory it does not own, is contained
   (`sched: task 'flaky' killed by LoadPageFault`), and the OS logs a
   deterministic diagnosis: `heal: diagnosed KB-0005 (…) → playbook: restart
   …`. The kernel and the other components keep running.

And off the bare target:

2. **Host unit test** — `heal::diagnose(LoadPageFault) == Some(KB-0005)`
   (and the other fault causes), while a non-crash cause
   (`diagnose(Breakpoint)`) returns `None`.

## 2. Non-goals (deferred)

- **Applying the fix / restarting the component** — 5a only diagnoses. The
  caged restart is **5b**.
- **The healer as an isolated user-space component** — 5a's diagnosis is a
  pure, kernel-side table lookup (safe: deterministic, tiny, host-tested).
  The *acting* healer — the part that gains agency and must be isolated and
  capability-caged — becomes a user-space component in **5b**.
- **Loading `knowledge-base/*.md` at runtime** — there is no filesystem yet,
  so the runtime knowledge is a **compiled-in subset**; the Markdown KB
  stays the human source of truth, and a real loader awaits a future FS
  phase.
- **Persistently recording new (unknown) issues** — 5a logs an unknown
  cause as "recorded for triage" but cannot persist it without an FS; humans
  still author KB entries (as today).
- **An AI advisor** — deferred indefinitely (ADR 0005), and never in the
  kernel; the deterministic core is never replaced by a black box.
- **Rich rules / multiple issues** — one rule (fatal fault → restart
  playbook) is enough to prove the mechanism; more accrue as real runtime
  issues are recorded.

## 3. Design

### 3.1 Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| `KnownIssue`, `diagnose`, compiled KB | `arch/riscv64/src/heal.rs` *(new)* | Pure (host-tested): `KnownIssue { id, title, playbook }`; a compiled-in record `KB-0005`; `diagnose(cause: trap::Cause) -> Option<&'static KnownIssue>` mapping a fatal fault to a known issue. |
| Detection hook | `arch/riscv64/src/sched.rs` | In `exit_current`'s `Killed(cause)` arm, after the existing containment line, call `heal::diagnose` and log the diagnosis (matched issue + playbook, or "no known issue — recorded for triage"). |
| `flaky` component | `kernel/src/main.rs` | A U-mode component that deliberately faults (inline-asm load of a kernel address → `LoadPageFault`), so it is contained and diagnosed. |
| Human KB entry | `knowledge-base/entries/KB-0005.md` | The schema-conformant record the runtime rule mirrors (single source of truth). |

### 3.2 The deterministic rule engine and compiled knowledge

`heal.rs` holds a small, machine-readable subset of the knowledge base —
the runtime form of `knowledge-base/entries/KB-0005.md` — plus the matcher:

```
pub struct KnownIssue { id: &'static str, title: &'static str, playbook: &'static str }

static KB_0005: KnownIssue = {
    id: "KB-0005",
    title: "user-space component terminated by a fatal fault",
    playbook: "restart the component (bounded retries); if it keeps crashing, stop and flag for triage",
};

pub fn diagnose(cause: Cause) -> Option<&'static KnownIssue> {
    match cause {
        LoadPageFault | StorePageFault | InstructionPageFault => Some(&KB_0005),
        _ => None,
    }
}
```

It is **pure and explainable** — a deterministic `match`, host-tested — which
is exactly why it is acceptable in the kernel (ADR 0005's hard rule forbids
AI/black boxes in the kernel, not a transparent table lookup). The compiled
record mirrors the human KB entry; when a filesystem exists, the runtime can
load the real store instead of carrying a subset.

`Cause` (from `trap.rs`) is the symptom key. In practice `exit_current` is
only called with the fatal-fault causes, but `diagnose` is defined over all
of `Cause` (returning `None` for non-crash causes) so it is fully
host-testable and robust.

### 3.3 The detection hook

`exit_current(Killed(cause))` is the microkernel's containment point — it
already prints `sched: task '<name>' killed by <cause>`. 5a adds, right
after that line, the diagnosis:

```
match heal::diagnose(cause) {
    Some(issue) => println!("heal: diagnosed {} ({}) -> playbook: {}", issue.id, issue.title, issue.playbook),
    None        => println!("heal: no known issue for {:?} (recorded for triage)", cause),
}
```

No control-flow change: the component is still contained exactly as before;
we only add an explainable diagnosis to the log. (5b will, instead of just
logging, route this to a user-space healer that acts.)

### 3.4 The demo — a `flaky` component

The demo adds one U-mode component, `flaky`, that deliberately faults so the
healer has something to diagnose. Like the earlier `user_bad`/`snoop`
probes, it performs an **inline-asm** load of a kernel address (U-mode
codegen rules: no `core` fn calls, no `.rodata`), which faults as
`LoadPageFault`; the kernel contains it and `exit_current` diagnoses it.
`flaky` is spawned alongside the RTC `rtc`/`client`/`rogue`/`idle` cast
(e.g. as a later slot, so it crashes after the RTC demo has run); each task
in its own address space (3b-ii). The RTC component, ticks, and the rest
keep running — proving containment + diagnosis without disruption.

### 3.5 Error handling summary

| Situation | Behavior |
|-----------|----------|
| Component killed by a fatal fault | Contained (unchanged) **and** diagnosed: `heal: diagnosed KB-0005 …`. |
| Component killed by a cause with no known issue | Contained **and** logged: `heal: no known issue for <cause> (recorded for triage)`. |
| Clean `exit(code)` | No diagnosis (only `Killed` is a "crash"); unchanged. |
| Diagnosis itself | Pure, total over `Cause`, cannot fault; no kernel risk. |

## 4. Testing

- **Host unit tests** (`arch/riscv64`, `cargo test`): `heal::diagnose`
  returns `Some(KB-0005)` for `LoadPageFault`/`StorePageFault`/
  `InstructionPageFault`, and `None` for a non-crash cause (e.g.
  `Breakpoint`); the returned issue's `id` is `"KB-0005"`.
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): add `sched: task 'flaky' killed by LoadPageFault` and
  `heal: diagnosed KB-0005`; keep every existing milestone (the system keeps
  running after the contained crash).

## 5. Deliverables

1. `arch/riscv64/src/heal.rs` (new): `KnownIssue`, the compiled `KB-0005`,
   `diagnose`, host tests; module declared in `lib.rs`.
2. `sched.rs`: the diagnosis log in `exit_current`'s `Killed` arm.
3. `kernel/src/main.rs`: the `flaky` component + its stacks + spawn wiring.
4. `knowledge-base/entries/KB-0005.md`: the human-readable runtime issue
   record (per the schema), which the compiled rule mirrors; update
   `knowledge-base/README.md`'s layout list.
5. Extended QEMU smoke test + host unit tests, all green.
6. Short learning note `docs/learning/0014-self-healing-diagnosis.md`.
7. Roadmap: Phase 5 decomposed (5a/5b); 5a marked done with date.
8. Glossary: knowledge organism, playbook, diagnosis, safety cage — only
   genuinely new terms.

## 6. Open questions (for later sub-phases)

- **5b — the caged fix:** an isolated user-space healer that the kernel
  notifies of a death, holding a capability-gated, **bounded/reversible**
  restart; the crashed component recovers. Needs kernel→component
  notification and a restart capability.
- **Recording new issues at runtime:** persisting an unknown fault as a new
  KB entry — needs a filesystem and a write path (and the cage).
- **Loading the real `knowledge-base/` store** once an FS exists, retiring
  the compiled-in subset.
- **Richer symptoms:** beyond fault cause — resource exhaustion, failed
  health checks, repeated crashes (crash-loop detection feeds 5b's bound).
- **The AI advisor (far future):** an isolated model that *suggests*
  diagnoses for the deterministic engine or a human to approve — never
  acting on its own, never in the kernel.
