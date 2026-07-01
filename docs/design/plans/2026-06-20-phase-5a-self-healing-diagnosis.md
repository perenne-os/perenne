# Phase 5a: Self-healing — detect + deterministic diagnosis — Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When the microkernel contains a crashed component, consult a deterministic, host-tested rule engine over compiled-in knowledge and log the matched diagnosis + playbook — the first runtime cell of the self-healing knowledge organism (ADR 0005). No action yet.

**Architecture:** A pure `heal` module maps a fault `Cause` to a compiled-in `KnownIssue` (mirroring `knowledge-base/entries/KB-0005.md`). The existing `exit_current(Killed{cause})` containment path calls it and logs the diagnosis. A `flaky` U-mode component deliberately faults so the organism has something to diagnose. The kernel and other components keep running.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU. Host tests: `cargo test -p kernel-arch-riscv64`. Bare: `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/design/specs/2026-06-20-phase-5a-self-healing-diagnosis-design.md`

**Grounding facts (verified):** `Cause` (in `trap.rs`) derives `Debug/Clone/Copy/PartialEq/Eq` with variants `Breakpoint`, `SupervisorTimer`, `InstructionPageFault`, `LoadPageFault`, `StorePageFault`, `UserEcall`, `Unknown{..}`. `exit_current`'s `Killed(cause)` arm (sched.rs ~line 357) prints `sched: task '<name>' killed by <cause:?>`. `MAX_TASKS` = 6 (room for a 5th task + idle).

---

## Task 1: the deterministic rule engine + the knowledge entry

**Files:**
- Create: `arch/riscv64/src/heal.rs`
- Modify: `arch/riscv64/src/lib.rs` (declare the module)
- Create: `knowledge-base/entries/KB-0005.md`
- Modify: `knowledge-base/README.md` (layout list)

- [ ] **Step 1: Create `arch/riscv64/src/heal.rs` with the rule engine + tests**

