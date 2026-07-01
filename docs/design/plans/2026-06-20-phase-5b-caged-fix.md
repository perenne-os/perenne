# Phase 5b: Self-healing — the caged fix — Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** An isolated, capability-gated user-space healer, notified by the kernel when a component is contained after a crash, applies the KB-0005 playbook — a bounded, reversible, logged restart — recovering the component.

**Architecture:** The acting agent (the healer) is an unprivileged U-mode component; the kernel is the cage. On a contained, diagnosed crash (5a), `exit_current` notifies a healer recv-blocked on a reserved crash endpoint (reusing the IPC rendezvous), delivering a badge that is the cap index of the crashed component's `Restart` capability. The healer calls a new `restart(cap_idx)` syscall; the kernel capability-checks it, enforces a per-task retry bound, re-forges the task's first-run context (reusing its address space/stacks), passes the launch generation in `a0`, and logs. A `transient` patient (crash once, then recover) proves recovery; the always-crashing `flaky` proves the bound.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU. Host tests: `cargo test -p kernel-arch-riscv64`. Bare: `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/design/specs/2026-06-20-phase-5b-caged-fix-design.md`

**Grounding facts (verified against the tree):**
- `Capability` (cap.rs) currently has one variant `Endpoint(EndpointId)`; `cap_lookup` matches `Endpoint` and falls through to `None`. `EndpointId = usize`.
- `Task` (task.rs, `#[derive(Debug)]`) fields: `context, state, stack_top, name, satp, caps, message`. Constructed in `sched::spawn`, `sched::spawn_user`, and the sched test helper `task()`.
- `forge_user_context(tramp, entry, user_sp, kstack_top, sstatus) -> Context` sets `s[0]=entry, s[1]=user_sp, s[2]=sstatus`; `s[3..]` stay 0. Only caller (non-test) is `spawn_user`.
- `user_trampoline` asm: `csrw sscratch,sp; csrw sepc,s0; csrw sstatus,s2; mv sp,s1; sret`.
- `exit_current(Killed(cause))` (sched.rs) logs the kill + 5a diagnosis inside `SCHED.with`. `find_blocked(s, ep, role)`, `Message`, `IpcRole`, `TaskState` are all in scope in sched.rs.
- `MAX_TASKS = 6` (sched.rs:25) and `Scheduler::new` initializes a 6-element `[None,...]` array (sched.rs:37).
- Syscalls: `decode_syscall` maps 1..5 (Print/Exit/Yield/Send/Recv); `dispatch` routes them; `Outcome::{Resume, Exit, Yield}`. `restart` will return `Outcome::Resume`.
- `kmain` demo (main.rs:72-102): spawns `rtc(0)`, `client(1)`, `rogue(2)`, `flaky(3)`, `idle(4)`. Helpers in scope: `ustack` closure, `NO_DEVICE`, `EP0`/`EP_CAP`, `Capability` (imported), `TASK_STACK`, `KStack`/`UStack` types.

**Cast after this plan:** `rtc(0)` · `client(1)` · `rogue(2)` · `healer(3)` · `transient(4)` · `flaky(5)` · `idle(6)` — 7 tasks (`MAX_TASKS` bumped to 8). Rogue is retained (the spec listed an illustrative cast; keeping rogue preserves the 3b-iii capability-rejection proof and its existing smoke assertion). Order guarantees the healer recv-blocks before either patient crashes.

---

## Task 1: the `Restart` capability + its lookup

**Files:**
- Modify: `arch/riscv64/src/cap.rs`

- [ ] **Step 1: Write the failing tests**

In `arch/riscv64/src/cap.rs`, add to the `tests` module (after `rejects_an_out_of_range_index`):

```rust
    #[test]
    fn looks_up_a_granted_restart_target() {
        let caps = [None, Some(Capability::Restart(4)), None];
        assert_eq!(restart_target(&caps, 1), Some(4));
    }

    #[test]
    fn restart_target_rejects_wrong_type_empty_and_oob() {
        let caps = [Some(Capability::Endpoint(0)), None];
        assert_eq!(restart_target(&caps, 0), None, "an Endpoint cap is not a Restart cap");
        assert_eq!(restart_target(&caps, 1), None, "empty slot");
        assert_eq!(restart_target(&caps, 9), None, "out of range");
    }

    #[test]
    fn cap_lookup_rejects_a_restart_cap() {
        // The endpoint lookup must not accept a Restart cap as an endpoint.
        let caps = [Some(Capability::Restart(3))];
        assert_eq!(cap_lookup(&caps, 0), None);
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p kernel-arch-riscv64 cap::`
Expected: FAIL — `Capability::Restart` and `restart_target` do not exist (compile error).

