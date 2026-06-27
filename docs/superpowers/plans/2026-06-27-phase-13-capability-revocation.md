# Phase 13 — Capability revocation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The kernel revokes an endpoint capability — sweeping every other component's cap table to clear all copies — so a holder's next use of it fails. Authorized by holding the cap.

**Architecture:** Transitive sweep-and-clear: a cleared cap slot is `None`, so the existing `cap_lookup` returns `None` and IPC fails with no new hot-path check. A `revoke(ep_cap)` syscall sweeps every holder except the caller.

**Tech Stack:** Rust `no_std` kernel (`arch/riscv64`, `kernel`), pure host-tested `cap.rs`, QEMU riscv64, PowerShell two-boot harness.

**Spec:** `docs/superpowers/specs/2026-06-27-phase-13-capability-revocation-design.md`

## Global Constraints

- **Commits:** Conventional Commits, NO Claude co-author; author Kathir (signing automated).
- **Pure cap logic host-tested:** `cargo test -p kernel-arch-riscv64`. **Build:** `./tools/build.ps1`. **Boot test:** `./tools/test-qemu.ps1`.
- **U-mode task code** lives in `.user_text`, no calls into kernel `.text`/`.rodata` (inline asm via the `sys_*` wrappers), reports via syscalls. New components follow the existing `needy_task`/`rtc_server` pattern.
- The demo uses a **dedicated** `LEASE_EP` so it does not disturb the RTC / delegation / self-healing proofs.

---

## File Structure

- `arch/riscv64/src/cap.rs` — add `revoke_in_caps` (pure) + tests.
- `arch/riscv64/src/syscall.rs` — `Syscall::Revoke` (a7 = 12), decode + dispatch + test.
- `arch/riscv64/src/sched.rs` — `revoke_endpoint`, `ipc_revoke`.
- `kernel/src/main.rs` — `sys_revoke` wrapper, `LEASE_EP`/cap constants, `lease_server` + `tenant` tasks, boot wiring, stacks, `MAX_TASKS` 20→22.
- `tools/test-qemu.ps1` — revocation assertions.
- `docs/learning/0031-capability-revocation.md`, `docs/learning/README.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md` — docs.

---

## Task 1: `cap::revoke_in_caps` — clear all copies of an endpoint cap

**Files:**
- Modify: `arch/riscv64/src/cap.rs` (impl + tests)

**Interfaces:**
- Produces: `pub fn revoke_in_caps(caps: &mut [Option<Capability>], ep: EndpointId) -> usize`

- [ ] **Step 1: Write the failing tests** (in `cap.rs` tests)

```rust
    #[test]
    fn revoke_clears_matching_endpoint_caps_and_counts() {
        let mut caps = [
            Some(Capability::Endpoint(7)),
            Some(Capability::Endpoint(3)),
            Some(Capability::Endpoint(7)),
            Some(Capability::Restart(7)),
        ];
        assert_eq!(revoke_in_caps(&mut caps, 7), 2, "two Endpoint(7) caps cleared");
        assert_eq!(caps[0], None);
        assert_eq!(caps[2], None);
        assert_eq!(caps[1], Some(Capability::Endpoint(3)), "a different endpoint id is untouched");
        assert_eq!(caps[3], Some(Capability::Restart(7)), "a non-endpoint cap with the same number is untouched");
    }

    #[test]
    fn revoke_absent_endpoint_clears_nothing() {
        let mut caps = [Some(Capability::Endpoint(1)), None, Some(Capability::Randomness)];
        assert_eq!(revoke_in_caps(&mut caps, 9), 0);
        assert_eq!(caps[0], Some(Capability::Endpoint(1)));
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p kernel-arch-riscv64 revoke_`
Expected: FAIL — `revoke_in_caps` not found.

- [ ] **Step 3: Implement** (in `cap.rs`, after `cap_at`)

