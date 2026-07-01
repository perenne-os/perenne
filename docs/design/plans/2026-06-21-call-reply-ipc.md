# call/reply IPC Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add request/response (call/reply) IPC so a server can return a value to the task that called it, and convert the RTC component to use it.

**Architecture:** `call` = atomically send a request + block for the reply; the kernel records `caller` on the server when it receives a Call; `reply` wakes exactly that caller. Built entirely from the existing block/wake rendezvous — no new capability type, no asm changes.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU. Host tests: `cargo test -p kernel-arch-riscv64`. Bare: `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/design/specs/2026-06-21-call-reply-ipc-design.md`

**Existing-code facts (verified):**
- `task.rs`: `TaskState` = `Ready | Running | Exited | Blocked{endpoint, role}` (derives `PartialEq/Eq`); `IpcRole = Send | Recv`; `Message { badge, data:[usize;3] }`. `Task` fields end with `message`; constructed in `sched::spawn`, `sched::spawn_user`, and the sched test helper `task()`.
- `sched.rs`: `find_blocked(s, ep, role)`, `block_current(endpoint, role)` (sets `Blocked{..}`, switches), `write_message(frame, msg)` (a0=badge, a1..a3=data), `ipc_recv`/`ipc_send` (the rendezvous), `RecvStep{BadCap, Got(Message), Block(EndpointId)}`, `SendStep{BadCap, Delivered, Block}`. `ipc_send` delivers to a `find_blocked(ep, Recv)` peer or blocks `Send`. `restart` follows `ipc_send`.
- `syscall.rs`: `Syscall` ends `Recv, Restart, Unknown(usize)`; `decode_syscall` maps 1..6; `dispatch` routes them; `Outcome::Resume`.
- `kernel/src/main.rs`: `sys_recv`/`sys_send`/`sys_exit` are `#[inline(always)]` asm wrappers; `rtc_server` recvs once, reads the goldfish RTC via inline-asm `lw` at `0x10_1000`, and `sys_exit`s with the time; `rtc_client` `sys_send`s then `sys_exit(0)`; `EP_CAP`=0.
- Smoke asserts `sched: task 'rtc' exited \(code \d{15,}\)` (the server currently exits with the time).

---

## Task 1: the data model — `Call` role, `AwaitingReply` state, `caller` field

**Files:**
- Modify: `arch/riscv64/src/task.rs`
- Modify: `arch/riscv64/src/sched.rs` (the three `Task` construction sites)

- [ ] **Step 1: Add the role, state, and field in `task.rs`**

In `arch/riscv64/src/task.rs`, extend `TaskState` (add the variant after `Blocked`):

```rust
    /// Waiting at an IPC rendezvous on `endpoint` as a sender or receiver.
    /// Non-`Ready`, so `pick_next` skips it until a peer wakes it.
    Blocked { endpoint: crate::cap::EndpointId, role: IpcRole },
    /// A caller whose request a server has picked up, now blocked until that
    /// server `reply`s. On no endpoint queue (so no second server can re-match
    /// it); `pick_next` skips it. (Phase: call/reply IPC.)
    AwaitingReply,
```

Extend `IpcRole`:

```rust
pub enum IpcRole {
    Send,
    Recv,
    /// A caller queued at an endpoint waiting for a server to pick up its
    /// request (a server's `recv` matches `Send` or `Call`).
    Call,
}
```

Add the `caller` field to `Task` (after `crash_badge`):

```rust
    /// The badge the kernel delivers to the healer when this task crashes [...]
    pub crash_badge: usize,
    /// When this task is a server currently handling a `call`, the scheduler
    /// slot of the caller it must `reply` to (set on receiving a Call, cleared
    /// by `reply`). `None` otherwise. (Phase: call/reply IPC.)
    pub caller: Option<usize>,
```

(Keep the existing `crash_badge` doc comment text; only the field list grows.)

- [ ] **Step 2: Add `caller: None` to the three construction sites in `sched.rs`**

In `arch/riscv64/src/sched.rs`, in `spawn`'s `Task { .. }` literal:

```rust
            relaunch: None,
            restarts: 0,
            crash_badge: 0,
            caller: None,
        });
```

In `spawn_user`'s `Task { .. }` literal:

```rust
            relaunch: Some(Relaunch { entry: entry as *const () as usize, user_sp }),
            restarts: 0,
            crash_badge: 0,
            caller: None,
        });
```

In the sched `tests` module `task()` helper's `Task { .. }` literal:

```rust
            relaunch: None,
            restarts: 0,
            crash_badge: 0,
            caller: None,
        }
```

- [ ] **Step 3: Verify host tests + bare build**

Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS (existing suite; the new variant/field are not yet used).
Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 4: Commit**

```bash
git add arch/riscv64/src/task.rs arch/riscv64/src/sched.rs
git commit -m "feat(task): call/reply data model - Call role, AwaitingReply state, Task.caller"
```

---

## Task 2: the call/reply rendezvous in `sched.rs`

**Files:**
- Modify: `arch/riscv64/src/sched.rs`

- [ ] **Step 1: Refactor `block_current` onto a `park_current(state)` helper**

In `arch/riscv64/src/sched.rs`, replace the `block_current` function with a generic `park_current` plus a thin `block_current`:

```rust
/// Park the current task in `state` and switch to the next runnable task.
/// Returns only when something sets this task `Ready` again. Called from the
/// trap handler with interrupts off (like `exit_current`). The always-`Ready`
/// idle task guarantees a successor exists.
#[cfg(target_arch = "riscv64")]
fn park_current(state: TaskState) {
    let switch = SCHED.with(|s| {
        let current = s.current;
        s.tasks[current].as_mut().unwrap().state = state;
        let next = s.pick_next();
        assert_ne!(next, current, "park_current: no runnable successor (idle must be Ready)");
        s.tasks[next].as_mut().unwrap().state = TaskState::Running;
        s.current = next;
        let old = core::ptr::addr_of_mut!(s.tasks[current].as_mut().unwrap().context);
        let new = core::ptr::addr_of!(s.tasks[next].as_ref().unwrap().context);
        let next_satp = s.tasks[next].as_ref().unwrap().satp;
        (old, new, next_satp)
    });
    // SAFETY: distinct 'static contexts, the kernel is mapped in both address
    // spaces, single hart, interrupts off. Resumes here when woken.
    unsafe {
        crate::csr::satp_write(switch.2);
        switch_context(switch.0, switch.1);
    }
}

/// Block the current task at an IPC rendezvous (`Blocked{endpoint, role}`).
#[cfg(target_arch = "riscv64")]
fn block_current(endpoint: EndpointId, role: IpcRole) {
    park_current(TaskState::Blocked { endpoint, role });
}
```

- [ ] **Step 2: Bind a Call's caller in `ipc_recv`**

In `arch/riscv64/src/sched.rs`, in `ipc_recv`, replace the `match find_blocked(s, ep, IpcRole::Send) { .. }` block with one that also matches a waiting `Call` and binds it:

```rust
        // A waiting one-way Send, or a Call (which expects a reply).
        let waiting = find_blocked(s, ep, IpcRole::Send)
            .map(|si| (si, false))
            .or_else(|| find_blocked(s, ep, IpcRole::Call).map(|si| (si, true)));
        match waiting {
            Some((si, is_call)) => {
                let msg = s.tasks[si].as_ref().unwrap().message;
                if is_call {
                    // Bind us (the server) to this caller; it now awaits our reply.
                    s.tasks[cur].as_mut().unwrap().caller = Some(si);
                    s.tasks[si].as_mut().unwrap().state = TaskState::AwaitingReply;
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
```

- [ ] **Step 3: Add `ipc_call` and `ipc_reply` after `ipc_send`**

In `arch/riscv64/src/sched.rs`, add after `ipc_send` (before `restart`):

