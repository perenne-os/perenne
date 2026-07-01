# One-shot reply capabilities Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let a server hold multiple outstanding calls and reply to them in any order, by minting a one-shot `Reply` capability per received Call instead of tracking a single caller.

**Architecture:** On a received Call, `recv(ep, reply_slot)` installs `Reply(caller)` into the server's `caps[reply_slot]`; `reply(reply_slot, msg)` looks it up, wakes the caller, and consumes the cap. `Task.caller` survives only as the transient that carries the caller slot from `ipc_call` to the server's `recv` continuation.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU. Host tests: `cargo test -p kernel-arch-riscv64`. Bare: `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/design/specs/2026-06-23-reply-capabilities-design.md`

**Existing-code facts (verified):**
- `cap.rs`: `Capability { Endpoint(EndpointId), Restart(usize), Randomness, Interrupt(u32) }`; lookups end with `_ => None`. `CAP_SLOTS = 4`.
- `sched.rs` `ipc_recv(frame)`: `a0`=ep cap (`regs[9]`); finds a waiting `Send` or `Call` peer; on a Call sets `s.tasks[cur].caller = Some(si)` and the peer `AwaitingReply`; returns the message (immediate `Got`, or `Block`→wake→`self.message`). `ipc_reply(frame)`: reads `msg` from `regs[9..13]` (a0=badge), takes `self.caller`, wakes it if `AwaitingReply`. `ipc_call` sets `s.tasks[ri].caller = Some(cur)` when waking a recv-blocked server. `find_blocked`, `write_message`, `CAP_SLOTS`, `Message`, `TaskState` in scope.
- `kernel/src/main.rs`: `sys_recv(cap)` (a7=5, a0=cap→badge, out a1..a3), `sys_reply(badge)` (a7=8, a0=badge→ret), `sys_call(cap, badge)` (a7=7). `rtc_server` calls `sys_recv(EP_CAP)` then `sys_reply(t)`; `rtc_client` `sys_call(EP_CAP, 1)`. `healer_task` calls `sys_recv(CRASH_CAP)`. `EP_CAP`=0, `CRASH_CAP`=0, `ENTROPY_CAP`=0. `MAX_TASKS`=12; cast is 10 tasks (…rnguser, idle).

**Cast after this plan (MAX_TASKS → 16):** …, `rnguser`, **`deferrer`, `dclientA`, `dclientB`**, `idle`. `deferrer` precedes its clients so it recv-blocks first.

---

## Task 1: the `Reply` capability

**Files:**
- Modify: `arch/riscv64/src/cap.rs`

- [ ] **Step 1: Write the failing test**

In `arch/riscv64/src/cap.rs`, add to `tests`:

```rust
    #[test]
    fn reply_caller_returns_the_caller_slot() {
        let caps = [None, Some(Capability::Reply(4)), Some(Capability::Randomness)];
        assert_eq!(reply_caller(&caps, 1), Some(4));
        assert_eq!(reply_caller(&caps, 2), None, "Randomness is not a Reply cap");
        assert_eq!(reply_caller(&caps, 0), None, "empty slot");
        assert_eq!(reply_caller(&caps, 9), None, "out of range");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 cap::`
Expected: FAIL — `Capability::Reply`/`reply_caller` do not exist.

- [ ] **Step 3: Add the variant and the lookup**

In `arch/riscv64/src/cap.rs`, extend `Capability`:

```rust
    /// Authority to `wait_irq` on this IRQ number (a device's interrupt).
    Interrupt(u32),
    /// One-shot authority to reply to the caller in this scheduler slot (minted
    /// by the kernel when a server receives a Call; consumed by `reply`).
    Reply(usize),
}
```

Add, after `interrupt_irq`:

```rust
/// The caller a one-shot `Reply` capability at `idx` answers. `None` for an
/// empty slot, an out-of-range index, or the wrong capability type.
pub fn reply_caller(caps: &[Option<Capability>], idx: usize) -> Option<usize> {
    match caps.get(idx) {
        Some(Some(Capability::Reply(slot))) => Some(*slot),
        _ => None,
    }
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 cap::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/cap.rs
git commit -m "feat(cap): one-shot Reply capability + reply_caller lookup (host-tested)"
```

---

## Task 2: mint on receive, reply through the cap

**Files:**
- Modify: `arch/riscv64/src/sched.rs`

- [ ] **Step 1: `recv` mints a `Reply` cap at the server's chosen slot**

In `arch/riscv64/src/sched.rs`, replace the body of `ipc_recv` with one that reads `a1`=`reply_slot` and mints on a Call (both the immediate and the woken paths):

