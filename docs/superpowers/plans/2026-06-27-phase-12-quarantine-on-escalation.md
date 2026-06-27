# Phase 12 — Quarantine on escalation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When a crash diagnoses to an escalated (chronic) issue, the kernel quarantines the component — it suppresses the futile restart and logs the decision — instead of notifying the healer.

**Architecture:** A single quarantine branch in `exit_current`'s crash path, gated on Phase 11's persisted `escalated` flag. No new persistence, no new task — quarantine is the behavioral consequence of the escalation, re-derived per crash.

**Tech Stack:** Rust `no_std` kernel (`arch/riscv64`), QEMU riscv64, PowerShell two-boot harness.

**Spec:** `docs/superpowers/specs/2026-06-27-phase-12-quarantine-on-escalation-design.md`

## Global Constraints

- **Commits:** Conventional Commits, NO Claude co-author; author Kathir (signing automated).
- **Build:** `./tools/build.ps1`. **Boot test:** `./tools/test-qemu.ps1`. **Arch host tests:** `cargo test -p kernel-arch-riscv64`.
- The change is in the interrupts-off crash path (`exit_current`) — no I/O, no blocking; it only chooses whether to notify the healer.
- Restart control flow for **non-escalated** issues is unchanged (Phase 5b preserved).

---

## File Structure

- `arch/riscv64/src/sched.rs` — `exit_current`: capture `quarantine_id` from the escalated issue; gate the healer-notify on it.
- `arch/riscv64/src/shell.rs` — `kb` shows `quarantined` for escalated entries (polish).
- `tools/test-qemu.ps1` — boot-2 quarantine assertion + updated persisted-seen value.
- `docs/learning/0030-quarantine-on-escalation.md`, `docs/learning/README.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md` — docs.

---

## Task 1: Quarantine branch in `exit_current`

**Files:**
- Modify: `arch/riscv64/src/sched.rs` (`exit_current` `Killed` path)
- Test: boot test (Task 3).

- [ ] **Step 1: Capture `quarantine_id` in the diagnosis arm**

In `exit_current`, just before the `match crate::heal::diagnose(cause)`, declare:

```rust
                let mut quarantine_id: Option<&'static str> = None;
```

In the `Some(issue)` arm, after the `note_diagnosis` block, add the escalated
check:

```rust
                        // Phase 12: a crash whose issue is now escalated
                        // (chronic) is quarantined — we will not restart it.
                        if issue.escalated() {
                            quarantine_id = Some(issue.id());
                        }
```

(`issue.escalated()` reflects the state `note_diagnosis` just updated; `issue.id()`
is a `&'static str` into the runtime table, valid beyond the borrow. Single hart,
interrupts off — same access pattern as the Phase 11 escalation log.)

- [ ] **Step 2: Gate the healer-notify on `quarantine_id`**

Replace the Phase 5b healer-notify block:

```rust
                if s.tasks[current].as_ref().unwrap().relaunch.is_some() {
                    let badge = s.tasks[current].as_ref().unwrap().crash_badge;
                    let name = s.tasks[current].as_ref().unwrap().name;
                    match find_blocked(s, CRASH_EP, IpcRole::Recv) {
                        Some(h) => {
                            s.tasks[h].as_mut().unwrap().message =
                                Message { badge, data: [0; 3] };
                            s.tasks[h].as_mut().unwrap().state = TaskState::Ready;
                            prefer = Some(h);
                        }
                        None => crate::println!("heal: no healer for '{name}' (left down)"),
                    }
                }
```

with:

```rust
                if s.tasks[current].as_ref().unwrap().relaunch.is_some() {
                    let name = s.tasks[current].as_ref().unwrap().name;
                    if let Some(id) = quarantine_id {
                        // Phase 12: the issue is chronic — stop the futile fix.
                        crate::println!("heal: '{name}' quarantined ({id} chronic) -- not restarting");
                    } else {
                        // Phase 5b: notify a user-space healer to restart it.
                        let badge = s.tasks[current].as_ref().unwrap().crash_badge;
                        match find_blocked(s, CRASH_EP, IpcRole::Recv) {
                            Some(h) => {
                                s.tasks[h].as_mut().unwrap().message =
                                    Message { badge, data: [0; 3] };
                                s.tasks[h].as_mut().unwrap().state = TaskState::Ready;
                                prefer = Some(h);
                            }
                            None => crate::println!("heal: no healer for '{name}' (left down)"),
                        }
                    }
                }
```

- [ ] **Step 3: Build**

Run: `./tools/build.ps1`
Expected: clean build (arch host tests unaffected).

- [ ] **Step 4: Commit**

```bash
git add arch/riscv64/src/sched.rs
git commit -m "feat(heal): quarantine a chronic (escalated) issue instead of restarting"
```

---

## Task 2: Shell `kb` shows quarantined

**Files:**
- Modify: `arch/riscv64/src/shell.rs` (the `kb` dispatch arm)

- [ ] **Step 1: Render escalated entries as quarantined** — update the `kb` arm:

```rust
        "kb" => {
            let mut i = 0;
            while let Some((id, title, seen, escalated)) = crate::heal::entry(i) {
                if escalated {
                    crate::println!("{id} (seen {seen}, escalated, quarantined)  {title}");
                } else {
                    crate::println!("{id} (seen {seen})  {title}");
                }
                i += 1;
            }
            if i == 0 {
                crate::println!("(knowledge base empty)");
            }
        }
```