```rust
/// Clear every capability slot in `caps` holding `Endpoint(ep)`, returning the
/// number cleared. This is how the kernel revokes an endpoint from one holder:
/// a cleared slot is `None`, so `cap_lookup` later returns `None` and the
/// holder's IPC fails — no new check on the hot path. Other endpoint ids and
/// other capability types are untouched. Pure.
pub fn revoke_in_caps(caps: &mut [Option<Capability>], ep: EndpointId) -> usize {
    let mut n = 0;
    for slot in caps.iter_mut() {
        if matches!(slot, Some(Capability::Endpoint(id)) if *id == ep) {
            *slot = None;
            n += 1;
        }
    }
    n
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p kernel-arch-riscv64 revoke_`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/cap.rs
git commit -m "feat(cap): revoke_in_caps — clear all copies of an endpoint cap"
```

---

## Task 2: `Syscall::Revoke` — decode a7 = 12

**Files:**
- Modify: `arch/riscv64/src/syscall.rs` (enum, decode, dispatch, test)

**Interfaces:**
- Produces: `Syscall::Revoke`; `decode_syscall(12) == Syscall::Revoke`; dispatch → `sched::ipc_revoke`.

- [ ] **Step 1: Write the failing test** (near the other decode tests)

```rust
    #[test]
    fn decodes_revoke() {
        assert_eq!(decode_syscall(12), Syscall::Revoke);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 decodes_revoke`
Expected: FAIL — no `Revoke` variant.

- [ ] **Step 3: Implement**

Add the variant to `enum Syscall` (after `Grant`):

```rust
    /// `revoke(ep_cap)` — revoke an endpoint capability from every other holder
    /// (the caller must hold it; it keeps its own).
    Revoke,
```

Add the decode arm (after `11 => Syscall::Grant,`):

```rust
        12 => Syscall::Revoke,
```

Add the dispatch arm (after the `Syscall::Grant` arm):

```rust
        Syscall::Revoke => {
            crate::sched::ipc_revoke(frame);
            Outcome::Resume
        }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 decodes_revoke`
Expected: FAIL to compile until Task 3 adds `ipc_revoke` — that is expected; this task's *test* passes once Task 3 lands. (Alternatively, add a stub `pub fn ipc_revoke(_frame: &mut crate::trap::TrapFrame) {}` now and fill it in Task 3.) To keep the build green, **add the real `ipc_revoke` (Task 3) before building.**

- [ ] **Step 5: Commit** (with Task 3, since dispatch references `ipc_revoke`)

---

## Task 3: `revoke_endpoint` + `ipc_revoke`

**Files:**
- Modify: `arch/riscv64/src/sched.rs`
- Test: boot test (Task 5).

**Interfaces:**
- Consumes: `cap::{cap_lookup, revoke_in_caps}`, `MAX_TASKS`, `SCHED`, `EndpointId`.
- Produces: `pub fn revoke_endpoint(ep: EndpointId, except: usize) -> usize`; `pub fn ipc_revoke(frame: &mut crate::trap::TrapFrame)`.

- [ ] **Step 1: Add `revoke_endpoint` and `ipc_revoke`** (in `sched.rs`, near `ipc_grant`)

```rust
/// Revoke endpoint `ep` from every task except `except` (the caller, which keeps
/// its own cap): clear all `Endpoint(ep)` capabilities from their cap tables.
/// Returns the number of capabilities cleared. Transitive — every copy in every
/// CSpace is invalidated at once, with no derivation tree.
#[cfg(target_arch = "riscv64")]
pub fn revoke_endpoint(ep: EndpointId, except: usize) -> usize {
    SCHED.with(|s| {
        let mut n = 0;
        for i in 0..MAX_TASKS {
            if i == except {
                continue;
            }
            if let Some(task) = s.tasks[i].as_mut() {
                n += crate::cap::revoke_in_caps(&mut task.caps, ep);
            }
        }
        n
    })
}

/// Service a `revoke` syscall: `a0` = the slot of an `Endpoint` cap the caller
/// holds. Revokes that endpoint from every *other* holder (the caller keeps its
/// own), logs the count, and returns it in `a0` — or `usize::MAX` if the caller
/// does not hold the endpoint capability (the authorization guard).
#[cfg(target_arch = "riscv64")]
pub fn ipc_revoke(frame: &mut crate::trap::TrapFrame) {
    let ep_idx = frame.regs[9]; // a0
    let result = SCHED.with(|s| {
        let cur = s.current;
        let ep = cap_lookup(&s.tasks[cur].as_ref().unwrap().caps, ep_idx)?;
        Some((cur, ep, s.tasks[cur].as_ref().unwrap().name))
    });
    match result {
        None => frame.regs[9] = usize::MAX,
        Some((cur, ep, name)) => {
            let n = revoke_endpoint(ep, cur);
            crate::println!("cap: '{name}' revoked endpoint {ep} from {n} holder(s)");
            frame.regs[9] = n;
        }
    }
}
```

(Confirm `cap_lookup` and `EndpointId` are already imported in `sched.rs` — they
are, from the IPC paths. `revoke_endpoint` calls `SCHED.with` and then
`ipc_revoke` calls `revoke_endpoint` which calls `SCHED.with` again: the first
`SCHED.with` in `ipc_revoke` has returned before `revoke_endpoint` runs, so there
is no nested borrow.)

- [ ] **Step 2: Build**

Run: `./tools/build.ps1`
Expected: clean build (the `Syscall::Revoke` dispatch from Task 2 now resolves).

- [ ] **Step 3: Run the decode test**

Run: `cargo test -p kernel-arch-riscv64 decodes_revoke`
Expected: PASS.

- [ ] **Step 4: Commit** (Tasks 2 + 3 together)

```bash
git add arch/riscv64/src/syscall.rs arch/riscv64/src/sched.rs
git commit -m "feat(cap): revoke syscall — transitive endpoint revocation (a7=12)"
```

---

## Task 4: `sys_revoke` wrapper + the lease/tenant demo

**Files:**
- Modify: `kernel/src/main.rs` (wrapper, constants, two tasks, boot wiring, stacks, `MAX_TASKS`)
- Modify: `arch/riscv64/src/sched.rs` (`MAX_TASKS` + the `Scheduler::new` initializer)

**Interfaces:**
- Consumes: `sched::ipc_revoke`, `sys_revoke`, `LEASE_EP`.

- [ ] **Step 1: Add the `sys_revoke` U-mode wrapper** (in `kernel/src/main.rs`, near `sys_grant`)

```rust
    /// revoke syscall (a7 = 12): a0 = the slot of an Endpoint cap the caller
    /// holds. Revokes that endpoint from every other holder. Returns the count
    /// revoked, or `usize::MAX` if the caller lacks the capability.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability.
    #[inline(always)]
    unsafe fn sys_revoke(ep: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 12usize,
            inout("a0") ep => ret,
            options(nostack),
        );
        ret
    }