```rust
/// The result of inspecting the rendezvous for a `call`.
#[cfg(target_arch = "riscv64")]
enum CallStep {
    BadCap,
    AwaitReply,
    Queue(EndpointId),
}

/// Service a `call` syscall: atomically send the request (`a1`=badge,
/// `a2..a4`=data) to the endpoint named by `a0`, then block for the reply.
/// On return the reply `Message` is in the ABI registers (a0=badge,
/// a1..a3=data), or `a0 = usize::MAX` if the capability check failed.
#[cfg(target_arch = "riscv64")]
pub fn ipc_call(frame: &mut crate::trap::TrapFrame) {
    let cap_idx = frame.regs[9];
    let msg = Message {
        badge: frame.regs[10],
        data: [frame.regs[11], frame.regs[12], frame.regs[13]],
    };
    let step = SCHED.with(|s| {
        let cur = s.current;
        let ep = match cap_lookup(&s.tasks[cur].as_ref().unwrap().caps, cap_idx) {
            Some(ep) => ep,
            None => {
                crate::println!(
                    "ipc: '{}' call rejected (no capability)",
                    s.tasks[cur].as_ref().unwrap().name
                );
                return CallStep::BadCap;
            }
        };
        match find_blocked(s, ep, IpcRole::Recv) {
            Some(ri) => {
                crate::println!(
                    "ipc: '{}' calls '{}' badge {:#x}",
                    s.tasks[cur].as_ref().unwrap().name,
                    s.tasks[ri].as_ref().unwrap().name,
                    msg.badge
                );
                s.tasks[ri].as_mut().unwrap().message = msg;
                s.tasks[ri].as_mut().unwrap().caller = Some(cur);
                s.tasks[ri].as_mut().unwrap().state = TaskState::Ready;
                CallStep::AwaitReply
            }
            None => {
                // No server yet: queue our request; a server's recv will pick
                // it up, bind us as its caller, and move us to AwaitingReply.
                s.tasks[cur].as_mut().unwrap().message = msg;
                CallStep::Queue(ep)
            }
        }
    });
    match step {
        CallStep::BadCap => frame.regs[9] = usize::MAX,
        CallStep::AwaitReply => {
            park_current(TaskState::AwaitingReply);
            let reply = SCHED.with(|s| s.tasks[s.current].as_ref().unwrap().message);
            write_message(frame, reply);
        }
        CallStep::Queue(ep) => {
            block_current(ep, IpcRole::Call);
            let reply = SCHED.with(|s| s.tasks[s.current].as_ref().unwrap().message);
            write_message(frame, reply);
        }
    }
}

/// Service a `reply` syscall: answer the caller the kernel recorded when this
/// server received a Call. `a0`=badge, `a1..a3`=data. `a0` becomes `0` on
/// success, or `usize::MAX` if there is no pending caller (or it has exited).
#[cfg(target_arch = "riscv64")]
pub fn ipc_reply(frame: &mut crate::trap::TrapFrame) {
    let msg = Message {
        badge: frame.regs[9],
        data: [frame.regs[10], frame.regs[11], frame.regs[12]],
    };
    let ok = SCHED.with(|s| {
        let cur = s.current;
        let caller = match s.tasks[cur].as_mut().unwrap().caller.take() {
            Some(c) => c,
            None => return false,
        };
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

- [ ] **Step 4: Verify host tests + bare build**

Run: `cargo test -p kernel-arch-riscv64`
Expected: PASS (gated code excluded on host; existing suite green).
Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/sched.rs
git commit -m "feat(sched): call/reply rendezvous - ipc_call, ipc_reply, Call-caller binding, park_current"
```

---

## Task 3: the `call`/`reply` syscall numbers (decode + dispatch)

**Files:**
- Modify: `arch/riscv64/src/syscall.rs`

- [ ] **Step 1: Write the failing test**

In `arch/riscv64/src/syscall.rs`, add to the `tests` module:

```rust
    #[test]
    fn decodes_call_and_reply() {
        assert_eq!(decode_syscall(7), Syscall::Call);
        assert_eq!(decode_syscall(8), Syscall::Reply);
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 syscall::`
Expected: FAIL — `Syscall::Call`/`Reply` do not exist.

- [ ] **Step 3: Add the variants, decode, and dispatch**

In `arch/riscv64/src/syscall.rs`, add to `Syscall` (after `Restart`):

```rust
    /// `call(cap, badge, data..)` — send a request and block for the reply.
    Call,
    /// `reply(badge, data..)` — answer the caller the kernel recorded.
    Reply,
```

Add the decode arms (after `6 => Syscall::Restart,`):

```rust
        7 => Syscall::Call,
        8 => Syscall::Reply,
```

Add the dispatch arms (after the `Syscall::Restart` arm):

```rust
        Syscall::Call => {
            crate::sched::ipc_call(frame);
            Outcome::Resume
        }
        Syscall::Reply => {
            crate::sched::ipc_reply(frame);
            Outcome::Resume
        }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 syscall::`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/syscall.rs
git commit -m "feat(syscall): call (a7=7) + reply (a7=8) decode + dispatch"
```

---

## Task 4: update the smoke test for the call/reply RTC (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Swap the RTC-exits assertion for a client-exits one**

In `tools/test-qemu.ps1`, replace the line:

```powershell
    "sched: task 'rtc' exited \(code \d{15,}\)",
```

with:

```powershell
    "sched: task 'client' exited \(code \d{15,}\)",
