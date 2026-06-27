# Phase 8 — Capability delegation through IPC — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a component delegate a capability it holds to another component at runtime over IPC, kernel-mediated and unforgeable — making the authority graph dynamic (ADR 0007).

**Architecture:** A dedicated `grant` syscall (a7 = 11) copies a sender-nominated cap from the sender's cap table into a recv-blocked peer's table, riding the existing send/recv rendezvous. Generalizes the precedent the kernel already has — `mint_reply_cap` installing a cap into a receiver during a `Call`. Copy (grant) semantics: the sender keeps its cap.

**Tech Stack:** Rust `no_std` kernel (`arch/riscv64`, `kernel`), pure host-tested `cap.rs`, QEMU riscv64, PowerShell boot harness.

**Spec:** `docs/superpowers/specs/2026-06-27-phase-8-capability-delegation-design.md`

## Global Constraints

- **Commits:** Conventional Commits, NO Claude co-author trailer; author Kathir (signing automated).
- **Pure cap logic stays host-tested** (`cap.rs` has no I/O); run `cargo test -p kernel-arch-riscv64`.
- **Kernel build:** `./tools/build.ps1`. **Boot test:** `./tools/test-qemu.ps1`.
- **U-mode task code** lives in `.user_text`, no calls into kernel `.text`/`.rodata` (inline asm), reports via syscalls. New components follow the existing `rtc_client`/`rtc_server` pattern.
- The crash/IPC paths run with interrupts off in the trap handler — no I/O there.

---

## File Structure

- `arch/riscv64/src/cap.rs` — **add** `cap_at` (pure read of the whole cap) + tests.
- `arch/riscv64/src/syscall.rs` — **add** `Syscall::Grant` (a7 = 11), decode + dispatch + tests.
- `arch/riscv64/src/task.rs` — **add** `Task.pending_grant: Option<Capability>`.
- `arch/riscv64/src/sched.rs` — **add** `install_cap`, `ipc_grant`; **extend** `ipc_recv` to install a delegated cap; set `pending_grant: None` at the Task construction sites.
- `kernel/src/main.rs` — **add** `sys_grant` wrapper, `broker_task`, rename `rtc_client`→`needy_task` (obtains its RTC cap by delegation), boot wiring (grant endpoint, broker/needy caps), new stacks.
- `tools/test-qemu.ps1` — assert the delegation + rejection lines; swap `'client'`→`'needy'`.
- `docs/learning/0026-capability-delegation.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md` — docs.

---

## Task 1: `cap::cap_at` — read the whole capability

**Files:**
- Modify: `arch/riscv64/src/cap.rs` (impl + tests)

**Interfaces:**
- Produces: `pub fn cap_at(caps: &[Option<Capability>], idx: usize) -> Option<Capability>`

- [ ] **Step 1: Write the failing tests** (in the `cap.rs` tests module)

```rust
    #[test]
    fn cap_at_reads_the_whole_capability() {
        let caps = [None, Some(Capability::Endpoint(7)), Some(Capability::Randomness)];
        assert_eq!(cap_at(&caps, 1), Some(Capability::Endpoint(7)));
        assert_eq!(cap_at(&caps, 2), Some(Capability::Randomness));
    }

    #[test]
    fn cap_at_rejects_empty_and_out_of_range() {
        let caps = [None, Some(Capability::Endpoint(0))];
        assert_eq!(cap_at(&caps, 0), None, "empty slot");
        assert_eq!(cap_at(&caps, 9), None, "out of range");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p kernel-arch-riscv64 cap_at`
Expected: FAIL — `cannot find function cap_at`.

- [ ] **Step 3: Implement** (in `cap.rs`, after `cap_lookup`)