```rust
pub fn ipc_recv(frame: &mut crate::trap::TrapFrame) {
    let cap_idx = frame.regs[9];
    let reply_slot = frame.regs[10]; // a1: where to install a minted reply cap
    let step = SCHED.with(|s| {
        let cur = s.current;
        let ep = match cap_lookup(&s.tasks[cur].as_ref().unwrap().caps, cap_idx) {
            Some(ep) => ep,
            None => return RecvStep::BadCap,
        };
        // A waiting one-way Send, or a Call (which expects a reply).
        let waiting = find_blocked(s, ep, IpcRole::Send)
            .map(|si| (si, false))
            .or_else(|| find_blocked(s, ep, IpcRole::Call).map(|si| (si, true)));
        match waiting {
            Some((si, is_call)) => {
                let msg = s.tasks[si].as_ref().unwrap().message;
                if is_call {
                    s.tasks[si].as_mut().unwrap().state = TaskState::AwaitingReply;
                    mint_reply_cap(s, cur, si, reply_slot);
                } else {
                    s.tasks[si].as_mut().unwrap().state = TaskState::Ready;
                }
                RecvStep::Got(msg)
            }
            None => {
                crate::println!("ipc: '{}' blocks on recv", s.tasks[cur].as_ref().unwrap().name);
                RecvStep::Block(ep)
            }
        }
    });
    match step {
        RecvStep::BadCap => frame.regs[9] = usize::MAX,
        RecvStep::Got(msg) => write_message(frame, msg),
        RecvStep::Block(ep) => {
            block_current(ep, IpcRole::Recv);
            // Woken: a sender stored our inbox. If it was a Call, `ipc_call`
            // stashed the caller slot in `self.caller` — mint the reply cap.
            let msg = SCHED.with(|s| {
                let cur = s.current;
                if let Some(caller) = s.tasks[cur].as_mut().unwrap().caller.take() {
                    mint_reply_cap(s, cur, caller, reply_slot);
                }
                s.tasks[cur].as_ref().unwrap().message
            });
            write_message(frame, msg);
        }
    }
}

/// Install a one-shot `Reply(caller)` capability into `server`'s cap table at
/// `slot` (a no-op if `slot` is out of range — a server bug, not fatal).
#[cfg(target_arch = "riscv64")]
fn mint_reply_cap(s: &mut Scheduler, server: usize, caller: usize, slot: usize) {
    if slot < CAP_SLOTS {
        s.tasks[server].as_mut().unwrap().caps[slot] = Some(crate::cap::Capability::Reply(caller));
    }
}
```

(`Scheduler` is the type of the `SCHED.with` argument; `mint_reply_cap` takes `&mut Scheduler`.)

- [ ] **Step 2: `reply` consumes the `Reply` cap**

In `arch/riscv64/src/sched.rs`, replace `ipc_reply`:

```rust
/// Service a `reply` syscall: `a0` = the reply-cap slot, `a1` = badge, `a2..a4`
/// = data. Look up the one-shot `Reply` capability at that slot, wake the
/// caller it names, and consume the cap. `a0` becomes `0` on success, or
/// `usize::MAX` if the slot holds no reply cap (or the caller is gone).
#[cfg(target_arch = "riscv64")]
pub fn ipc_reply(frame: &mut crate::trap::TrapFrame) {
    let reply_slot = frame.regs[9];
    let msg = Message {
        badge: frame.regs[10],
        data: [frame.regs[11], frame.regs[12], frame.regs[13]],
    };
    let ok = SCHED.with(|s| {
        let cur = s.current;
        let caller = match crate::cap::reply_caller(&s.tasks[cur].as_ref().unwrap().caps, reply_slot) {
            Some(c) => c,
            None => return false,
        };
        // Consume the one-shot cap regardless of the caller's state.
        s.tasks[cur].as_mut().unwrap().caps[reply_slot] = None;
        let awaiting = matches!(
            s.tasks[caller].as_ref(),
            Some(t) if t.state == TaskState::AwaitingReply
        );
        if awaiting {
            s.tasks[caller].as_mut().unwrap().message = msg;
            s.tasks[caller].as_mut().unwrap().state = TaskState::Ready;
            true
        } else {
            false
        }
    });
    frame.regs[9] = if ok { 0 } else { usize::MAX };
}
```

- [ ] **Step 3: Verify host tests + bare build**