```rust
//! The self-healing knowledge organism — deterministic core (Phase 5a).
//!
//! When the microkernel contains a crashed component
//! (`sched::exit_current(Killed{cause})`), it consults this module to
//! DIAGNOSE the crash: match the fault against a compiled-in knowledge base
//! and return the known issue + its fix playbook. The match is a pure,
//! explainable table lookup — never a black box — which is why it is safe in
//! the kernel (ADR 0005). 5a only diagnoses; the caged, isolated, user-space
//! healer that *acts* on the playbook is Phase 5b.
//!
//! The compiled records here are the machine-readable subset of the human
//! knowledge base (`knowledge-base/entries/`); a real loader awaits a
//! filesystem.

use crate::trap::Cause;

/// A compiled-in knowledge record — the runtime subset of a
/// `knowledge-base/entries/*.md` issue record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownIssue {
    pub id: &'static str,
    pub title: &'static str,
    pub playbook: &'static str,
}

/// KB-0005: a user-space component terminated by a fatal fault. Mirrors
/// `knowledge-base/entries/KB-0005.md`.
static KB_0005: KnownIssue = KnownIssue {
    id: "KB-0005",
    title: "user-space component terminated by a fatal fault",
    playbook: "restart the component (bounded retries); if it keeps crashing, stop and flag for triage",
};

/// Diagnose a contained crash by matching its `cause` to a known issue.
/// Deterministic and total over `Cause` (returns `None` for non-crash
/// causes). Pure — host-tested, explainable, no allocation.
pub fn diagnose(cause: Cause) -> Option<&'static KnownIssue> {
    match cause {
        Cause::LoadPageFault | Cause::StorePageFault | Cause::InstructionPageFault => Some(&KB_0005),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnoses_a_fatal_fault_as_kb_0005() {
        assert_eq!(diagnose(Cause::LoadPageFault).map(|i| i.id), Some("KB-0005"));
        assert_eq!(diagnose(Cause::StorePageFault).map(|i| i.id), Some("KB-0005"));
        assert_eq!(diagnose(Cause::InstructionPageFault).map(|i| i.id), Some("KB-0005"));
    }

    #[test]
    fn no_diagnosis_for_a_non_crash_cause() {
        assert!(diagnose(Cause::Breakpoint).is_none());
        assert!(diagnose(Cause::SupervisorTimer).is_none());
        assert!(diagnose(Cause::Unknown { interrupt: false, code: 2 }).is_none());
    }
}
```

- [ ] **Step 2: Declare the module in `lib.rs`**

In `arch/riscv64/src/lib.rs`, add after the `cap` module declaration:

```rust
/// The self-healing knowledge organism: the deterministic, host-tested rule
/// engine that diagnoses a contained crash against compiled-in knowledge
/// (Phase 5a — diagnosis only; the caged healer that acts is 5b).
pub mod heal;
```

- [ ] **Step 3: Create the human knowledge entry `knowledge-base/entries/KB-0005.md`**

```markdown
---
id: KB-0005
title: "User-space component terminated by a fatal fault"
status: diagnosed
severity: medium
component: user-space
symptoms:
  - "A user-space component is killed by a load/store/instruction page fault — it touched memory it does not own."
  - "The kernel logs: 'sched: task '<name>' killed by LoadPageFault' (or Store/InstructionPageFault)."
diagnosis: >
  A bug or fault in the component made it access memory outside its address
  space. The microkernel contained the fault: it terminated just that
  component, while the kernel and every other component kept running (per-
  address-space isolation + capability confinement). This is the expected,
  safe outcome of a component crash — not a kernel failure.
playbook:
  - "Restart the component, up to a bounded number of retries."
  - "If it keeps crashing (exceeds the retry bound), stop restarting and flag it for triage — repeated restarts of a deterministically-crashing component are futile."
  - "Reversible by construction: the restart bound stops the action; a human can inspect the log and the (unchanged) component image."
verification: "After a restart, the component runs again and serves requests (e.g. resumes responding on its endpoint)."
created: 2026-06-20
updated: 2026-06-20
references:
  - "docs/decisions/0005-self-healing-knowledge-organism.md"
  - "docs/design/specs/2026-06-20-phase-5a-self-healing-diagnosis-design.md"
  - "knowledge-base/schema/issue-record.md"
---

## Notes

The first **runtime** knowledge entry (KB-0001..0004 are dev-environment
issues). It is the machine-consultable record behind the self-healing
organism's first diagnosis: Phase 5a's `arch/riscv64/src/heal.rs` carries a
compiled subset of this entry (`KB_0005`) and matches a contained crash to
it, logging the playbook. Phase 5b adds an isolated, capability-caged
user-space healer that *applies* the playbook (the bounded restart). Loading
this `.md` at runtime (rather than compiling a subset) awaits a filesystem.
```

- [ ] **Step 4: Add KB-0005 to the knowledge-base README layout**

In `knowledge-base/README.md`, in the `entries/` part of the Layout block,
add the line (after the `KB-0004.md` line):

```
    └── KB-0005.md         user-space component killed by a fault (first runtime entry)
```

(Adjust the tree connectors so `KB-0005.md` is the last entry — make the
former `KB-0004.md └──` into `├──` and `KB-0005.md` the `└──`.)

- [ ] **Step 5: Run the tests**

Run: `cargo test -p kernel-arch-riscv64 heal::`
Expected: PASS — `diagnoses_a_fatal_fault_as_kb_0005`, `no_diagnosis_for_a_non_crash_cause`.
Then `cargo test -p kernel-arch-riscv64` → all green; `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf` → SUCCESS.

- [ ] **Step 6: Commit**

```bash
git add arch/riscv64/src/heal.rs arch/riscv64/src/lib.rs knowledge-base/entries/KB-0005.md knowledge-base/README.md
git commit -m "feat(heal): deterministic crash-diagnosis rule engine + KB-0005 (host-tested)"
```

---

## Task 2: update the smoke test for the diagnosis milestone (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Add the crash + diagnosis patterns**

In `tools/test-qemu.ps1`, add to the `$mustMatch` array (just before
`"console: ns16550a @ 0x10000000"`):

```powershell
    "sched: task 'flaky' killed by LoadPageFault",
    "heal: diagnosed KB-0005",
```

Update the header comment and PASS message to mention the 5a milestone:
"the first cell of the self-healing knowledge organism (Phase 5a) — a
contained component crash is detected and deterministically diagnosed
against the knowledge base (matched to KB-0005, with its fix playbook)."

- [ ] **Step 2: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — there is no `flaky` component yet, and `exit_current` does
not call the diagnosis, so both new lines are absent.

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts self-healing diagnosis of a contained crash (red)"
```

---

## Task 3: wire the diagnosis into `exit_current` + add the `flaky` component (green)

**Files:**
- Modify: `arch/riscv64/src/sched.rs`
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Diagnose on containment in `exit_current`**

In `arch/riscv64/src/sched.rs`, the `Killed` arm of `exit_current` currently
reads:

```rust
            ExitReason::Killed(cause) => {
                crate::println!("sched: task '{}' killed by {cause:?}", s.tasks[current].as_ref().unwrap().name)
            }
```

Change it to also consult the knowledge organism and log the diagnosis:

```rust
            ExitReason::Killed(cause) => {
                crate::println!("sched: task '{}' killed by {cause:?}", s.tasks[current].as_ref().unwrap().name);
                // Phase 5a: consult the deterministic knowledge organism and
                // log the diagnosis (no action yet — the caged restart is 5b).
                match crate::heal::diagnose(cause) {
                    Some(issue) => crate::println!(
                        "heal: diagnosed {} ({}) -> playbook: {}",
                        issue.id, issue.title, issue.playbook
                    ),
                    None => crate::println!("heal: no known issue for {cause:?} (recorded for triage)"),
                }
            }
```

(Containment is unchanged — only an explainable diagnosis is added to the log.)

- [ ] **Step 2: Add the `flaky` component's stacks**

In `kernel/src/main.rs`, add a kernel stack and a user stack for `flaky`
alongside the existing ones. Add to the `KS_*` block:

```rust
    static mut KS_FLAKY: KStack = [0; TASK_STACK];
```

and to the `US_*` block:

```rust
    #[link_section = ".user_data"]
    static mut US_FLAKY: UStack = UStack([0; USER_STACK_SIZE]);
```

- [ ] **Step 3: Add the `flaky` task function**

In `kernel/src/main.rs`, add (e.g. just after `rogue_task`):

```rust
    /// A deliberately faulty component: it reads a kernel address it does not
    /// own, faults (LoadPageFault), and is contained — the "patient" the
    /// self-healing organism diagnoses. Inline asm (not read_volatile) keeps
    /// the load in `.user_text` (a U-mode task can't call kernel code).
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn flaky_task() -> ! {
        let _v: u8;
        // SAFETY: the deliberate fault. 0x80200000 is the kernel .text base
        // (no U bit); the U-mode load faults before completing and the kernel
        // contains this component. Control never returns here.
        unsafe {
            core::arch::asm!(
                "lb {v}, 0({p})",
                v = out(reg) _v,
                p = in(reg) 0x8020_0000usize,
                options(nostack),
            );
            sys_exit(0) // unreachable: the load faults first
        }
    }
```

- [ ] **Step 4: Spawn `flaky` in `kmain`**

In `kernel/src/main.rs`, in the spawn block, add `flaky` after `rogue` and
before `idle` (it needs no endpoint cap and no device — just its own AS):

```rust
        // A deliberately faulty component, to exercise self-healing: it
        // crashes (is contained), and the kernel diagnoses the crash (5a).
        let fu = ustack(core::ptr::addr_of!(US_FLAKY) as usize);
        let _flaky = sched::spawn_user("flaky", flaky_task, fu.1,
            core::ptr::addr_of!(KS_FLAKY) as usize + TASK_STACK,
            mem::build_user_space(fu, NO_DEVICE));

        sched::spawn("idle", idle, core::ptr::addr_of!(KS_IDLE) as usize + TASK_STACK);
```

(Place the two `flaky` lines immediately before the existing
`sched::spawn("idle", ...)` line.)

- [ ] **Step 5: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 6: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS` including `sched: task 'flaky' killed by LoadPageFault`
and `heal: diagnosed KB-0005 (…) -> playbook: restart …`, with the RTC
component, ticks, and all prior milestones still present (the system kept
running after the contained crash). If `flaky` faults with
`InstructionPageFault` instead of `LoadPageFault`, the load became a call —
ensure the read is inline asm `lb` (it is); diagnose, don't weaken the test.

- [ ] **Step 7: Commit**

```bash
git add arch/riscv64/src/sched.rs kernel/src/main.rs
git commit -m "feat: Phase 5a live - the kernel diagnoses a contained component crash against the knowledge base"
```

---

## Task 4: docs — learning note, roadmap, glossary; final verification

**Files:**
- Create: `docs/learning/0014-self-healing-diagnosis.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md`

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0014-self-healing-diagnosis.md`:

```markdown
# 0014 — Self-healing, step one: diagnosis (Phase 5a)

**One-line:** when the kernel contains a crashed component, it now *consults
itself* — matching the crash to a known issue and logging the fix playbook.

## What changed
- New `arch/riscv64/src/heal.rs`: a pure, host-tested rule engine.
  `diagnose(cause)` maps a fatal fault to a compiled-in knowledge record
  (`KB-0005`, mirroring `knowledge-base/entries/KB-0005.md`).
- `exit_current`'s containment path (where a faulting component is already
  terminated) now also calls `heal::diagnose` and logs the diagnosis +
  playbook. A `flaky` component crashes on purpose to exercise it.

## The point (ADR 0005)
This is the first runtime cell of the "knowledge organism": detect → consult
the knowledge base. It is deliberately **diagnosis only**, and deliberately
**deterministic** — a transparent table lookup, not a black box — which is
why it is safe in the kernel. The part that *acts* (a bounded, reversible,
capability-caged restart) is Phase 5b, and it moves into an isolated
user-space healer, because an agent with the power to act is exactly what
must be confined.

## Why diagnosis before action
"Deterministic core first": prove the OS can recognize and explain a problem
before you give anything the authority to change the system in response. The
matching is host-tested and auditable; the action will be caged.

## Proof
`flaky` touches memory it doesn't own → `sched: task 'flaky' killed by
LoadPageFault` (contained) → `heal: diagnosed KB-0005 … -> playbook: restart
…`, while the RTC component and the heartbeat keep running. Next: 5b applies
the (caged) fix.
```

- [ ] **Step 2: Update the roadmap**

In `docs/roadmap/roadmap.md`, replace the `## Phase 5 — Self-healing seed`
block with:

```markdown
## Phase 5 — Self-healing seed

The soul of the project ([ADR 0005](../decisions/0005-self-healing-knowledge-organism.md)):
the OS diagnoses and fixes its own issues, deterministically and inside a
safety cage. Decomposed (2026-06-20) in the trust-preserving order —
diagnose before act.

### Phase 5a — Detect + deterministic diagnosis  *(done — 2026-06-20)*

- **Goal:** when the kernel contains a crashed component, match the crash to
  a known issue (a compiled-in knowledge record) and log the diagnosis +
  playbook. No action.
- **You learn:** the containment path is the detection point; a deterministic,
  host-tested rule engine turns a fault into an explainable diagnosis (see
  [learning note 0014](../learning/0014-self-healing-diagnosis.md)).
- **Done when:** `./tools/test-qemu.ps1` shows a deliberately faulty
  component contained and diagnosed (matched to KB-0005), with the rest of
  the system running on. QEMU-only.

### Phase 5b — The caged fix

- **Goal:** an isolated, capability-gated **user-space** healer that the
  kernel notifies of a crash and that applies the playbook — a **bounded,
  reversible, logged** restart — recovering the component.
- **Done when:** a crashing component is automatically restarted by the
  healer and resumes working, with the restart bounded (it gives up and
  flags after N attempts).
```

- [ ] **Step 3: Add glossary entries**

In `docs/glossary.md`, add entries (in the file's format) for: **self-healing
knowledge organism** (the OS's growing, machine- and human-readable memory of
issues and proven fix playbooks, consulted to diagnose and repair itself
instead of depending on a human community — ADR 0005), **playbook** (an
ordered, reversible set of fix steps recorded for a known issue), **diagnosis
(rule engine)** (the deterministic, explainable matching of a symptom to a
known issue — never a black box; in this kernel, `heal::diagnose`), and
**safety cage** (the rule that every automated healing action is
capability-checked, logged, reversible, and auditable, so the healer can
never gain unchecked power). Reuse existing capability/component terms.

- [ ] **Step 4: Update the learning-notes index**

In `docs/learning/README.md`, add:

```markdown
- [0014 — Self-healing, step one: diagnosis (Phase 5a)](0014-self-healing-diagnosis.md)
```

- [ ] **Step 5: Final verification (run all, paste results)**

Run: `cargo test -p kernel-arch-riscv64` → expect all green.
Run: `cargo build --workspace` → expect success.
Run (PowerShell): `./tools/check-references.ps1` → expect PASS (the new KB-0005, learning note, and roadmap links resolve; fix any of YOUR broken references).
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0014-self-healing-diagnosis.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: Phase 5a learning note, roadmap (Phase 5 decomposed), glossary"
```

---

## Done-when checklist (maps to spec §1)

- [ ] A contained crash is detected and diagnosed — smoke patterns `sched: task 'flaky' killed by LoadPageFault` and `heal: diagnosed KB-0005`, with the system still running (RTC, ticks, etc.).
- [ ] Host test — `heal::diagnose(LoadPageFault) == Some(KB-0005)`; a non-crash cause returns `None`.
- [ ] `check-references` clean (KB-0005 + note resolve); `cargo build --workspace` green; `BOOT TEST PASS`.
```
