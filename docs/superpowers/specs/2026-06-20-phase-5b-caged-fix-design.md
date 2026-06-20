# Kernel — Phase 5b Design: Self-healing — the caged fix

- **Date:** 2026-06-20
- **Status:** Draft — awaiting review
- **Scope of this document:** Phase 5b only — the self-healer that *acts*.
  An isolated, capability-gated **user-space** healer is notified by the
  kernel when a component is contained after a crash, and applies the
  KB-0005 playbook: a **bounded, reversible, logged** restart that recovers
  the component. The deterministic diagnosis (5a) stays exactly as is; 5b
  adds the action — inside a safety cage. Fully QEMU-testable.

---

## 0. Where 5b sits

Phase 5 (the self-healing knowledge organism, the project's #2 priority —
[ADR 0005](../decisions/0005-self-healing-knowledge-organism.md)) was
decomposed (2026-06-20) in the trust-preserving order:

- **5a (done) — detect + diagnose.** When the microkernel contains a crashed
  component (`sched::exit_current(Killed{cause})`), a deterministic,
  host-tested rule engine (`heal::diagnose`) matches the crash to a known
  issue (KB-0005) and logs the diagnosis + playbook. No action.
- **5b (this doc) — the caged fix.** The part that *acts*. It moves into an
  isolated **user-space** healer component, because an agent with the power
  to change the system is exactly what must be confined.

5b builds on everything already proven: components (the RTC driver, ADR
0007), per-address-space isolation (3b-ii), capability-checked synchronous
IPC + blocking (3b-iii), and the containment + diagnosis path (5a).

## 1. Goal

When a U-mode component is contained after a fatal fault and diagnosed
(KB-0005), the kernel **notifies an isolated user-space healer**, which
**applies the playbook**: it asks the kernel to **restart** the component.
The restart is **caged** — capability-gated, bounded, reversible, and logged:

- **The acting agent lives in user space.** The healer decides to act and
  which capability to invoke; it is itself an unprivileged, isolated task.
- **The kernel is the cage.** It capability-checks the restart, enforces the
  retry **bound** (even a buggy/compromised healer cannot restart-loop), does
  the re-forge, and logs every action.
- **Recovery is real.** A transient crash is followed by a successful run —
  the component serves again.
- **The bound holds.** A component that keeps crashing is restarted only up
  to a fixed limit, then abandoned and flagged for triage.

**You learn (kept brief):** how the acting half of self-healing is made safe
by *isolation + a capability cage* — the agent runs unprivileged in user
space and can only do what a capability grants, while the kernel enforces the
bound and logs; and how a "restart" is just re-forging a task's first-run
context (the address space, stacks, and data persist), with the launch
generation handed to the task so a transient fault can be distinguished from
a permanent one.

**Done when** `./tools/test-qemu.ps1` observes, alongside every existing
milestone (2a/2b, 3c PQC, 4a/4b, the RTC component, ticks):

1. **Recovery** — a `transient` component crashes once
   (`sched: task 'transient' killed by LoadPageFault`), the kernel diagnoses
   it (5a) and notifies the healer, the healer restarts it
   (`heal: restarted 'transient' (attempt 1)`), and on the restart it
   succeeds and exits cleanly (`sched: task 'transient' exited (code 0)`).
2. **The bound** — a `flaky` component crashes every time; the healer
   restarts it up to the limit, then the kernel refuses and flags it
   (`heal: giving up on 'flaky' after 2 restarts (flagged for triage)`).

And off the bare target:

3. **Host unit tests** — the restart-capability lookup, the bound predicate,
   and the launch-generation forge.

## 2. Non-goals (deferred)

- **Restarting kernel tasks** — only U-mode components (those with relaunch
  info) are restartable. Kernel tasks have `relaunch = None`.
- **Reaping / re-allocating on restart** — restart reuses the crashed task's
  existing slot, address space (`satp`), stacks, and data page. No frames are
  freed or allocated. (Reaping remains deferred, as since 3b-i.)
- **Reading `knowledge-base/*.md` at runtime / a general playbook
  interpreter** — the healer's policy is fixed ("on a crash notification,
  invoke the restart capability"); the kernel carries the compiled KB-0005
  subset from 5a. A real loader/interpreter awaits a filesystem.
- **Multiple healers / healer supervision** — one healer; if it is not
  blocked on the crash endpoint when a crash occurs, the notification is
  dropped and the component is left down (logged). Healing the healer is out
  of scope.
- **Capability delegation, revocation, rights** — still one new static cap
  type granted at boot; no transfer through IPC.
- **An AI advisor** — deferred indefinitely (ADR 0005); never in the kernel.

## 3. Design