Run: `cargo test -p kernel-arch-riscv64` → expect PASS (existing suite; gated IPC excluded on host).
Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf` → expect SUCCESS.

(The kernel binary still compiles — `sys_recv`/`sys_reply` are independent asm wrappers — but the IPC ABI now mismatches the old wrappers; Task 4 fixes them. Smoke is intentionally not run here.)

- [ ] **Step 4: Commit**

```bash
git add arch/riscv64/src/sched.rs
git commit -m "feat(sched): mint a one-shot Reply cap on receive; reply through it"
```

---

## Task 3: update the smoke test for out-of-order replies (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Add the deferrer-client assertions**

In `tools/test-qemu.ps1`, add to `$mustMatch` (after the RTC `sched: task 'client' exited …` line):

```powershell
    "sched: task 'dclientA' exited \(code 417\)",
    "sched: task 'dclientB' exited \(code 433\)",
```

Update the header comment + PASS message to note a `deferrer` server holds two
calls in flight and replies out of order (B before A) via one-shot reply
capabilities; each client exits with its own reply value.

- [ ] **Step 2: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — the two `dclient…` lines are absent (no deferrer demo yet; and the RTC ABI mismatch from Task 2 may also disrupt the RTC line — both fixed in Task 4).

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts out-of-order replies via reply capabilities (red)"
```

---

## Task 4: the new ABI wrappers + RTC conversion + deferrer demo (green)

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Update `sys_recv` to take a reply slot**

In `kernel/src/main.rs`, replace `sys_recv`:

```rust
    /// recv syscall (a7 = 5): a0 = endpoint cap index, a1 = reply slot (where
    /// the kernel installs a one-shot Reply cap if the message is a Call; for a
    /// one-way Send it is unused). Returns the badge in a0.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and may block us.
    #[inline(always)]
    unsafe fn sys_recv(cap: usize, reply_slot: usize) -> usize {
        let badge;
        core::arch::asm!(
            "ecall",
            in("a7") 5usize,
            inout("a0") cap => badge,
            inout("a1") reply_slot => _,
            out("a2") _,
            out("a3") _,
            options(nostack),
        );
        badge
    }
```

- [ ] **Step 2: Update `sys_reply` to take the reply slot**

In `kernel/src/main.rs`, replace `sys_reply`:

```rust
    /// reply syscall (a7 = 8): a0 = reply-cap slot, a1 = badge, a2..a4 = data
    /// (zero here). Wakes the caller named by the one-shot Reply cap and
    /// consumes it. Returns 0, or `usize::MAX` if the slot holds no reply cap.
    ///
    /// # Safety
    /// Always sound; the kernel validates the reply capability.
    #[inline(always)]
    unsafe fn sys_reply(reply_slot: usize, badge: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 8usize,
            inout("a0") reply_slot => ret,
            in("a1") badge,
            in("a2") 0usize,
            in("a3") 0usize,
            options(nostack),
        );
        ret
    }
```

- [ ] **Step 3: Convert the RTC server and fix the healer's recv**

In `kernel/src/main.rs`, after `const EP_CAP: usize = 0;`, add:

```rust
    /// The cap slot the RTC server lets the kernel mint its reply cap into.
    const RTC_REPLY_SLOT: usize = 1;
```

In `rtc_server`, change the recv and reply:

```rust
            let _req = unsafe { sys_recv(EP_CAP, RTC_REPLY_SLOT) };
```

and:

```rust
            unsafe { sys_reply(RTC_REPLY_SLOT, t) };
```

In `healer_task`, change its recv (one-way; the reply slot is unused — pass 0):

```rust
            let cap_idx = unsafe { sys_recv(CRASH_CAP, 0) };
```

- [ ] **Step 4: Add the deferrer endpoint, stacks, and components**

In `kernel/src/main.rs`, after the `ENTROPY_EP`/`ENTROPY_CAP`/`RNG_CAP`/`IRQ_CAP` constants, add:

```rust
    /// The endpoint the deferrer demo uses, and the cap slot the deferrer and
    /// its clients hold it in.
    const DEFER_EP: usize = 3;
    const DEFER_CAP: usize = 0;
```

Add kernel stacks (after `KS_RNGUSER`):

```rust
    static mut KS_DEFERRER: KStack = [0; TASK_STACK];
    static mut KS_DCLIENTA: KStack = [0; TASK_STACK];
    static mut KS_DCLIENTB: KStack = [0; TASK_STACK];
```

Add user stacks (after `US_RNGUSER`):

```rust
    #[link_section = ".user_data"]
    static mut US_DEFERRER: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_DCLIENTA: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_DCLIENTB: UStack = UStack([0; USER_STACK_SIZE]);
```