```rust
/// Read the whole `Capability` at `idx` (the value the kernel copies when a
/// component delegates it via `grant`). `None` for an empty slot or an
/// out-of-range index — which is the unforgeability guard: a component can only
/// delegate a capability it actually holds.
pub fn cap_at(caps: &[Option<Capability>], idx: usize) -> Option<Capability> {
    match caps.get(idx) {
        Some(Some(cap)) => Some(*cap),
        _ => None,
    }
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p kernel-arch-riscv64 cap_at`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/cap.rs
git commit -m "feat(cap): cap_at — read the whole capability for delegation"
```

---

## Task 2: `Syscall::Grant` — decode a7 = 11

**Files:**
- Modify: `arch/riscv64/src/syscall.rs` (enum, decode, test)

**Interfaces:**
- Produces: `Syscall::Grant`; `decode_syscall(11) == Syscall::Grant`.

- [ ] **Step 1: Write the failing test** (in the `syscall.rs` tests module, near the other decode tests)

```rust
    #[test]
    fn decodes_grant() {
        assert_eq!(decode_syscall(11), Syscall::Grant);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 decodes_grant`
Expected: FAIL — no `Grant` variant.

- [ ] **Step 3: Implement**

Add the variant to `enum Syscall` (after `WaitIrq`):

```rust
    /// `grant(ep_cap, src_cap_slot, badge)` — delegate (copy) the capability in
    /// the sender's `src_cap_slot` to a peer recv-blocked on `ep_cap`.
    Grant,
```

Add the decode arm (after `10 => Syscall::WaitIrq,`):

```rust
        11 => Syscall::Grant,
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 decodes_grant`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/syscall.rs
git commit -m "feat(syscall): decode grant (a7=11)"
```

---

## Task 3: Kernel rendezvous — `pending_grant`, `install_cap`, `ipc_grant`, recv install

**Files:**
- Modify: `arch/riscv64/src/task.rs` (`Task.pending_grant` field)
- Modify: `arch/riscv64/src/sched.rs` (`install_cap`, `ipc_grant`, `ipc_recv` install step, construction sites)
- Test: boot test (Task 5).

**Interfaces:**
- Consumes: `cap::cap_at` (Task 1), `find_blocked`, `block_current`, `write_message`, `Message`, `IpcRole`, `CAP_SLOTS`.
- Produces: `pub fn ipc_grant(frame: &mut crate::trap::TrapFrame)`; `Task.pending_grant`.

- [ ] **Step 1: Add the `pending_grant` field to `Task`**

In `arch/riscv64/src/task.rs`, after the `caller` field in `struct Task`:

```rust
    /// A capability in transit through this task during a `grant` exchange:
    /// while this task is a blocked sender it holds the cap it is delegating;
    /// when this task is a receiver being woken it holds the delegated cap to
    /// install into its recv reply-slot. Transient (single hart); `None`
    /// otherwise. (Phase 8: capability delegation.)
    pub pending_grant: Option<crate::cap::Capability>,
```

- [ ] **Step 2: Initialize `pending_grant: None` at every `Task` construction site**

In `arch/riscv64/src/sched.rs`, find each `caller: None,` (the Task literals in `spawn`, `spawn_user`, and the third construction site) and add `pending_grant: None,` immediately after it. There are three sites — search for `caller: None`.

- [ ] **Step 3: Build to verify the field compiles**

Run: `./tools/build.ps1`
Expected: builds (host tests still pass; the field is unused for now).

- [ ] **Step 4: Add `install_cap` and `ipc_grant`** (in `sched.rs`, near `mint_reply_cap`)

```rust
/// Install `cap` into `task`'s cap table at `slot`; a no-op if `slot` is out of
/// range (a receiver bug, not fatal). Generalizes the slot-bounded write
/// `mint_reply_cap` performs.
#[cfg(target_arch = "riscv64")]
fn install_cap(s: &mut Scheduler, task: usize, slot: usize, cap: crate::cap::Capability) {
    if slot < CAP_SLOTS {
        s.tasks[task].as_mut().unwrap().caps[slot] = Some(cap);
    }
}

/// Service a `grant` syscall: delegate (copy) the capability in the sender's
/// `a1` cap slot to a peer recv-blocked on the endpoint named by `a0`, carrying
/// the `a2` badge. The receiver installs the cap into the slot it named in its
/// `recv` when it wakes. Copy semantics: the sender keeps its capability.
/// `a0 = 0` on success, `usize::MAX` if the sender lacks the endpoint or the
/// source slot is empty (the unforgeability guard — a component can only
/// delegate what it holds).
#[cfg(target_arch = "riscv64")]
pub fn ipc_grant(frame: &mut crate::trap::TrapFrame) {
    let ep_idx = frame.regs[9];   // a0
    let src_slot = frame.regs[10]; // a1
    let badge = frame.regs[11];   // a2
    enum G { BadCap, Delivered, Block(EndpointId) }
    let step = SCHED.with(|s| {
        let cur = s.current;
        let name = s.tasks[cur].as_ref().unwrap().name;
        let ep = match cap_lookup(&s.tasks[cur].as_ref().unwrap().caps, ep_idx) {
            Some(ep) => ep,
            None => {
                crate::println!("cap: '{name}' grant rejected (no endpoint capability)");
                return G::BadCap;
            }
        };
        let cap = match crate::cap::cap_at(&s.tasks[cur].as_ref().unwrap().caps, src_slot) {
            Some(c) => c,
            None => {
                crate::println!("cap: '{name}' grant rejected (no capability in slot)");
                return G::BadCap;
            }
        };
        let msg = Message { badge, data: [0; 3] };
        match find_blocked(s, ep, IpcRole::Recv) {
            Some(ri) => {
                let rname = s.tasks[ri].as_ref().unwrap().name;
                crate::println!("cap: '{name}' delegated {cap:?} to '{rname}'");
                s.tasks[ri].as_mut().unwrap().message = msg;
                s.tasks[ri].as_mut().unwrap().pending_grant = Some(cap);
                s.tasks[ri].as_mut().unwrap().state = TaskState::Ready;
                G::Delivered
            }
            None => {
                // No receiver yet: carry the cap on us until one arrives.
                s.tasks[cur].as_mut().unwrap().message = msg;
                s.tasks[cur].as_mut().unwrap().pending_grant = Some(cap);
                G::Block(ep)
            }
        }
    });
    match step {
        G::BadCap => frame.regs[9] = usize::MAX,
        G::Delivered => frame.regs[9] = 0,
        G::Block(ep) => {
            block_current(ep, IpcRole::Send);
            frame.regs[9] = 0;
        }
    }
}
```

- [ ] **Step 5: Extend `ipc_recv` to install a delegated cap**

In `ipc_recv` (`sched.rs`), the receiver installs a delegated cap into its `reply_slot` (already read as `reply_slot` at the top of the fn) in BOTH delivery branches.

(a) In the **immediate** branch — where a waiting sender is found — move that sender's `pending_grant` onto the receiver before readying it. Replace the `Some((si, is_call))` arm body so that, after computing `msg`, it also does:

```rust
            Some((si, is_call)) => {
                let msg = s.tasks[si].as_ref().unwrap().message;
                // A grant sender carries the delegated cap; move it to us to
                // install below.
                let granted = s.tasks[si].as_mut().unwrap().pending_grant.take();
                if is_call {
                    s.tasks[si].as_mut().unwrap().state = TaskState::AwaitingReply;
                    mint_reply_cap(s, cur, si, reply_slot);
                } else {
                    s.tasks[si].as_mut().unwrap().state = TaskState::Ready;
                }
                if let Some(cap) = granted {
                    install_cap(s, cur, reply_slot, cap);
                }
                RecvStep::Got(msg)
            }
```

(b) In the **blocked-then-woken** branch — after `block_current` returns — a grant set `self.pending_grant` directly; install it. Extend the `RecvStep::Block(ep)` arm's post-wake `SCHED.with` so that, after the existing reply-cap mint, it also installs a pending grant:

```rust
        RecvStep::Block(ep) => {
            block_current(ep, IpcRole::Recv);
            let msg = SCHED.with(|s| {
                let cur = s.current;
                if let Some(caller) = s.tasks[cur].as_mut().unwrap().caller.take() {
                    mint_reply_cap(s, cur, caller, reply_slot);
                }
                if let Some(cap) = s.tasks[cur].as_mut().unwrap().pending_grant.take() {
                    install_cap(s, cur, reply_slot, cap);
                }
                s.tasks[cur].as_ref().unwrap().message
            });
            write_message(frame, msg);
        }
```

- [ ] **Step 6: Build**

Run: `./tools/build.ps1`
Expected: clean build (the new fns are wired by Task 4).

- [ ] **Step 7: Commit**

```bash
git add arch/riscv64/src/task.rs arch/riscv64/src/sched.rs
git commit -m "feat(sched): ipc_grant — kernel-mediated capability delegation over IPC"
```

---

## Task 4: Syscall wiring + the broker/needy demo

**Files:**
- Modify: `arch/riscv64/src/syscall.rs` (`dispatch` arm for `Grant`)
- Modify: `kernel/src/main.rs` (`sys_grant`, `broker_task`, rename `rtc_client`→`needy_task`, boot wiring, stacks)
- Test: boot test (Task 5).

**Interfaces:**
- Consumes: `sched::ipc_grant`, `sys_grant`, the grant endpoint + caps.

- [ ] **Step 1: Route the syscall**

In `arch/riscv64/src/syscall.rs` `dispatch`, add an arm (after `Syscall::WaitIrq`):

```rust
        Syscall::Grant => {
            crate::sched::ipc_grant(frame);
            Outcome::Resume
        }
```

- [ ] **Step 2: Add the `sys_grant` U-mode wrapper** (in `kernel/src/main.rs`, near `sys_send`)

```rust
    /// grant syscall (a7 = 11): a0 = endpoint cap to send over, a1 = the
    /// sender's source cap slot to delegate, a2 = badge. Returns a0 = 0 on
    /// success, or `usize::MAX` if the endpoint cap or the source slot is
    /// invalid.
    ///
    /// # Safety
    /// Always sound; the kernel validates both capabilities.
    #[inline(always)]
    unsafe fn sys_grant(ep: usize, src_slot: usize, badge: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 11usize,
            inout("a0") ep => ret,
            in("a1") src_slot,
            in("a2") badge,
            options(nostack),
        );
        ret
    }
```

- [ ] **Step 3: Add the grant-endpoint constants** (near the other EP constants around `const EP0`)

```rust
    /// The broker→needy delegation channel (Phase 8). Distinct from the RTC
    /// endpoint EP0, whose cap the broker delegates.
    const GRANT_EP: usize = 6;
    /// needy/broker cap slots for the grant channel and the delegated cap.
    const GRANT_CHAN_CAP: usize = 0; // Endpoint(GRANT_EP), held by both
    const BROKER_RTC_SLOT: usize = 1; // broker's Endpoint(EP0) to delegate
    const NEEDY_RTC_SLOT: usize = 1;  // where needy receives the delegated RTC cap
    const NEEDY_EMPTY_SLOT: usize = 3; // an empty slot the broker's bad grant names
```

- [ ] **Step 4: Replace `rtc_client` with `needy_task`** (the RTC client that obtains its cap by delegation)

Rename the existing `extern "C" fn rtc_client()` to `needy_task` and rewrite its body so it first receives the delegated cap on the grant channel, then calls the RTC server with it:

```rust
    /// `needy` (Phase 8): an RTC client that holds NO RTC capability. It blocks
    /// receiving on the grant channel (naming NEEDY_RTC_SLOT as where the kernel
    /// installs the delegated cap), then `call`s the RTC server on that
    /// now-delegated capability and exits with the live clock — proof that
    /// authority reached it only by runtime delegation.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn needy_task() -> ! {
        unsafe {
            // Block until the broker delegates the RTC endpoint cap into
            // NEEDY_RTC_SLOT; the badge is discarded.
            let _ = sys_recv(GRANT_CHAN_CAP, NEEDY_RTC_SLOT);
            let t = sys_call(NEEDY_RTC_SLOT, 1);
            sys_exit(t)
        }
    }
```

- [ ] **Step 5: Add the `broker_task`** (near `needy_task`)

```rust
    /// `broker` (Phase 8): holds the RTC endpoint cap (BROKER_RTC_SLOT) and the
    /// grant-channel cap (GRANT_CHAN_CAP). It first attempts a bad grant (an
    /// empty source slot → rejected, proving the unforgeability guard), then
    /// delegates the RTC endpoint cap to whoever is waiting on the grant
    /// channel (needy), and exits.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn broker_task() -> ! {
        unsafe {
            // Negative: delegate a slot we don't hold -> rejected.
            let _ = sys_grant(GRANT_CHAN_CAP, NEEDY_EMPTY_SLOT, 0);
            // Real delegation: hand the RTC endpoint cap to needy.
            let _ = sys_grant(GRANT_CHAN_CAP, BROKER_RTC_SLOT, 1);
            sys_exit(0)
        }
    }
```

- [ ] **Step 6: Rewire the boot cast** (in `kmain`, the rtc/client block at ~line 82)

Keep `rtc_server` and `rogue` as-is. Replace the `client` spawn with `needy`, and add `broker`. Spawn order: `rtc` (recv-blocks on EP0), then `needy` (recv-blocks on GRANT_EP), then `broker` (grants). Use the existing `US_CLIENT`/`KS_CLIENT` stacks for `needy` (renamed in place is fine — or keep the names; they are just storage) and add new stacks for `broker`:

```rust
        let cu = ustack(core::ptr::addr_of!(US_CLIENT) as usize);
        let needy = sched::spawn_user("needy", needy_task, cu.1,
            core::ptr::addr_of!(KS_CLIENT) as usize + TASK_STACK,
            mem::build_user_space(cu, NO_DEVICE));
        sched::grant_cap(needy, GRANT_CHAN_CAP, Capability::Endpoint(GRANT_EP));
        // needy holds NO Endpoint(EP0) — it must obtain the RTC cap by delegation.

        let bru = ustack(core::ptr::addr_of!(US_BROKER) as usize);
        let broker = sched::spawn_user("broker", broker_task, bru.1,
            core::ptr::addr_of!(KS_BROKER) as usize + TASK_STACK,
            mem::build_user_space(bru, NO_DEVICE));
        sched::grant_cap(broker, GRANT_CHAN_CAP, Capability::Endpoint(GRANT_EP));
        sched::grant_cap(broker, BROKER_RTC_SLOT, Capability::Endpoint(EP0));
```

- [ ] **Step 7: Declare `US_BROKER`/`KS_BROKER` stacks** (alongside the other `US_*`/`KS_*` declarations; copy the `US_CLIENT`/`KS_CLIENT` form)

```rust
    static mut KS_BROKER: KStack = [0; TASK_STACK];
```

and with the `#[link_section = ".user_data"]` UStacks:

```rust
    #[link_section = ".user_data"]
    static mut US_BROKER: UStack = UStack([0; USER_STACK_SIZE]);
```

- [ ] **Step 8: Check `MAX_TASKS` headroom**

The cast gains one task (broker; needy reuses client's slot). If `./tools/build.ps1` boots but the smoke later panics `scheduler full`, raise `MAX_TASKS` in `sched.rs` by 1 (and add one `None` to the `Scheduler::new` initializer array), as Phase 7 did. Verify by running Task 5; only bump if it panics.

- [ ] **Step 9: Build**

Run: `./tools/build.ps1`
Expected: clean build.

- [ ] **Step 10: Commit**

```bash
git add arch/riscv64/src/syscall.rs kernel/src/main.rs
git commit -m "feat(cap): grant syscall wiring + broker/needy delegation demo"
```

---

## Task 5: Boot test — assert the delegation

**Files:**
- Modify: `tools/test-qemu.ps1`
- Test: this IS the integration test.

- [ ] **Step 1: Update the boot-1 assertions**

In `tools/test-qemu.ps1`, in the `$mustMatch1` array: replace the `'client'` exit line
```powershell
    "sched: task 'client' exited \(code \d{15,}\)",
```
with the needy + delegation lines:
```powershell
    "cap: 'broker' grant rejected \(no capability in slot\)",
    "cap: 'broker' delegated Endpoint\(0\) to 'needy'",
    "sched: task 'needy' exited \(code \d{15,}\)",
```

(The `Endpoint(0)` text comes from `{cap:?}` formatting `Capability::Endpoint(0)` where `EP0 = 0`.)

- [ ] **Step 2: Update the PASS banner** to append a Phase 8 clause, e.g. `; and Phase 8 capability delegation: a 'broker' delegates the RTC endpoint capability to a 'needy' client that held no RTC cap, which then reads the live clock through it (and a grant of a slot the broker doesn't hold is refused) - authority flows between components at runtime, kernel-mediated.`

- [ ] **Step 3: Run the full boot test**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS: ...; and Phase 8 capability delegation: ...`. If `scheduler full` panics, apply Task 4 Step 8 (bump `MAX_TASKS`).

- [ ] **Step 4: Commit**

```bash
git add tools/test-qemu.ps1
git commit -m "test: assert runtime capability delegation (Phase 8)"
```

---

## Task 6: Documentation

**Files:**
- Create: `docs/learning/0026-capability-delegation.md`
- Modify: `docs/learning/README.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md`

- [ ] **Step 1: Write the learning note** `docs/learning/0026-capability-delegation.md`

Short (per memory: learning-notes-minimal). Cover: what changed (a `grant` syscall delegates a held cap to a peer over IPC, copy semantics); the idea worth keeping (delegation generalizes the kernel's existing `mint_reply_cap` step — a component, not just the kernel, can install a cap into a peer, and unforgeability holds because the kernel copies a cap the sender provably holds); the demo (broker delegates the RTC endpoint to a needy client that had no RTC cap); what's next (move/transfer, rights attenuation, revocation, reply-cap forwarding). Follow the structure of `0021-reply-capabilities.md`.

- [ ] **Step 2: Index it** in `docs/learning/README.md` (add the `0025` line if missing, then `0026`).

- [ ] **Step 3: Roadmap** — replace `## Phase 8+ — Breadth` with a completed `## Phase 8 — Capability delegation through IPC (done — 2026-06-27)` (goal / you-learn / done-when, citing learning note 0026), and re-add a `## Phase 9+ — Breadth` placeholder listing the remaining directions (revisable/growable KB, interactive shell, physical board boot 4c, more devices/HAL).

- [ ] **Step 4: Glossary** — add **Capability delegation** (a component passing a capability it holds to another at runtime, kernel-mediated and unforgeable; copy semantics; the `grant` syscall) and **`grant`** near the existing capability/IPC terms.

- [ ] **Step 5: Cross-reference check**

Run: `./tools/check-references.ps1`
Expected: passes.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0026-capability-delegation.md docs/learning/README.md docs/roadmap/roadmap.md docs/glossary.md
git commit -m "docs: Phase 8 capability delegation — learning note 0026, roadmap, glossary"
```

---

## Self-Review (completed during planning)

- **Spec coverage:** `cap_at` guard → Task 1; `grant` syscall decode → Task 2; rendezvous + `pending_grant` + both delivery paths → Task 3; ABI wrapper + broker/needy demo + bad-grant negative → Task 4; boot proof → Task 5; docs → Task 6. All spec sections map to a task.
- **Type consistency:** `cap_at(&[Option<Capability>], usize) -> Option<Capability>` consistent across Tasks 1 and 3; `ipc_grant(frame)` consistent across Tasks 3 and 4; `Task.pending_grant: Option<Capability>` set in Task 3, initialized at all construction sites; `sys_grant(ep, src_slot, badge)` consistent across Tasks 4 and the demo; `GRANT_EP=6` distinct from EP0..BLK_EP(4)/GATE_EP(5).
- **Open verification during execution:** confirm the count of `Task` construction sites (`caller: None`) before adding `pending_grant: None` (Task 3 Step 2); confirm `MAX_TASKS` headroom at runtime (Task 4 Step 8) — only bump if `scheduler full` panics.
```