### 3.1 Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| `Capability::Restart` + `restart_target` | `arch/riscv64/src/cap.rs` | New cap variant `Restart(TaskHandle)` (a scheduler slot); pure `restart_target(caps, idx) -> Option<usize>` lookup (host-tested), mirroring `cap_lookup`. |
| Launch generation in the forge | `arch/riscv64/src/task.rs` | `forge_user_context` gains a `generation` param stashed in `s3`; `Task` gains `relaunch: Option<Relaunch{entry, user_sp}>`, `restarts: usize`, `crash_badge: usize`. Pure `can_restart(restarts, bound) -> bool`. |
| `mv a0, s3` in the U-mode launchpad | `arch/riscv64/src/sched.rs` (asm) | `user_trampoline` passes the generation to the task in `a0` before `sret`. |
| `restart` syscall + crash notification | `arch/riscv64/src/sched.rs`, `syscall.rs`, `trap.rs` | `restart(cap_idx)` (`a7=6`): cap-check → bound-check → re-forge → `Ready` → log. `exit_current` (Killed) notifies a recv-blocked healer on the reserved crash endpoint. |
| The healer | `kernel/src/main.rs` | A U-mode component: `loop { let c = recv(CRASH_CAP); restart(c); }`. The acting agent — minimal, register-only. |
| The patients | `kernel/src/main.rs` | `transient` (crash if `a0==0`, else serve+exit 0) proves recovery; `flaky` (always crash) proves the bound. |

### 3.2 Restart = re-forge the first-run context

A restart does **not** allocate or reap. The crashed task keeps its slot,
`satp`, kernel/user stacks, and data page; the kernel simply rebuilds its
first-run `Context` with the existing `task::forge_user_context` and flips the
state back to `Ready`. To rebuild it the kernel needs the original `entry` and
`user_sp`, which are not stored today, so `Task` gains:

```
relaunch: Option<Relaunch { entry: usize, user_sp: usize }>,  // None = not restartable
restarts: usize,                                              // bound counter
crash_badge: usize,                                           // see 3.4
```

`spawn_user` records `relaunch = Some(Relaunch{entry, user_sp})`, `restarts =
0`. `spawn` (kernel tasks) leaves `relaunch = None`. Re-forge reuses
`Task.stack_top` (kstack), `Task.satp`, and `user_sstatus(sstatus_read())`,
exactly as `spawn_user` did.

### 3.3 Launch generation → distinguishing transient from permanent

A deterministic patient, re-forged identically, would crash identically — so
recovery needs the run to differ. Rather than a persistent writable page
(awkward under the U-mode codegen rules), the kernel **passes the restart
generation to the task on every launch**:

- `forge_user_context(tramp, entry, user_sp, kstack_top, sstatus,
  generation)` stashes `generation` in `s3`.
- `user_trampoline` adds `mv a0, s3` immediately before `sret`, so the task
  starts with `a0 = generation` (0 on first run, N after N restarts).

This is a clean, general capability ("a task learns its launch generation");
tasks that ignore `a0` are unaffected. The `transient` patient reads `a0`:
`a0 == 0` → crash; `a0 > 0` → do its work and `exit(0)`. All register-only —
no `.rodata`, no extra mapping (respects the RTC-phase codegen lesson:
U-mode must not call kernel `.text` or read `.rodata`).

### 3.4 Crash notification — reuse the IPC rendezvous

A reserved endpoint id `CRASH_EP` carries crash notifications. The healer
holds an `Endpoint(CRASH_EP)` capability and `recv`-blocks on it. When
`exit_current` contains and diagnoses a **restartable** crash (the dead task
has `relaunch = Some(..)`), it does what `ipc_send` does to a waiting
receiver — find the task `Blocked{CRASH_EP, Recv}`, set its `message` and mark
it `Ready` — delivering `Message { badge = dead.crash_badge }`. The dead task
stays `Exited` until the healer restarts it.

`crash_badge` is **the healer's own cap-table index of that patient's Restart
capability**, set by `kmain` when it grants the caps. So the badge the healer
receives *is* the cap index it passes to `restart` — the healer needs no
slot→cap mapping table (which would need `.rodata`). The healer is therefore:

```
loop {
    let cap_idx = recv(CRASH_CAP);  // a0 = badge = which Restart cap to use
    restart(cap_idx);               // kernel checks cap + bound, re-forges, logs
}
```

If no healer is blocked on `CRASH_EP` at crash time, the notification is
dropped and the kernel logs `heal: no healer for '<name>' (left down)`.

### 3.5 The `restart` syscall and the cage

New syscall `restart(cap_idx)` (`a7 = 6`); ABI: `a0 = cap_idx`, returns `a0 =
0` on success, `usize::MAX` on refusal (bad cap or bound exceeded). Kernel
logic (in `sched`, under `SCHED.with`, like `ipc_*`):

1. `restart_target(caller.caps, cap_idx)` → target slot, or refuse (bad cap).
2. If `!can_restart(target.restarts, MAX_RESTARTS)` → log `heal: giving up on
   '<name>' after <restarts> restarts (flagged for triage)`, return refusal.
   The target stays `Exited` — the bound is what makes the action reversible:
   it stops.