Add the components (near the other U-mode tasks, e.g. after `rnguser_task`):

```rust
    /// A server that holds two calls in flight and replies OUT OF ORDER, proving
    /// one-shot reply capabilities. It receives a call into reply slot 1 and a
    /// second into reply slot 2 (holding a Reply cap for each), then replies to
    /// the second before the first. Each reply returns `request | 0x100`.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn deferrer_task() -> ! {
        // SAFETY: we hold the endpoint cap at DEFER_CAP; recv blocks for a call.
        unsafe {
            let a = sys_recv(DEFER_CAP, 1); // call A -> Reply cap in slot 1
            let b = sys_recv(DEFER_CAP, 2); // call B -> Reply cap in slot 2
            sys_reply(2, b | 0x100); // reply B first
            sys_reply(1, a | 0x100); // then A
            sys_exit(0)
        }
    }

    /// A client of the deferrer: call with badge 0xa1, exit with the reply
    /// (0x1a1 = 417) — proving its call was tracked independently of B's.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn dclient_a_task() -> ! {
        // SAFETY: we hold the endpoint cap at DEFER_CAP.
        unsafe { sys_exit(sys_call(DEFER_CAP, 0xa1)) }
    }

    /// A client of the deferrer: call with badge 0xb1, exit with the reply
    /// (0x1b1 = 433).
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn dclient_b_task() -> ! {
        // SAFETY: we hold the endpoint cap at DEFER_CAP.
        unsafe { sys_exit(sys_call(DEFER_CAP, 0xb1)) }
    }
```

- [ ] **Step 5: Spawn the deferrer + clients and bump `MAX_TASKS`**

In `arch/riscv64/src/sched.rs`, bump the constant and the initializer:

```rust
/// Maximum concurrent tasks: the demo runs thirteen plus headroom.
pub const MAX_TASKS: usize = 16;
```

```rust
        Self { tasks: [None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None], current: 0 }
```

In `kernel/src/main.rs` `kmain`, insert just before the `idle` spawn (after the `rnguser` grant):

```rust
        // The deferrer demo: a server that holds two calls in flight and replies
        // out of order (proves one-shot reply caps). Spawn the server before its
        // clients so it recv-blocks first.
        let du = ustack(core::ptr::addr_of!(US_DEFERRER) as usize);
        let deferrer = sched::spawn_user("deferrer", deferrer_task, du.1,
            core::ptr::addr_of!(KS_DEFERRER) as usize + TASK_STACK,
            mem::build_user_space(du, NO_DEVICE));
        sched::grant_cap(deferrer, DEFER_CAP, Capability::Endpoint(DEFER_EP));

        let dau = ustack(core::ptr::addr_of!(US_DCLIENTA) as usize);
        let dclient_a = sched::spawn_user("dclientA", dclient_a_task, dau.1,
            core::ptr::addr_of!(KS_DCLIENTA) as usize + TASK_STACK,
            mem::build_user_space(dau, NO_DEVICE));
        sched::grant_cap(dclient_a, DEFER_CAP, Capability::Endpoint(DEFER_EP));

        let dbu = ustack(core::ptr::addr_of!(US_DCLIENTB) as usize);
        let dclient_b = sched::spawn_user("dclientB", dclient_b_task, dbu.1,
            core::ptr::addr_of!(KS_DCLIENTB) as usize + TASK_STACK,
            mem::build_user_space(dbu, NO_DEVICE));
        sched::grant_cap(dclient_b, DEFER_CAP, Capability::Endpoint(DEFER_EP));
```

- [ ] **Step 6: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 7: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS`, including `sched: task 'dclientA' exited (code 417)` and `sched: task 'dclientB' exited (code 433)`, with the RTC `client exited (code <time>)` line and every other milestone still present.

Troubleshooting (diagnose, don't weaken the test):
- A `dclient` exits 0 or hangs → the reply didn't reach it: confirm the deferrer recvs into slots 1 then 2 and replies `sys_reply(2,…)` then `sys_reply(1,…)`, and that `mint_reply_cap` ran (the server holds `Reply` caps in slots 1 and 2).
- The RTC `client` line is gone → the RTC ABI conversion: confirm `rtc_server` uses `sys_recv(EP_CAP, RTC_REPLY_SLOT)` / `sys_reply(RTC_REPLY_SLOT, t)`.
- Wrong codes → the reply transform is `request | 0x100` (0xa1→0x1a1=417, 0xb1→0x1b1=433).

- [ ] **Step 8: Commit**

```bash
git add arch/riscv64/src/sched.rs kernel/src/main.rs
git commit -m "feat: reply capabilities live - deferrer holds two calls, replies out of order; RTC on the new ABI"
```

---

## Task 5: docs — learning note, roadmap, glossary; final verification

**Files:**
- Create: `docs/learning/0021-reply-capabilities.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md`

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0021-reply-capabilities.md`:

```markdown
# 0021 — One-shot reply capabilities

**One-line:** a server can now hold several calls at once and answer them in
any order — each received call mints a one-shot capability to reply to it.

## What changed
- New `Capability::Reply(caller)` + `reply_caller` lookup.
- `recv(ep, reply_slot)` mints a `Reply` cap into the server's chosen cap slot
  when the message is a Call; `reply(reply_slot, msg)` wakes the named caller
  and consumes the cap (one-shot).
- Before, the kernel tracked a single `caller` per server, so a server had to
  reply before its next receive. `caller` now survives only as a transient (it
  carries the caller slot from `call` to the server's `recv` continuation); the
  durable binding is the reply cap — one per outstanding call.

## The ideas worth keeping
1. **One outstanding reply = one capability.** Tracking "who do I owe a reply"
   as a *field* allows exactly one; as a *capability per call*, a server can
   hold many and reply out of order — the deferrer holds a reply cap in slot 1
   and slot 2, then replies to slot 2 first.
2. **Mint on receive, consume on use.** The kernel creates the reply cap when
   the call arrives and clears it when used — so a reply cap is one-shot and a
   double-reply is simply a missing cap.
3. **No staleness guard needed in a blocking kernel.** A caller is
   `AwaitingReply` (blocked — it can't run, exit, or be reused) until replied,
   and the cap is consumed on use, so a `Reply` cap always names a valid waiting
   caller. (A reaping kernel would need a generation.)

## What this does *not* do yet
Forwarding a reply cap to *another* component needs capability delegation
through IPC (moving caps between cap tables) — a separate, larger feature. So a
reply cap stays in the server that minted it.

## Proof
`dclientA` and `dclientB` both call the `deferrer`; it receives both before
replying, then answers B then A. They exit `417` (0x1a1) and `433` (0x1b1) —
distinct values, delivered out of order.
```

- [ ] **Step 2: Update the roadmap**

In `docs/roadmap/roadmap.md`, find the "(Next candidates: …)" note after the "PLIC interrupt path" block (it lists "one-shot reply capabilities" first). Replace that parenthetical with:

```markdown
### One-shot reply capabilities  *(done — 2026-06-23)*

- **Goal:** let a server hold multiple calls in flight and reply out of order,
  by minting a one-shot reply capability per received call (replacing the
  single-caller binding).
- **You learn:** tracking one outstanding reply as a *capability per call*
  (minted on receive, consumed on reply) instead of a field; and why a blocking
  kernel needs no staleness guard (see
  [learning note 0021](../learning/0021-reply-capabilities.md)).
- **Done when:** `./tools/test-qemu.ps1` shows a deferrer server answer two
  clients out of order, each exiting with its own reply value. QEMU-only.

(Next candidates: capability delegation through IPC — enabling reply-cap
forwarding and more; interrupt-driven UART input; richer self-healing once a
filesystem exists.)
```

- [ ] **Step 3: Add a glossary entry**

In `docs/glossary.md`, add after the "call/reply (RPC)" entry:

```markdown
- **reply capability (one-shot)** — a capability the kernel mints when a server receives a `call`, naming the caller to answer. The server replies through it (in any order, even after receiving other calls) and the cap is consumed — letting a server hold several calls in flight at once.
```

- [ ] **Step 4: Update the learning-notes index**

In `docs/learning/README.md`, add after the 0020 line:

```markdown
- [0021 — One-shot reply capabilities](0021-reply-capabilities.md)
```

- [ ] **Step 5: Final verification (run all, paste results)**

Run: `cargo test -p kernel-arch-riscv64` → expect all green.
Run: `cargo build --workspace` → expect success.
Run (PowerShell): `./tools/check-references.ps1` → expect PASS.
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0021-reply-capabilities.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: one-shot reply capabilities - learning note, roadmap, glossary"
```

---

## Done-when checklist (maps to spec §1)

- [ ] **Multiple in flight, out of order** — smoke shows `dclientA exited (code 417)` and `dclientB exited (code 433)`, with the RTC client still working.
- [ ] **Host tests** — `cap::reply_caller` (hit/wrong-type/empty/oob).
- [ ] `check-references` clean; `cargo build --workspace` green; `BOOT TEST PASS`.
```