```

Update the header comment near the RTC description to note the RTC is now a real server: the client `call`s it and receives the live clock back (the time crosses to the caller), instead of the server reporting via its exit code.

- [ ] **Step 2: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — the client still exits 0 (not the time); `sched: task 'client' exited (code <15+ digits>)` is absent. (The old `rtc exited (code …)` line is also gone, but the assertion now looks for `client`.)

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts the RTC client receives the time via call/reply (red)"
```

---

## Task 5: convert the RTC component to call/reply (green)

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Add the `call`/`reply` syscall wrappers**

In `kernel/src/main.rs`, add after `sys_send` (or near the other wrappers):

```rust
    /// call syscall (a7 = 7): a0 = cap, a1 = badge, a2..a4 = data (zero here).
    /// Sends the request and blocks for the reply; returns the reply badge in
    /// a0 (reply data words are discarded here), or `usize::MAX` on bad cap.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and blocks us until a
    /// server replies.
    #[inline(always)]
    unsafe fn sys_call(cap: usize, badge: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 7usize,
            inout("a0") cap => ret,
            inout("a1") badge => _,
            inout("a2") 0usize => _,
            inout("a3") 0usize => _,
            in("a4") 0usize,
            options(nostack),
        );
        ret
    }

    /// reply syscall (a7 = 8): a0 = badge, a1..a3 = data (zero here). Answers
    /// the caller the kernel recorded. Returns 0, or `usize::MAX` if there is
    /// no pending caller.
    ///
    /// # Safety
    /// Always sound; the kernel routes the reply to the recorded caller.
    #[inline(always)]
    unsafe fn sys_reply(badge: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 8usize,
            inout("a0") badge => ret,
            in("a1") 0usize,
            in("a2") 0usize,
            in("a3") 0usize,
            options(nostack),
        );
        ret
    }
```

- [ ] **Step 2: Make `rtc_server` a real call/reply server**

In `kernel/src/main.rs`, replace the `rtc_server` function. Update its doc comment to say it now loops serving and replies the time to the caller (rather than reporting via exit code), and the body:

```rust
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn rtc_server() -> ! {
        loop {
            // SAFETY: we hold the endpoint cap; recv blocks for a request, and
            // the kernel records the caller so our reply reaches it.
            let _req = unsafe { sys_recv(EP_CAP) };
            let low: u32;
            let high: u32;
            // SAFETY: the goldfish RTC page is mapped R-U in our address space;
            // reading TIME_LOW (offset 0) latches TIME_HIGH (offset 4).
            unsafe {
                core::arch::asm!(
                    "lw {lo}, 0({base})",
                    "lw {hi}, 4({base})",
                    base = in(reg) 0x10_1000usize,
                    lo = out(reg) low,
                    hi = out(reg) high,
                    options(nostack),
                );
            }
            let t = ((high as usize) << 32) | (low as usize);
            // SAFETY: reply the live clock to the caller; the client's `call`
            // returns it. Then loop to serve the next request.
            unsafe { sys_reply(t) };
        }
    }
```

- [ ] **Step 3: Make `rtc_client` use `call` and exit with the returned time**

In `kernel/src/main.rs`, replace the `rtc_client` function:

```rust
    /// A client of the RTC server: `call` it (badge 1 = "report time") and
    /// receive the live clock back, then exit with it — proving the value
    /// crossed back from the server to the caller via call/reply.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn rtc_client() -> ! {
        // SAFETY: we hold the endpoint cap; call sends the request and blocks
        // for the reply (the clock value).
        unsafe {
            let t = sys_call(EP_CAP, 1);
            sys_exit(t)
        }
    }
```

- [ ] **Step 4: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS.

- [ ] **Step 5: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS`, including `sched: task 'client' exited (code <15+ digit nanoseconds>)`, with `ipc: 'rtc' blocks on recv`, the rogue-refused line, the healing (5a/5b) and entropy milestones, and ticks all still present.

Troubleshooting (diagnose, don't weaken the test):
- Client exits 0 (no large code) → the reply isn't reaching it: confirm `ipc_call` sets `caller` on the server and `ipc_reply` reads `self.caller` and wakes it; confirm the RTC server `reply`s before looping back to `recv`.
- Hang after `ipc: 'rtc' blocks on recv` → the call/recv role match: confirm `ipc_recv` matches `IpcRole::Call` and `ipc_call` matches `IpcRole::Recv`.

- [ ] **Step 6: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat: RTC component over call/reply - the server returns the time to its caller"
```