- [ ] **Step 2: Build**

Run: `./tools/build.ps1`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add arch/riscv64/src/shell.rs
git commit -m "feat(shell): kb shows an escalated issue as quarantined"
```

---

## Task 3: Cross-boot quarantine assertions

**Files:**
- Modify: `tools/test-qemu.ps1`

**Interfaces:** boot-2 marker `heal: 'flaky' quarantined (KB-0005 chronic) -- not restarting`; the persisted-seen value changes from 8 to 6 in boot 2.

- [ ] **Step 1: Update the boot-2 persisted-seen assertion** — because quarantine stops `flaky` after one crash in boot 2, `seen` ends at 6, not 8. Change in `$mustMatch2`:

```powershell
    "heal: persisted KB-0005 \(seen 8, escalated\)",
```
to:
```powershell
    "heal: persisted KB-0005 \(seen 6, escalated\)",
```

- [ ] **Step 2: Add the boot-2 quarantine assertion** to `$mustMatch2`:

```powershell
    "heal: 'flaky' quarantined \(KB-0005 chronic\) -- not restarting",
```

- [ ] **Step 3: Confirm boot 1 is unchanged** — the boot-1 `$mustMatch1` still
  asserts the normal flaky behavior; do **not** change it:
  - `heal: restarted 'transient' \(attempt 1\)`
  - `heal: giving up on 'flaky' after 2 restarts \(flagged for triage\)`
  These hold because KB-0005 never escalates in boot 1 (it maxes at seen 4).

- [ ] **Step 4: Update the PASS banner** — append: `; and Phase 12 act on escalation: once an issue is escalated the organism QUARANTINES the crashing component instead of restarting it (on the second boot 'flaky' is quarantined as chronic rather than restarted-to-bound) - the organism stops a futile fix based on what it learned across reboots.`

- [ ] **Step 5: Run the full boot test**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS: …; and Phase 12 act on escalation: …`.

Debugging aids:
- If boot 2 lacks the quarantine line → confirm the escalated check runs after `note_diagnosis` and that `find_blocked`/notify is the `else` branch.
- If boot 2's persisted line is still `(seen 8, …)` → quarantine isn't suppressing flaky's restarts (it should crash only once in boot 2 now).
- If boot 1 now fails → escalation is firing in boot 1 (threshold ≤ 4); it must be 6 (set in Phase 11). Quarantine must NOT trigger in boot 1.

- [ ] **Step 6: Commit**

```bash
git add tools/test-qemu.ps1
git commit -m "test: cross-boot quarantine of a chronic fault (Phase 12)"
```

---

## Task 4: Documentation

**Files:**
- Create: `docs/learning/0030-quarantine-on-escalation.md`
- Modify: `docs/learning/README.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md`

- [ ] **Step 1: Learning note** `docs/learning/0030-quarantine-on-escalation.md` — short. Cover: what changed (a quarantine branch that suppresses the restart of a crash whose issue is escalated); the idea worth keeping (the organism *acts* on what it learned — it stops a futile fix; and the action **requires** cross-boot memory, just like the escalation that drives it); reused machinery (no new persistence — quarantine is the behavioral consequence of Phase 11's persisted flag); the proof (boot 1 restarts flaky to the bound; boot 2 quarantines it as chronic); what's next (per-component ledgers, de-quarantine, don't-launch-at-boot). Follow `0029` in style.

- [ ] **Step 2: Index** in `docs/learning/README.md` (`0030` line).

- [ ] **Step 3: Roadmap** — replace `## Phase 12+ — Breadth` with a completed `## Phase 12 — Act on escalation: quarantine a chronic fault (done — 2026-06-27)` (goal / you-learn / done-when citing note 0030), and re-add a `## Phase 13+ — Breadth` placeholder.

- [ ] **Step 4: Glossary** — add **Quarantine (self-healing)** near the escalation/self-healing terms.

- [ ] **Step 5: Cross-reference check**

Run: `./tools/check-references.ps1`
Expected: passes.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0030-quarantine-on-escalation.md docs/learning/README.md docs/roadmap/roadmap.md docs/glossary.md
git commit -m "docs: Phase 12 quarantine on escalation — learning note 0030, roadmap, glossary"
```

---

## Self-Review (completed during planning)

- **Spec coverage:** quarantine branch → Task 1; shell display → Task 2; cross-boot proof + updated seen value → Task 3; docs → Task 4. All spec sections map to a task.
- **Consistency:** `quarantine_id: Option<&'static str>` set from `issue.id()` (Task 1); the boot-2 persisted-seen drops to 6 because quarantine stops flaky after one crash (Tasks 1 & 3 agree); boot-1 assertions deliberately unchanged (no escalation in boot 1).
- **Open verification during execution:** confirm boot 2 `seen` ends at exactly 6 (transient 1 + flaky 1 after the loaded 4) — if flaky somehow crashes more than once before quarantine, the persisted value/threshold interplay needs a recheck; watch that the quarantine line uses the exact text the test asserts (`'flaky' quarantined (KB-0005 chronic) -- not restarting`).