- [ ] **Step 3: Add the variant and the lookup**

In `arch/riscv64/src/cap.rs`, extend the `Capability` enum:

```rust
pub enum Capability {
    /// Authority to `send` to / `recv` from the endpoint with this id.
    Endpoint(EndpointId),
    /// Authority to `restart` the task in this scheduler slot (Phase 5b).
    Restart(usize),
}
```

Then add, after `cap_lookup`:

```rust
/// Look up capability `idx` in `caps`; if it is a restart capability, return
/// its target scheduler slot. `None` if the index is out of range, the slot
/// is empty, or the capability is the wrong type (e.g. an endpoint).
pub fn restart_target(caps: &[Option<Capability>], idx: usize) -> Option<usize> {
    match caps.get(idx) {
        Some(Some(Capability::Restart(slot))) => Some(*slot),
        _ => None,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p kernel-arch-riscv64 cap::`
Expected: PASS (all cap tests, new and old).

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/cap.rs
git commit -m "feat(cap): Restart capability + restart_target lookup (host-tested)"
```

---

## Task 2: the restart data model — `Relaunch`, `Task` fields, `can_restart`, generation in the forge

This task changes `Task` and `forge_user_context`, which breaks their construction sites — so it updates every site in the same task to keep the crate (and host tests) compiling.

**Files:**
- Modify: `arch/riscv64/src/task.rs`
- Modify: `arch/riscv64/src/sched.rs` (construction sites + the forge call)

- [ ] **Step 1: Write/adjust the failing tests in `task.rs`**

In `arch/riscv64/src/task.rs`, add two tests and amend the forge test. Add to the `tests` module:

```rust
    #[test]
    fn can_restart_respects_the_bound() {
        assert!(can_restart(0, 2));
        assert!(can_restart(1, 2));
        assert!(!can_restart(2, 2), "at the bound: refuse");
        assert!(!can_restart(3, 2), "over the bound: refuse");
    }