```

- [ ] **Step 2: Add the lease constants** (near the `GRANT_EP` constants)

```rust
    /// The lease endpoint for the Phase 13 revocation demo (distinct from EP0 and
    /// the grant channel). Both the lease server and the tenant hold a cap to it
    /// at cap slot 0; the server also revokes it.
    const LEASE_EP: usize = 7;
    const LEASE_CAP: usize = 0; // Endpoint(LEASE_EP), held by server and tenant
    const LEASE_REPLY_SLOT: usize = 1; // the server's reply-cap slot
    /// The tenant's exit code when it used the cap once and its second (revoked)
    /// use was rejected.
    const TENANT_REVOKED_CODE: usize = 13;
```

- [ ] **Step 3: Add the `lease_server` and `tenant` tasks** (near `broker_task`)

```rust
    /// `lease_server` (Phase 13): holds Endpoint(LEASE_EP) for recv + revoke
    /// authority. It answers the tenant's first call, then REVOKES LEASE_EP —
    /// clearing the tenant's delegated cap while keeping its own — and exits.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn lease_server() -> ! {
        unsafe {
            let _ = sys_recv(LEASE_CAP, LEASE_REPLY_SLOT); // tenant's call 1
            sys_reply(LEASE_REPLY_SLOT, 1);
            // Take back the leased authority from every other holder (the tenant).
            let _ = sys_revoke(LEASE_CAP);
            sys_exit(0)
        }
    }

    /// `tenant` (Phase 13): holds Endpoint(LEASE_EP). It calls the lease server
    /// twice; the first succeeds, the second fails because its cap was revoked in
    /// between. Exits with TENANT_REVOKED_CODE iff "call 1 ok, call 2 revoked".
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn tenant_task() -> ! {
        unsafe {
            let r1 = sys_call(LEASE_CAP, 1);
            let r2 = sys_call(LEASE_CAP, 2);
            if r1 != usize::MAX && r2 == usize::MAX {
                sys_exit(TENANT_REVOKED_CODE)
            } else {
                sys_exit(99)
            }
        }
    }