3. Else: re-forge the target's first-run context with `generation =
   restarts + 1`, set `restarts += 1`, set state `Ready`, log `heal: restarted
   '<name>' (attempt <restarts>)`, return success.

`MAX_RESTARTS = 2` (a constant). The bound lives in the **kernel** so even a
faulty healer cannot loop. Every restart is logged (auditable), capability-
checked (confined), and bounded (reversible — it terminates).

### 3.6 Scheduling and the demo cast

`MAX_TASKS` bumps 6 → 8 to host the healer + two patients + idle alongside
the kept RTC component (`rtc`/`client`) with headroom. Spawn order puts the
RTC server (slot 0) and the healer (slot 2) before the patients so each
`recv`-blocks before any patient runs and crashes:

`rtc(0)` · `client(1)` · `healer(2)` · `transient(3)` · `flaky(4)` ·
`idle(5)`. `kmain` grants: the RTC endpoint caps as today; the healer an
`Endpoint(CRASH_EP)` cap at cap slot 0 and `Restart(transient_slot)` /
`Restart(flaky_slot)` at cap slots 1 / 2; and sets `transient.crash_badge =
1`, `flaky.crash_badge = 2` so each crash notification names the right cap.

Sequence the smoke test proves: RTC demo runs (unchanged); `transient` crashes
once → diagnosed → healer restart attempt 1 → `transient` re-runs with `a0=1`
→ exits 0 (recovered); `flaky` crashes → restart 1 → crash → restart 2 → crash
→ kernel refuses (bound) → flagged. The kernel and other components keep
running throughout; ticks continue.

### 3.7 Error handling summary

| Situation | Behavior |
|-----------|----------|
| Restartable component crashes, healer waiting | Contained + diagnosed (5a); healer notified; restarted (within bound). |
| Component keeps crashing | Restarted up to `MAX_RESTARTS`, then refused + flagged; left `Exited`. |
| Crash with no healer blocked on `CRASH_EP` | Notification dropped; logged `heal: no healer for '<name>' (left down)`; component stays down. |
| `restart` with a bad/wrong-type cap index | Refused (`a0 = usize::MAX`); nothing re-forged. |
| Kernel task or non-restartable crash | `relaunch = None`; no notification; behaves as today. |
| Clean `exit(code)` | No diagnosis, no notification (only `Killed` is a crash); unchanged. |

## 4. Testing

- **Host unit tests** (`arch/riscv64`, `cargo test`):
  - `cap::restart_target` — returns the target slot for a `Restart` cap at the
    index; `None` for an empty slot, an out-of-range index, or an `Endpoint`
    cap (wrong type).
  - `task::can_restart` — `true` below the bound, `false` at/over it.
  - `task::forge_user_context` — carries `generation` in `s3` (extend the
    existing forge test); other slots unaffected.
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): add `heal: restarted 'transient' (attempt 1)`,
  `sched: task 'transient' exited (code 0)`, and `heal: giving up on 'flaky'
  after 2 restarts (flagged for triage)`; keep every existing milestone.

## 5. Deliverables

1. `cap.rs`: `Capability::Restart(usize)`, `restart_target`, host tests.
2. `task.rs`: `Relaunch`, `Task.{relaunch, restarts, crash_badge}`,
   `forge_user_context` generation param, `can_restart`, host tests.
3. `sched.rs`: `user_trampoline` `mv a0, s3`; `spawn`/`spawn_user` set the new
   fields; the `restart` machinery; the crash notification in `exit_current`;
   `CRASH_EP` + `MAX_RESTARTS` constants.
4. `syscall.rs`: `Syscall::Restart` (`a7=6`) decode + dispatch → `sched`.
5. `kernel/src/main.rs`: the `healer`, `transient`, and `flaky` components,
   their stacks, the grants/badges, and `MAX_TASKS` 6 → 8.
6. Extended QEMU smoke test + host unit tests, all green.
7. Short learning note `docs/learning/0015-self-healing-the-caged-fix.md`.
8. Roadmap: Phase 5b marked done with date; **Phase 5 complete**.
9. Glossary: only genuinely new terms (e.g. *launch generation*); reuse the
   existing self-healing / safety-cage / playbook entries.

## 6. Open questions (for later phases)

- **Recording new issues at runtime** and **loading the real
  `knowledge-base/` store** — both need a filesystem.
- **Supervising the healer** (healing the healer; multiple healers).
- **Richer playbooks** — choosing among actions, escalation, crash-loop
  detection feeding the bound; the healer consulting the KB itself.
- **Reaping** restarted/abandoned components to reclaim slots and frames.
- **Capability delegation/revocation** so authority can move at runtime.
- **The AI advisor (far future):** an isolated model that *suggests*
  diagnoses/playbooks for the deterministic engine or a human to approve —
  never acting on its own, never in the kernel.