```

Replace the existing `forge_user_context_sets_launch_fields` test with one that exercises the new `generation` parameter:

```rust
    #[test]
    fn forge_user_context_sets_launch_fields() {
        // tramp/entry/user_sp/kstack are arbitrary addresses; 16-alignment is
        // applied to the two stack pointers. generation rides in s[3].
        let c = forge_user_context(0xAAAA, 0xBBBB, 0x1_0008, 0x2_0008, 0xCAFE, 2);
        assert_eq!(c.ra, 0xAAAA, "ra = trampoline");
        assert_eq!(c.sp, 0x2_0000, "sp = kstack_top, 16-aligned");
        assert_eq!(c.s[0], 0xBBBB, "s0 = user entry (-> sepc)");
        assert_eq!(c.s[1], 0x1_0000, "s1 = user sp, 16-aligned");
        assert_eq!(c.s[2], 0xCAFE, "s2 = sstatus");
        assert_eq!(c.s[3], 2, "s3 = launch generation (-> a0)");
        assert_eq!(c.s[4..], [0usize; 8], "untouched slots stay zero");
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p kernel-arch-riscv64 task::`
Expected: FAIL — `can_restart` undefined and `forge_user_context` takes 5 args (arity mismatch).

- [ ] **Step 3: Add `Relaunch`, the `Task` fields, `can_restart`, and the forge param**

In `arch/riscv64/src/task.rs`:

Add after the `Message` impl (before `IpcRole`):

```rust
/// What a U-mode task needs to be relaunched from scratch: its entry point
/// and the top of its user stack. The kernel keeps this so a `restart`
/// (Phase 5b) can re-forge the task's first-run context. Kernel tasks are
/// not restartable and carry `None`.
#[derive(Debug, Clone, Copy)]
pub struct Relaunch {
    pub entry: usize,
    pub user_sp: usize,
}

/// Is another restart allowed? `true` while `restarts` is below `bound`.
/// The kernel's safety cage uses this so a component that keeps crashing is
/// abandoned (and flagged) once the bound is reached. Pure (host-tested).
pub fn can_restart(restarts: usize, bound: usize) -> bool {
    restarts < bound
}
```

Add the `generation` parameter to `forge_user_context` (set `s[3]`):

```rust
pub fn forge_user_context(
    tramp: usize,
    entry: usize,
    user_sp: usize,
    kstack_top: usize,
    sstatus: usize,
    generation: usize,
) -> Context {
    let mut c = Context::zeroed();
    c.ra = tramp;
    c.sp = kstack_top & !0xF;
    c.s[0] = entry;
    c.s[1] = user_sp & !0xF;
    c.s[2] = sstatus; // CSR bit-field, not an address — no alignment
    c.s[3] = generation; // -> a0 at launch (user_trampoline does `mv a0, s3`)
    c
}
```

Add the three fields to `Task` (after `message`):

```rust
    /// In-flight IPC message: a task's outbox while it blocks sending, or
    /// its inbox when a sender delivers to it.
    pub message: Message,
    /// How to relaunch this task (Phase 5b). `Some` for U-mode components
    /// (restartable); `None` for kernel tasks.
    pub relaunch: Option<Relaunch>,
    /// How many times the self-healer has restarted this task. The kernel's
    /// retry bound is checked against this (`can_restart`).
    pub restarts: usize,
    /// The badge the kernel delivers to the healer when this task crashes —
    /// the healer's own cap-table index of this task's Restart capability,
    /// so the healer can act without a slot→cap mapping. 0 if not a patient.
    pub crash_badge: usize,
```

- [ ] **Step 4: Update the construction sites in `sched.rs`**

In `arch/riscv64/src/sched.rs`, add the import (in the `#[cfg(target_arch = "riscv64")]` import group near the top, e.g. after the `cap` import):

```rust
#[cfg(target_arch = "riscv64")]
use crate::task::Relaunch;
```

In `spawn` (the kernel-task spawner), extend the `Task { .. }` literal:

```rust
        s.tasks[slot] = Some(Task {
            context,
            state: TaskState::Ready,
            stack_top,
            name,
            satp: crate::mem::kernel_satp(),
            caps: [None; CAP_SLOTS],
            message: Message::EMPTY,
            relaunch: None,
            restarts: 0,
            crash_badge: 0,
        });
```

In `spawn_user`, update the forge call to pass generation `0` and record relaunch info. Replace the `forge_user_context(...)` call and the `Task { .. }` literal:

```rust
        let context = crate::task::forge_user_context(
            user_trampoline as *const () as usize,
            entry as *const () as usize,
            user_sp,
            kstack_top,
            crate::task::user_sstatus(crate::csr::sstatus_read()),
            0, // first launch: generation 0
        );
        s.tasks[slot] = Some(Task {
            context,
            state: TaskState::Ready,
            stack_top: kstack_top,
            name,
            satp,
            caps: [None; CAP_SLOTS],
            message: Message::EMPTY,
            relaunch: Some(Relaunch { entry: entry as *const () as usize, user_sp }),
            restarts: 0,
            crash_badge: 0,
        });
```

In the sched `tests` module, extend the `task()` helper's `Task { .. }` literal:

```rust
        Task {
            context: Context::zeroed(),
            state,
            stack_top: 0,
            name,
            satp: 0,
            caps: [None; CAP_SLOTS],
            message: Message::EMPTY,
            relaunch: None,
            restarts: 0,
            crash_badge: 0,
        }
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS — all host tests (new `can_restart`, amended forge test, and the existing suite) green.

- [ ] **Step 6: Verify the bare build still compiles**

Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 7: Commit**

```bash
git add arch/riscv64/src/task.rs arch/riscv64/src/sched.rs
git commit -m "feat(task): restart data model - Relaunch, per-task restarts/crash_badge, can_restart, launch generation in forge"
```

---

## Task 3: the kernel cage — trampoline `a0`, restart machinery, crash notification, bigger run queue

**Files:**
- Modify: `arch/riscv64/src/sched.rs`

- [ ] **Step 1: Pass the launch generation to U-mode (`mv a0, s3`)**

In `arch/riscv64/src/sched.rs`, in the `user_trampoline` global_asm, add `mv a0, s3` right before `sret`:

```rust
user_trampoline:
    csrw sscratch, sp       # sscratch = trap-stack top (for the U->S trap)
    csrw sepc, s0           # user entry point
    csrw sstatus, s2        # SPP = 0, SPIE = 1
    mv sp, s1               # switch to the user stack
    mv a0, s3               # a0 = launch generation (forge put it in s3)
    sret                    # -> U-mode at the entry point
```

- [ ] **Step 2: Bump `MAX_TASKS` to 8 (constant AND the array initializer)**

In `arch/riscv64/src/sched.rs`, change the constant and its doc comment:

```rust
/// Maximum concurrent tasks: the 5b demo runs seven (rtc, client, rogue,
/// healer, transient, flaky, idle) plus one slot of headroom.
pub const MAX_TASKS: usize = 8;
```

And extend the `Scheduler::new` initializer to eight `None`s:

```rust
    pub const fn new() -> Self {
        Self { tasks: [None, None, None, None, None, None, None, None], current: 0 }
    }
```

- [ ] **Step 3: Add the cage constants and the `set_crash_badge` setter**

In `arch/riscv64/src/sched.rs`, add near `MAX_TASKS` (these are used by the gated kernel code and `kmain`):

```rust
/// The reserved endpoint id the kernel uses to notify the self-healer of a
/// contained crash. The healer holds an `Endpoint(CRASH_EP)` capability and
/// `recv`-blocks on it. Distinct from the demo endpoint `EP0` (= 0).
#[cfg(target_arch = "riscv64")]
pub const CRASH_EP: crate::cap::EndpointId = 1;

/// How many times the self-healer may restart a single component before the
/// kernel gives up and flags it for triage (the safety-cage bound).
#[cfg(target_arch = "riscv64")]
pub const MAX_RESTARTS: usize = 2;
```

Add the setter near `grant_cap`:

```rust
/// Set the crash-notification badge of the task in scheduler `slot` — the
/// healer's cap-table index of that task's Restart capability. Called at boot
/// for each patient so a crash notification names the right capability.
#[cfg(target_arch = "riscv64")]
pub fn set_crash_badge(slot: usize, badge: usize) {
    SCHED.with(|s| {
        s.tasks[slot]
            .as_mut()
            .expect("set_crash_badge: empty task slot")
            .crash_badge = badge;
    });
}
```

- [ ] **Step 4: Notify the healer from `exit_current` — and run it next**

The healer's inbox holds one message. With both patients `Ready`, plain
round-robin could run the second patient (overwriting the first crash
notification) before the healer runs — losing a notification. So when a crash
wakes the healer, `exit_current` must switch to the **healer next** (not
`pick_next`), draining each crash before another can occur.

In `arch/riscv64/src/sched.rs`, replace the body of `exit_current` (from `let switch = SCHED.with(|s| {` through the closing `});` of that block) with this version, which threads a `prefer` choice out of the match:

```rust
    let switch = SCHED.with(|s| {
        let current = s.current;
        // If a crash wakes the healer, run it next so its one-message inbox
        // is drained before any other patient can crash.
        let mut prefer: Option<usize> = None;
        match reason {
            ExitReason::Exited(code) => {
                crate::println!("sched: task '{}' exited (code {code})", s.tasks[current].as_ref().unwrap().name)
            }
            ExitReason::Killed(cause) => {
                crate::println!("sched: task '{}' killed by {cause:?}", s.tasks[current].as_ref().unwrap().name);
                // Phase 5a: consult the deterministic knowledge organism and
                // log the diagnosis.
                match crate::heal::diagnose(cause) {
                    Some(issue) => crate::println!(
                        "heal: diagnosed {} ({}) -> playbook: {}",
                        issue.id, issue.title, issue.playbook
                    ),
                    None => crate::println!("heal: no known issue for {cause:?} (recorded for triage)"),
                }
                // Phase 5b: if this is a restartable component, notify a
                // user-space healer waiting on the crash endpoint so it can
                // apply the playbook (a caged restart). Reuses the IPC
                // rendezvous: deliver to a recv-blocked healer and wake it,
                // then run it next.
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
            }
        }
        s.tasks[current].as_mut().unwrap().state = TaskState::Exited;
        let next = prefer.unwrap_or_else(|| s.pick_next());
        assert_ne!(next, current, "exit_current: no runnable successor (idle must be Ready)");
        s.tasks[next].as_mut().unwrap().state = TaskState::Running;
        s.current = next;
        // Save into the dying slot's own context (never resumed).
        let old = core::ptr::addr_of_mut!(s.tasks[current].as_mut().unwrap().context);
        let new = core::ptr::addr_of!(s.tasks[next].as_ref().unwrap().context);
        let next_satp = s.tasks[next].as_ref().unwrap().satp;
        (old, new, next_satp)
    });
```

(The `unsafe { satp_write; switch_context }` tail and the trailing `unreachable!` below this block are unchanged.)

- [ ] **Step 5: Add the `restart` syscall service**

In `arch/riscv64/src/sched.rs`, add after `ipc_send` (it follows the same `SCHED.with` + frame-register pattern):

```rust
/// Service a `restart` syscall (Phase 5b — the caged fix). `a0`
/// (= `frame.regs[9]`) is the capability index of a `Restart` capability.
/// The kernel capability-checks it, enforces the per-task retry bound, and —
/// if allowed — re-forges the target's first-run context (reusing its address
/// space, stacks, and data page), passing the new launch generation, and
/// marks it `Ready`. Every outcome is logged. `a0` becomes `0` on success or
/// `usize::MAX` on refusal (bad/wrong-type cap, or the bound was reached).
#[cfg(target_arch = "riscv64")]
pub fn restart(frame: &mut crate::trap::TrapFrame) {
    let cap_idx = frame.regs[9];
    let ok = SCHED.with(|s| {
        let cur = s.current;
        let target = match crate::cap::restart_target(
            &s.tasks[cur].as_ref().unwrap().caps,
            cap_idx,
        ) {
            Some(t) => t,
            None => return false, // no/wrong capability — refuse
        };
        let restarts = s.tasks[target].as_ref().unwrap().restarts;
        let name = s.tasks[target].as_ref().unwrap().name;
        if !crate::task::can_restart(restarts, MAX_RESTARTS) {
            crate::println!(
                "heal: giving up on '{name}' after {restarts} restarts (flagged for triage)"
            );
            return false;
        }
        let relaunch = s.tasks[target]
            .as_ref()
            .unwrap()
            .relaunch
            .expect("restart target must be a restartable U-mode component");
        let kstack_top = s.tasks[target].as_ref().unwrap().stack_top;
        let satp = s.tasks[target].as_ref().unwrap().satp;
        let generation = restarts + 1;
        let context = crate::task::forge_user_context(
            user_trampoline as *const () as usize,
            relaunch.entry,
            relaunch.user_sp,
            kstack_top,
            crate::task::user_sstatus(crate::csr::sstatus_read()),
            generation,
        );
        let t = s.tasks[target].as_mut().unwrap();
        t.context = context;
        t.satp = satp; // unchanged; explicit for clarity (same address space)
        t.restarts = generation;
        t.state = TaskState::Ready;
        crate::println!("heal: restarted '{}' (attempt {})", t.name, t.restarts);
        true
    });
    frame.regs[9] = if ok { 0 } else { usize::MAX };
}
```

- [ ] **Step 6: Build (host + bare) to verify it compiles**

Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS (gated kernel code is excluded on host; the existing suite stays green).
Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 7: Commit**

```bash
git add arch/riscv64/src/sched.rs
git commit -m "feat(sched): the caged-fix kernel half - restart syscall (cap-checked + bounded + logged), crash notification, a0 generation, MAX_TASKS=8"
```

---

## Task 4: the `restart` syscall number (decode + dispatch)

**Files:**
- Modify: `arch/riscv64/src/syscall.rs`

- [ ] **Step 1: Write the failing test**

In `arch/riscv64/src/syscall.rs`, add to the `tests` module:

```rust
    #[test]
    fn decodes_restart_syscall() {
        assert_eq!(decode_syscall(6), Syscall::Restart);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 syscall::`
Expected: FAIL — `Syscall::Restart` does not exist.

- [ ] **Step 3: Add the variant, decode, and dispatch**

In `arch/riscv64/src/syscall.rs`, add the variant to `Syscall` (after `Recv`):

```rust
    /// `restart(cap)` — the self-healer asks the kernel to restart the
    /// component named by a Restart capability (Phase 5b).
    Restart,
```

Add the decode arm (after `5 => Syscall::Recv,`):

```rust
        6 => Syscall::Restart,
```

Add the dispatch arm (after the `Syscall::Recv` arm):

```rust
        Syscall::Restart => {
            crate::sched::restart(frame);
            Outcome::Resume
        }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 syscall::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/syscall.rs
git commit -m "feat(syscall): restart syscall (a7=6) decode + dispatch"
```

---

## Task 5: extend the smoke test for recovery + the bound (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Add the healing patterns**

In `tools/test-qemu.ps1`, add to the `$mustMatch` array (after the existing `"heal: diagnosed KB-0005",` line):

```powershell
    "sched: task 'transient' killed by LoadPageFault",
    "heal: restarted 'transient' \(attempt 1\)",
    "sched: task 'transient' exited \(code 0\)",
    "heal: giving up on 'flaky' after 2 restarts \(flagged for triage\)",
```

Update the header comment and PASS message to mention 5b: "Phase 5b self-healing — the caged fix: a user-space healer, notified by the kernel of a contained crash, restarts a 'transient' component which then recovers and runs to completion, while an always-crashing 'flaky' is restarted only up to the bound and then flagged for triage."

- [ ] **Step 2: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — the four new patterns are absent (no healer/transient wired up yet). The 5a `flaky killed`/`diagnosed` lines still pass.

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts caged-fix recovery + bound (red)"
```

---

## Task 6: the healer and patients + demo wiring (green)

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Add the `restart` syscall wrapper**

In `kernel/src/main.rs`, add after `sys_recv` (around line 258):

```rust
    /// restart syscall (a7 = 6): a0 = cap index of a Restart capability.
    /// Returns a0: 0 on success, or `usize::MAX` if the capability check
    /// failed or the retry bound was reached.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and the bound.
    #[inline(always)]
    unsafe fn sys_restart(cap: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 6usize,
            inout("a0") cap => ret,
            options(nostack),
        );
        ret
    }
```

- [ ] **Step 2: Add the crash-endpoint cap-slot constant**

In `kernel/src/main.rs`, after `const EP_CAP: usize = 0;` (around line 174), add:

```rust
    /// The healer's cap-table slot holding its `Endpoint(CRASH_EP)` capability
    /// (it `recv`s on this to learn of crashes). Restart caps live at slots
    /// 1.. so a crash notification's badge is directly the cap slot to use.
    const CRASH_CAP: usize = 0;
```

- [ ] **Step 3: Add the kernel + user stacks for the healer and transient**

In `kernel/src/main.rs`, add to the `KS_*` block (after `KS_FLAKY`):

```rust
    static mut KS_HEALER: KStack = [0; TASK_STACK];
    static mut KS_TRANSIENT: KStack = [0; TASK_STACK];
```

and to the `US_*` block (after `US_FLAKY`):

```rust
    #[link_section = ".user_data"]
    static mut US_HEALER: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_TRANSIENT: UStack = UStack([0; USER_STACK_SIZE]);
```

- [ ] **Step 4: Add the healer and transient task functions**

In `kernel/src/main.rs`, add after `flaky_task` (which ends around line 346):

```rust
    /// The self-healer (Phase 5b) — the acting agent, in user space. It blocks
    /// on the crash endpoint; each crash notification's badge IS the cap index
    /// of the crashed component's Restart capability, so it simply asks the
    /// kernel to restart that component. The kernel is the cage: it
    /// capability-checks, enforces the retry bound, and logs. Register-only —
    /// no `.rodata`, no printing (the kernel logs the actions).
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn healer_task() -> ! {
        loop {
            // SAFETY: we hold Endpoint(CRASH_EP) at CRASH_CAP; recv blocks
            // until the kernel reports a crash. The returned badge is the cap
            // index of the crashed component's Restart capability.
            let cap_idx = unsafe { sys_recv(CRASH_CAP) };
            // SAFETY: ask the kernel to apply the playbook (a caged restart).
            unsafe { sys_restart(cap_idx) };
        }
    }

    /// A patient with a TRANSIENT fault: it crashes on its first run and
    /// recovers after the healer restarts it. The kernel passes the launch
    /// generation in `a0` (0 = first run, >0 = a restart); on the first run we
    /// fault like `flaky`, and on any restart we run to completion and exit 0
    /// — proving the component serves again. Register-only (reads `a0` via
    /// inline asm before any other code; the no-arg `-> !` prologue does not
    /// touch `a0`).
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn transient_task() -> ! {
        let generation: usize;
        // SAFETY: read the launch generation the kernel placed in a0.
        unsafe {
            core::arch::asm!("mv {g}, a0", g = out(reg) generation, options(nomem, nostack, preserves_flags));
        }
        if generation == 0 {
            let _v: u8;
            // SAFETY: the deliberate first-run fault (a transient bug). The
            // U-mode load of kernel .text faults (LoadPageFault); the kernel
            // contains, diagnoses, and (via the healer) restarts us.
            unsafe {
                core::arch::asm!(
                    "lb {v}, 0({p})",
                    v = out(reg) _v,
                    p = in(reg) 0x8020_0000usize,
                    options(nostack),
                );
            }
        }
        // Recovered (this is a restart): do our work and exit cleanly.
        // SAFETY: exit is always sound.
        unsafe { sys_exit(0) }
    }
```

- [ ] **Step 5: Rewire the demo cast in `kmain`**

In `kernel/src/main.rs`, replace the rogue + flaky + idle spawn block (currently main.rs:89-102, from the `// rogue gets NO endpoint capability` comment through the `idle` spawn) with:

```rust
        // rogue gets NO endpoint capability — its send must be refused.
        let ru = ustack(core::ptr::addr_of!(US_ROGUE) as usize);
        let _rogue = sched::spawn_user("rogue", rogue_task, ru.1,
            core::ptr::addr_of!(KS_ROGUE) as usize + TASK_STACK,
            mem::build_user_space(ru, NO_DEVICE));

        // Phase 5b — the caged fix. The healer (the acting agent, in user
        // space) blocks on the crash endpoint before either patient runs, so
        // it is waiting when they crash.
        let hu = ustack(core::ptr::addr_of!(US_HEALER) as usize);
        let healer = sched::spawn_user("healer", healer_task, hu.1,
            core::ptr::addr_of!(KS_HEALER) as usize + TASK_STACK,
            mem::build_user_space(hu, NO_DEVICE));
        sched::grant_cap(healer, CRASH_CAP, Capability::Endpoint(sched::CRASH_EP));

        // A patient with a transient fault: crashes once, then recovers.
        let tu = ustack(core::ptr::addr_of!(US_TRANSIENT) as usize);
        let transient = sched::spawn_user("transient", transient_task, tu.1,
            core::ptr::addr_of!(KS_TRANSIENT) as usize + TASK_STACK,
            mem::build_user_space(tu, NO_DEVICE));
        // The healer holds transient's Restart cap at cap slot 1; the crash
        // notification carries badge 1 so the healer uses that exact cap.
        sched::grant_cap(healer, 1, Capability::Restart(transient));
        sched::set_crash_badge(transient, 1);

        // A patient that always crashes: exercises the retry bound.
        let fu = ustack(core::ptr::addr_of!(US_FLAKY) as usize);
        let flaky = sched::spawn_user("flaky", flaky_task, fu.1,
            core::ptr::addr_of!(KS_FLAKY) as usize + TASK_STACK,
            mem::build_user_space(fu, NO_DEVICE));
        sched::grant_cap(healer, 2, Capability::Restart(flaky));
        sched::set_crash_badge(flaky, 2);

        sched::spawn("idle", idle, core::ptr::addr_of!(KS_IDLE) as usize + TASK_STACK);
```

- [ ] **Step 6: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 7: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS`, including `sched: task 'transient' killed by LoadPageFault`, `heal: restarted 'transient' (attempt 1)`, `sched: task 'transient' exited (code 0)`, and `heal: giving up on 'flaky' after 2 restarts (flagged for triage)`, with the RTC component, rogue rejection, ticks, and all prior milestones still present.

Troubleshooting (diagnose, don't weaken the test):
- If `transient` never recovers (no `exited (code 0)`): the `a0` generation read is being clobbered — confirm the `mv {g}, a0` asm is the first statement in `transient_task`.
- If the healer never restarts (no `heal: restarted`): the healer must be recv-blocked before the patient crashes — confirm spawn order puts `healer` before `transient`/`flaky`, and that `CRASH_EP` matches between the grant and `exit_current`.
- If you see `heal: no healer for 'transient'`: same ordering issue.

- [ ] **Step 8: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat: Phase 5b live - a user-space healer restarts a crashed component (caged); transient recovers, flaky is bounded"
```

---

## Task 7: docs — learning note, roadmap, glossary; final verification

**Files:**
- Create: `docs/learning/0015-self-healing-the-caged-fix.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md`

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0015-self-healing-the-caged-fix.md`:

```markdown
# 0015 — Self-healing, step two: the caged fix (Phase 5b)

**One-line:** the self-healer now *acts* — an isolated user-space component
restarts a crashed component — and the kernel is the cage that keeps that
power bounded.

## What changed
- A `healer` U-mode component blocks on a reserved crash endpoint. When the
  kernel contains and diagnoses a crash (5a), `exit_current` notifies the
  healer (reusing the IPC rendezvous), delivering a badge that is the cap
  index of the crashed component's `Restart` capability.
- A new `restart(cap_idx)` syscall: the kernel capability-checks it, enforces
  a per-task retry bound (`MAX_RESTARTS`), re-forges the task's first-run
  context (its address space, stacks, and data persist), and logs.
- A `transient` patient crashes once and recovers; the always-crashing
  `flaky` is restarted only up to the bound, then flagged for triage.

## The two ideas worth keeping
1. **Isolation + a capability cage make an acting agent safe.** The agent
   (healer) runs unprivileged in user space and can do only what a capability
   grants. The *kernel* enforces the bound and logs — so even a buggy or
   compromised healer cannot restart-loop. Agency in user space; enforcement
   in the kernel (ADR 0005's safety cage).
2. **A restart is just re-forging the first-run context.** Nothing is
   reaped or re-allocated — the slot, `satp`, stacks, and data page persist.
   To let a transient fault differ from a permanent one, the kernel hands the
   task its **launch generation** in `a0` (`user_trampoline` does `mv a0, s3`):
   generation 0 = first run, >0 = a restart. The transient patient crashes
   only on generation 0.

## Why act only after diagnosis (5a before 5b)
Recognize and explain a problem deterministically *before* anything gains the
authority to change the system in response — and then confine that authority.

## Proof
`transient` crashes (LoadPageFault) → diagnosed (KB-0005) → `heal: restarted
'transient' (attempt 1)` → `transient` exits 0 (recovered). `flaky` crashes
repeatedly → restarted to the bound → `heal: giving up on 'flaky' after 2
restarts (flagged for triage)`. The kernel, the RTC component, and the
heartbeat run throughout.
```

- [ ] **Step 2: Update the roadmap**

In `docs/roadmap/roadmap.md`, replace the `### Phase 5b — The caged fix` block (currently the placeholder ending at the `give up ... after N attempts` line) with:

```markdown
### Phase 5b — The caged fix  *(done — 2026-06-20)*

- **Goal:** an isolated, capability-gated **user-space** healer that the
  kernel notifies of a crash and that applies the playbook — a **bounded,
  reversible, logged** restart — recovering the component.
- **You learn:** isolation + a capability cage make an acting agent safe
  (agency in user space, enforcement in the kernel); a restart is re-forging
  the first-run context, with the launch generation handed to the task (see
  [learning note 0015](../learning/0015-self-healing-the-caged-fix.md)).
- **Done when:** `./tools/test-qemu.ps1` shows a `transient` component crash,
  get restarted by the healer, and run to completion (recovered), while an
  always-crashing `flaky` is restarted only up to the bound and then flagged.
  QEMU-only.

**Phase 5 (self-healing seed) is complete:** the OS detects, deterministically
diagnoses (5a), and applies a caged, bounded fix (5b) for a contained
component crash.
```

- [ ] **Step 3: Add the glossary entry**

In `docs/glossary.md`, add one genuinely new term after the existing
self-healing cluster (after the `Diagnosis (rule engine)` line):

```markdown
- **Launch generation** — the count of how many times a task has been (re)started, handed to the task by the kernel at launch (in `a0`). It lets a component tell a fresh start from a restart — e.g. so a transient fault can be retried into success.
```

- [ ] **Step 4: Update the learning-notes index**

In `docs/learning/README.md`, add after the 0014 line:

```markdown
- [0015 — Self-healing, step two: the caged fix (Phase 5b)](0015-self-healing-the-caged-fix.md)
```

- [ ] **Step 5: Final verification (run all, paste results)**

Run: `cargo test -p kernel-arch-riscv64` → expect all green.
Run: `cargo build --workspace` → expect success.
Run (PowerShell): `./tools/check-references.ps1` → expect PASS (the new note + roadmap links resolve).
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0015-self-healing-the-caged-fix.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: Phase 5b learning note, roadmap (Phase 5 complete), glossary"
```

---

## Done-when checklist (maps to spec §1)

- [ ] **Recovery** — smoke shows `transient` killed → `heal: restarted 'transient' (attempt 1)` → `transient` exited code 0, with the system still running.
- [ ] **The bound** — smoke shows `heal: giving up on 'flaky' after 2 restarts (flagged for triage)`.
- [ ] **Host tests** — `cap::restart_target` (hit/wrong-type/empty/oob), `task::can_restart` (under/at/over bound), `forge_user_context` carries the generation in `s[3]`.
- [ ] `check-references` clean; `cargo build --workspace` green; `BOOT TEST PASS`.
```