---

## Task 6: docs — learning note, roadmap, glossary; final verification

**Files:**
- Create: `docs/learning/0017-call-reply-ipc.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md`

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0017-call-reply-ipc.md`:

```markdown
# 0017 — call/reply IPC: a server that answers

**One-line:** a client can now `call` a server and get a value back — built
from the one-way rendezvous, with the kernel binding each reply to its caller.

## What changed
- New syscalls: `call(ep, request)` (atomically send + block for the reply)
  and `reply(response)` (answer the recorded caller).
- New wait states: `IpcRole::Call` (a caller queued at an endpoint) and
  `TaskState::AwaitingReply` (a caller whose request was picked up, waiting for
  the answer). The server carries a `caller` back-pointer.
- The RTC component is now a real server: `loop { recv; read clock; reply(t) }`.
  The client `call`s it and exits with the returned time — the value crosses
  *back* to the caller, instead of the server reporting via its exit code.

## The ideas worth keeping
1. **`call` is "send + await reply" as one atomic step.** Splitting it into a
   separate send then recv would race — the reply could arrive before the recv.
   One syscall blocks the caller in `AwaitingReply` the moment it sends.
2. **Caller-tracking instead of reply capabilities.** Because we are
   single-hart and servers handle one call at a time, the kernel just records
   `caller` on the server when it receives a Call; `reply` wakes exactly that
   task. A server can only ever answer whoever just called it (secure), with no
   new capability type. seL4's one-shot reply caps buy generality (deferred /
   forwarded / out-of-order replies) we don't need yet.
3. **Two wait phases, two states.** A caller first queues at the endpoint
   (`Blocked{ep, Call}`); once a server picks it up it moves to
   `AwaitingReply` so no second server can re-match it. Only the server's
   `reply` makes it `Ready`.

## Proof
`ipc: 'rtc' blocks on recv` → the client `call`s → `sched: task 'client'
exited (code <nanoseconds>)`: the live clock value returned from the server to
its caller. The server keeps looping, ready for the next request.
```

- [ ] **Step 2: Update the roadmap**

In `docs/roadmap/roadmap.md`, the "Second component — virtio-rng" block ends with a "(Next candidates: …)" note listing call/reply first. Replace that parenthetical with:

```markdown
### call/reply IPC  *(done — 2026-06-21)*

- **Goal:** request/response IPC so a server returns a value to the task that
  called it, instead of via an exit code or a one-way send.
- **You learn:** `call` is an atomic send + await-reply; the kernel binds each
  reply to its caller with a back-pointer (no reply capability needed for a
  single-hart, one-call-at-a-time server) (see
  [learning note 0017](../learning/0017-call-reply-ipc.md)).
- **Done when:** `./tools/test-qemu.ps1` shows the RTC client `call` the server
  and exit with the returned live-clock value. QEMU-only.

(Next candidates: a kernel entropy pool/CSPRNG seeded by virtio-rng; an
interrupt-driven (PLIC) device path; one-shot reply capabilities for
deferred/forwarded replies.)
```

- [ ] **Step 3: Add a glossary entry**

In `docs/glossary.md`, add after the existing IPC entry (only the genuinely new term):

```markdown
- **call/reply (RPC)** — request/response IPC: a client `call`s a server (sending a request and blocking for the answer) and the server `reply`s. The kernel records which task called so the reply returns to exactly that caller. Built on the one-way send/recv rendezvous.
```

- [ ] **Step 4: Update the learning-notes index**

In `docs/learning/README.md`, add after the 0016 line:

```markdown
- [0017 — call/reply IPC: a server that answers](0017-call-reply-ipc.md)
```

- [ ] **Step 5: Final verification (run all, paste results)**

Run: `cargo test -p kernel-arch-riscv64` → expect all green.
Run: `cargo build --workspace` → expect success.
Run (PowerShell): `./tools/check-references.ps1` → expect PASS.
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0017-call-reply-ipc.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: call/reply IPC - learning note, roadmap, glossary"
```

---

## Done-when checklist (maps to spec §1)

- [ ] **A reply crosses back to the caller** — smoke shows `sched: task 'client' exited (code <15+ digits>)` (the returned time), the RTC server still looping, and the rogue refused.
- [ ] **Host tests** — `decode_syscall(7)`/`(8)` → `Call`/`Reply`.
- [ ] `check-references` clean; `cargo build --workspace` green; `BOOT TEST PASS`.
```