```

- [ ] **Step 4: Spawn the demo** (in `kmain`, after the broker block at ~line 103). Spawn `lease_server` first (so it recv-blocks before the tenant calls), then `tenant`:

```rust
        // Phase 13 — capability revocation. The lease server answers the tenant
        // once, then revokes the leased endpoint; the tenant's second call fails.
        let lsu = ustack(core::ptr::addr_of!(US_LEASE) as usize);
        let lease = sched::spawn_user("lease", lease_server, lsu.1,
            core::ptr::addr_of!(KS_LEASE) as usize + TASK_STACK,
            mem::build_user_space(lsu, NO_DEVICE));
        sched::grant_cap(lease, LEASE_CAP, Capability::Endpoint(LEASE_EP));

        let tnu = ustack(core::ptr::addr_of!(US_TENANT) as usize);
        let tenant = sched::spawn_user("tenant", tenant_task, tnu.1,
            core::ptr::addr_of!(KS_TENANT) as usize + TASK_STACK,
            mem::build_user_space(tnu, NO_DEVICE));
        sched::grant_cap(tenant, LEASE_CAP, Capability::Endpoint(LEASE_EP));
```

- [ ] **Step 5: Declare the stacks** (with the other `KS_*` / `US_*` declarations; copy the `US_BROKER`/`KS_BROKER` form)

```rust
    static mut KS_LEASE: KStack = [0; TASK_STACK];
    static mut KS_TENANT: KStack = [0; TASK_STACK];
```

and with the `#[link_section = ".user_data"]` UStacks:

```rust
    #[link_section = ".user_data"]
    static mut US_LEASE: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_TENANT: UStack = UStack([0; USER_STACK_SIZE]);
```

- [ ] **Step 6: Raise `MAX_TASKS` 20 → 22** in `arch/riscv64/src/sched.rs` (and add **two** `None` to the `Scheduler::new` initializer array — it currently has 20).

- [ ] **Step 7: Build**

Run: `./tools/build.ps1`
Expected: clean build.

- [ ] **Step 8: Commit**

```bash
git add kernel/src/main.rs arch/riscv64/src/sched.rs
git commit -m "feat(cap): sys_revoke wrapper + lease/tenant revocation demo"
```

---

## Task 5: Revocation assertions in the boot test

**Files:**
- Modify: `tools/test-qemu.ps1`

**Interfaces:** boot-1 markers `cap: 'lease' revoked endpoint 7 from 1 holder(s)` and `sched: task 'tenant' exited (code 13)`.

- [ ] **Step 1: Add the boot-1 assertions** to `$mustMatch1`:

```powershell
    "cap: 'lease' revoked endpoint 7 from 1 holder\(s\)",
    "sched: task 'tenant' exited \(code 13\)",
```

- [ ] **Step 2: Update the PASS banner** — append: `; and Phase 13 capability revocation: a lease server hands a 'tenant' an endpoint capability, answers one call, then REVOKES it - the tenant's second call is rejected (its cap was swept from its table) - authority can be taken back, transitively.`

- [ ] **Step 3: Run the full boot test**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS: …; and Phase 13 capability revocation: …`.

Debugging aids:
- If `tenant` exits 99 → the ordering is off (revoke didn't happen before call 2): confirm the server runs `reply` then `revoke` then `exit`, and the tenant is spawned *after* the server (so the server is recv-blocked first).
- If the revoke count is not 1 → another task holds `Endpoint(7)`; `LEASE_EP = 7` must be unique (EP0=0, CRASH_EP=1, ENTROPY_EP=2, DEFER_EP=3, BLK_EP=4, GATE_EP=5, GRANT_EP=6).
- If `scheduler full` panics → raise `MAX_TASKS` (Task 4 Step 6) — 22 should fit (21 tasks).

- [ ] **Step 4: Commit**

```bash
git add tools/test-qemu.ps1
git commit -m "test: capability revocation demo (Phase 13)"
```

---

## Task 6: Documentation

**Files:**
- Create: `docs/learning/0031-capability-revocation.md`
- Modify: `docs/learning/README.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md`

- [ ] **Step 1: Learning note** `docs/learning/0031-capability-revocation.md` — short. Cover: what changed (a `revoke` syscall that sweeps every holder's CSpace clearing an endpoint cap); the idea worth keeping (transitive revocation via sweep needs no derivation tree — a cleared slot is just `None`, so the existing lookup already fails; authority = holding the cap; the caller keeps its own); the alternative (epoch/generation revocation is O(1) and re-grantable, but needs the cap to carry a generation — deferred); the proof (tenant's first call ok, second rejected after revoke); what's next (generations, a derivation tree for descendant-only revocation). Follow `0026` (delegation) in style — they are the delegate/revoke pair.

- [ ] **Step 2: Index** in `docs/learning/README.md` (`0031` line).

- [ ] **Step 3: Roadmap** — replace `## Phase 13+ — Breadth` with a completed `## Phase 13 — Capability revocation (done — 2026-06-27)` (goal / you-learn / done-when citing note 0031), and re-add a `## Phase 14+ — Breadth` placeholder.

- [ ] **Step 4: Glossary** — add **Capability revocation** near the delegation/capability terms.

- [ ] **Step 5: Cross-reference check**

Run: `./tools/check-references.ps1`
Expected: passes.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0031-capability-revocation.md docs/learning/README.md docs/roadmap/roadmap.md docs/glossary.md
git commit -m "docs: Phase 13 capability revocation — learning note 0031, roadmap, glossary"
```

---

## Self-Review (completed during planning)

- **Spec coverage:** `revoke_in_caps` → Task 1; `Revoke` decode → Task 2; `revoke_endpoint`/`ipc_revoke` → Task 3; wrapper + demo + MAX_TASKS → Task 4; proof → Task 5; docs → Task 6. All spec sections map to a task.
- **Type consistency:** `revoke_in_caps(&mut [Option<Capability>], EndpointId) -> usize` (Tasks 1, 3); `revoke_endpoint(ep, except) -> usize` and `ipc_revoke(frame)` (Tasks 3, 4); `sys_revoke(ep) -> usize` (Task 4); `LEASE_EP = 7` unique vs EP0..GRANT_EP(6); `LEASE_CAP = 0` matches both grants and the wrapper arg.
- **Open verification during execution:** confirm `cap_lookup`/`EndpointId` are in scope in `sched.rs` (Task 3) — they are, used by the IPC paths; confirm the revoke ordering yields `tenant` exit 13 (Task 5); confirm `MAX_TASKS` 22 fits (Task 4 Step 6 / Task 5).
```
