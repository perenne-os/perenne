//! Scheduling: a fixed-size round-robin run queue plus the context
//! switch that makes it real.
//!
//! Pure here: the `Scheduler` struct and `pick_next` (host-tested). The
//! gated section below adds the static `SCHED` instance, the
//! `switch_context` assembly, `spawn`, `yield_now`, `enter`, and
//! `preempt` — everything that touches CSRs, assembly, or the live
//! console.

use crate::task::{Task, TaskState};
// `Context` is named only by the gated switch code and the host tests
// (the pure run queue holds `Task`s, not `Context`s), so gate the import
// to those two configs to keep the host lib build warning-free.
#[cfg(any(target_arch = "riscv64", test))]
use crate::task::Context;
// The test helper and the gated IPC code both name these.
#[cfg(any(target_arch = "riscv64", test))]
use crate::task::{Message, IpcRole, CAP_SLOTS};
// The IPC rendezvous looks up endpoint capabilities.
#[cfg(target_arch = "riscv64")]
use crate::cap::{cap_lookup, EndpointId};
// `spawn_user` records relaunch info so a crashed component can be re-forged.
#[cfg(target_arch = "riscv64")]
use crate::task::Relaunch;

/// Maximum concurrent tasks: the demo runs thirteen (the above plus deferrer,
/// dclientA, dclientB) plus headroom.
pub const MAX_TASKS: usize = 25;

/// The reserved endpoint id the kernel uses to notify the self-healer of a
/// contained crash. The healer holds an `Endpoint(CRASH_EP)` capability and
/// `recv`-blocks on it. Distinct from the demo endpoint `EP0` (= 0).
#[cfg(target_arch = "riscv64")]
pub const CRASH_EP: crate::cap::EndpointId = 1;

/// How many times the self-healer may restart a single component before the
/// kernel gives up and flags it for triage (the safety-cage bound).
#[cfg(target_arch = "riscv64")]
pub const MAX_RESTARTS: usize = 2;

/// The run queue: a fixed array of optional tasks and the index of the
/// one currently running.
pub struct Scheduler {
    tasks: [Option<Task>; MAX_TASKS],
    current: usize,
}

impl Scheduler {
    /// An empty scheduler; `spawn` fills slots and `enter` starts it.
    pub const fn new() -> Self {
        Self { tasks: [None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None, None], current: 0 }
    }

    /// Index of the next task to run after `current`, round-robin: scan
    /// forward (wrapping) for the next `Ready` slot, skipping empty
    /// slots and the current task. Returns `current` if nobody else is
    /// ready — the caller then keeps running.
    pub fn pick_next(&self) -> usize {
        // 1..MAX_TASKS covers every slot except `current` (which is
        // Running, never Ready); the fallback below returns `current`.
        for offset in 1..MAX_TASKS {
            let i = (self.current + offset) % MAX_TASKS;
            if let Some(t) = &self.tasks[i] {
                if t.state == TaskState::Ready {
                    return i;
                }
            }
        }
        self.current
    }
}

// The actual switch. Saves the callee-saved set of the running task into
// `*old` (a0), loads it from `*new` (a1), and `ret`s — which resumes
// wherever `new.ra` points. Offsets match `task::Context`:
// ra@0, sp@8, s0..s11 @ 16..104. Caller-saved registers are deliberately
// NOT saved: the Rust calling convention already treats them as clobbered
// across this call.
#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text
    .global switch_context
switch_context:
    sd ra,  0(a0)
    sd sp,  8(a0)
    sd s0,  16(a0)
    sd s1,  24(a0)
    sd s2,  32(a0)
    sd s3,  40(a0)
    sd s4,  48(a0)
    sd s5,  56(a0)
    sd s6,  64(a0)
    sd s7,  72(a0)
    sd s8,  80(a0)
    sd s9,  88(a0)
    sd s10, 96(a0)
    sd s11, 104(a0)
    ld ra,  0(a1)
    ld sp,  8(a1)
    ld s0,  16(a1)
    ld s1,  24(a1)
    ld s2,  32(a1)
    ld s3,  40(a1)
    ld s4,  48(a1)
    ld s5,  56(a1)
    ld s6,  64(a1)
    ld s7,  72(a1)
    ld s8,  80(a1)
    ld s9,  88(a1)
    ld s10, 96(a1)
    ld s11, 104(a1)
    ret

    # First entry of a never-run task. switch_context loaded sp = stack
    # top, s0 = the task's entry function, ra = here. We enable interrupts
    # (so this task is preemptible — a freshly entered task has no prior
    # yield_now to restore SIE for it) and jump into the entry. If the
    # entry ever returns (it is typed `-> !`, so it should not), fall
    # through to a loud panic.
    .global task_trampoline
task_trampoline:
    csrsi sstatus, 0x2
    jalr s0
    call task_has_returned
"#
);

#[cfg(target_arch = "riscv64")]
extern "C" {
    fn switch_context(old: *mut Context, new: *const Context);
    fn task_trampoline();
}

/// Reached only if a *kernel* task's entry function returns. Kernel tasks
/// (entered via `task_trampoline`) must loop forever — there is no exit path
/// for them. (U-mode tasks exit cleanly via the `exit` syscall →
/// `exit_current`; this trampoline is not on their path.)
#[cfg(target_arch = "riscv64")]
#[no_mangle]
extern "C" fn task_has_returned() -> ! {
    panic!("kernel task entry returned (kernel tasks must loop forever)");
}

/// The kernel's one scheduler. Lives in a `SingleHartCell` (the same
/// re-entry-guarded primitive `mem` introduced): one hart, and the
/// guard fires if a future change lets the scheduler re-enter itself.
#[cfg(target_arch = "riscv64")]
static SCHED: crate::mem::SingleHartCell<Scheduler> =
    crate::mem::SingleHartCell::new(Scheduler::new());

/// Register a task: forge a `Context` whose first switch lands in
/// `task_trampoline` with `sp` = top of `stack` and `s0` = `entry`.
/// Panics if there is no free slot — a static configuration bug.
///
/// `stack_top` is the (exclusive) top address of a static stack array;
/// it is rounded down to a 16-byte boundary per the RISC-V ABI.
#[cfg(target_arch = "riscv64")]
pub fn spawn(name: &'static str, entry: extern "C" fn() -> !, stack_top: usize) -> usize {
    SCHED.with(|s| {
        let slot = s
            .tasks
            .iter()
            .position(Option::is_none)
            .expect("scheduler full");
        let mut context = Context::zeroed();
        context.ra = task_trampoline as *const () as usize;
        context.sp = stack_top & !0xF;
        context.s[0] = entry as usize;
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
            caller: None,
            pending_grant: None,
        });
        slot
    })
}

/// Install `cap` at `cap_slot` in the capability table of the task in
/// scheduler `slot`. Called at boot to hand a task its initial authority.
#[cfg(target_arch = "riscv64")]
pub fn grant_cap(slot: usize, cap_slot: usize, cap: crate::cap::Capability) {
    SCHED.with(|s| {
        s.tasks[slot]
            .as_mut()
            .expect("grant_cap: empty task slot")
            .caps[cap_slot] = Some(cap);
    });
}

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

/// Set the U-mode launch-register arguments of the task in scheduler `slot`:
/// `a1`/`a2` at first run (stored in the forged context's `s4`/`s5`; the
/// trampoline moves them into `a1`/`a2`). Used to hand a driver its device
/// addresses. (Not re-applied on a 5b restart — restartable launch args are
/// future work; the entropy component is not a restart patient.)
#[cfg(target_arch = "riscv64")]
pub fn set_launch_args(slot: usize, a1: usize, a2: usize) {
    SCHED.with(|s| {
        let t = s.tasks[slot].as_mut().expect("set_launch_args: empty task slot");
        t.context.s[4] = a1;
        t.context.s[5] = a2;
    });
}

/// Voluntarily give up the CPU to the next ready task. Returns when this
/// task is scheduled again. A no-op if nobody else is ready.
///
/// The pick-and-switch runs with interrupts disabled so a timer tick
/// cannot re-enter `SCHED` mid-switch. On resume we restore the SIE the
/// caller had on entry: a task that yielded with interrupts on resumes
/// with them on, while a task preempted from inside the trap handler
/// (SIE already off) resumes with them off and lets the trap return
/// re-enable them atomically via `sret`.
#[cfg(target_arch = "riscv64")]
pub fn yield_now() {
    // SAFETY: disabling interrupts is always safe; we restore the prior
    // state below (or, for a switch, on resume).
    let had_interrupts = unsafe { crate::csr::sstatus_disable_interrupts() };

    let switch = SCHED.with(|s| {
        let current = s.current;
        let next = s.pick_next();
        if next == current {
            return None;
        }
        s.tasks[current].as_mut().unwrap().state = TaskState::Ready;
        s.tasks[next].as_mut().unwrap().state = TaskState::Running;
        s.current = next;
        let old = core::ptr::addr_of_mut!(s.tasks[current].as_mut().unwrap().context);
        let new = core::ptr::addr_of!(s.tasks[next].as_ref().unwrap().context);
        let next_satp = s.tasks[next].as_ref().unwrap().satp;
        Some((old, new, next_satp))
    });

    if let Some((old, new, next_satp)) = switch {
        // SAFETY: both pointers address `Context`s inside the 'static
        // SCHED, valid for the whole program. `next != current` above
        // guarantees `old` and `new` are distinct slots, so they never
        // alias. Single hart + the disabled interrupts above mean no
        // other code touches them while the assembly runs. The next task's
        // tree maps every kernel address (map_kernel_sections), so swapping
        // satp before the switch is seamless. Execution resumes here when
        // this task is picked again.
        unsafe {
            // Re-arm the timer for the task we are switching to, so a tick that
            // came due while interrupts were masked for this switch is not
            // delivered at its first instruction (which would preempt it before
            // it runs — a livelock for a repeatedly-scheduled task).
            crate::timer::rearm();
            crate::csr::satp_write(next_satp);
            switch_context(old, new);
        }
    }

    if had_interrupts {
        // SAFETY: re-enabling is the inverse of the disable above; a trap
        // handler is installed and the timer is armed by the time any
        // task runs.
        unsafe { crate::csr::sstatus_enable_interrupts() };
    }
}

/// Start scheduling: switch into the first spawned task. Never returns —
/// the bootstrap (`kmain`) context is abandoned, which is correct because
/// a kernel never returns from `kmain`. The first switch needs an `old`
/// `Context` to save into; we use a throwaway on this stack that is never
/// restored (xv6's scheduler-context trick in miniature).
#[cfg(target_arch = "riscv64")]
pub fn enter() -> ! {
    // SAFETY: returned state ignored — `enter` never returns, and the
    // first task re-enables interrupts in `task_trampoline`.
    let _ = unsafe { crate::csr::sstatus_disable_interrupts() };

    let (first, first_satp) = SCHED.with(|s| {
        s.current = 0;
        let t = s.tasks[0].as_mut().expect("enter() with no spawned task");
        t.state = TaskState::Running;
        (core::ptr::addr_of!(t.context), t.satp)
    });

    let mut throwaway = Context::zeroed();
    // SAFETY: switch into the first task's address space (the kernel is
    // mapped in it), then into its context. `first` addresses a Context in
    // the 'static SCHED; the throwaway is a valid (never-restored) save
    // target on this stack.
    unsafe {
        crate::csr::satp_write(first_satp);
        switch_context(core::ptr::addr_of_mut!(throwaway), first);
    }
    unreachable!("enter() never returns to the bootstrap context");
}

/// Preemption entry, called from the timer trap handler. The interrupted
/// task's full 31-GPR state is already saved in its `TrapFrame` on its
/// stack; this just parks the handler's continuation and runs the next
/// task via the same primitive as a voluntary yield.
#[cfg(target_arch = "riscv64")]
pub fn preempt() {
    yield_now();
}

// First entry into U-mode for a freshly spawned U-mode task. Reached via
// `switch_context` "returning" into it with the context `spawn_user`
// forged: s0 = user entry (-> sepc), s1 = user sp, s2 = user sstatus,
// sp = this task's kernel/trap-stack top (we run on it now). We arm
// sscratch with that top (so the user's first trap swaps onto it), load the
// user CSRs, switch to the user stack, and `sret`. The U-mode analogue of
// `task_trampoline`.
#[cfg(target_arch = "riscv64")]
core::arch::global_asm!(
    r#"
    .section .text
    .global user_trampoline
user_trampoline:
    csrw sscratch, sp       # sscratch = trap-stack top (for the U->S trap)
    csrw sepc, s0           # user entry point
    csrw sstatus, s2        # SPP = 0, SPIE = 1
    mv sp, s1               # switch to the user stack
    mv a0, s3               # a0 = launch generation (forge put it in s3)
    mv a1, s4               # a1 = launch arg 1 (set_launch_args put it in s4)
    mv a2, s5               # a2 = launch arg 2 (set_launch_args put it in s5)
    sret                    # -> U-mode at the entry point
"#
);

#[cfg(target_arch = "riscv64")]
extern "C" {
    fn user_trampoline();
}

#[cfg(target_arch = "riscv64")]
use crate::task::ExitReason;

/// Register a U-mode task: forge a `Ready` slot whose first `switch_context`
/// lands in `user_trampoline` and `sret`s to U-mode. `entry` must live in a
/// `U`-mapped executable page; `user_sp` is the top of a `U`-mapped stack;
/// `kstack_top` is the top of a trusted kernel stack the task's traps land
/// on. Panics if there is no free slot (a static configuration bug).
#[cfg(target_arch = "riscv64")]
pub fn spawn_user(
    name: &'static str,
    entry: extern "C" fn() -> !,
    user_sp: usize,
    kstack_top: usize,
    satp: usize,
) -> usize {
    SCHED.with(|s| {
        let slot = s
            .tasks
            .iter()
            .position(Option::is_none)
            .expect("scheduler full");
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
            caller: None,
            pending_grant: None,
        });
        slot
    })
}

/// Terminate the running task and switch to the next ready one. Called from
/// the trap handler for the `exit` syscall (`Exited`) and fatal U-mode
/// faults (`Killed`). Prints the outcome, marks the current slot `Exited`,
/// and `switch_context`s away — never resuming the dead slot, so its trap
/// frame and stacks are simply abandoned (reaping is deferred). The idle
/// task is always `Ready`, so a successor always exists.
#[cfg(target_arch = "riscv64")]
pub fn exit_current(reason: ExitReason) -> ! {
    // SAFETY: runs inside the trap handler with interrupts off; single hart.
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
                // Phase 12: set to the issue id when the diagnosed issue is
                // escalated (chronic) — such a crash is quarantined, not restarted.
                let mut quarantine_id: Option<&'static str> = None;
                // Phase 5a: consult the deterministic knowledge organism and
                // log the diagnosis.
                match crate::heal::diagnose(cause) {
                    Some(issue) => {
                        crate::println!(
                            "heal: diagnosed {} ({}) -> playbook: {}",
                            issue.id(), issue.title(), issue.playbook()
                        );
                        // Phase 11: if this diagnosis just crossed the escalation
                        // threshold, log the one-time event.
                        if let Some(seen) = crate::heal::note_diagnosis(issue) {
                            crate::println!(
                                "heal: {} escalated (seen {seen}) -- recurring; flag for triage",
                                issue.id()
                            );
                        }
                        // Phase 12: a crash whose issue is now escalated is
                        // chronic — we will quarantine the component, not restart
                        // it. `escalated()` reflects the state `note_diagnosis`
                        // just updated; the id is a `&'static` into the table.
                        if issue.escalated() {
                            quarantine_id = Some(issue.id());
                        }
                    }
                    None => {
                        crate::println!("heal: no known issue for {cause:?} (recording for write-back)");
                        // Phase 7: latch the cause for the KB-writer to record
                        // to disk, so a later boot recognizes it.
                        crate::heal::note_unmatched(cause);
                    }
                }
                // Phase 5b: if this is a restartable component, notify a
                // user-space healer waiting on the crash endpoint so it can
                // apply the playbook (a caged restart). Reuses the IPC
                // rendezvous: deliver to a recv-blocked healer and wake it,
                // then run it next.
                if s.tasks[current].as_ref().unwrap().relaunch.is_some() {
                    let name = s.tasks[current].as_ref().unwrap().name;
                    if let Some(id) = quarantine_id {
                        // Phase 12: the issue is chronic — stop the futile fix.
                        crate::println!("heal: '{name}' quarantined ({id} chronic) -- not restarting");
                    } else {
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
    // SAFETY: the `assert_ne!` above guarantees `old` and `new` are distinct
    // slots, so they never alias; both are 'static contexts in SCHED. The
    // dying (old) slot is never run again, so its saved state is irrelevant.
    // Single hart + interrupts off in the trap handler mean nothing else
    // touches them. The next task's tree maps every kernel address, so the
    // satp switch is seamless. Control resumes in `next`.
    unsafe {
        crate::csr::satp_write(switch.2);
        switch_context(switch.0, switch.1);
    }
    unreachable!("exit_current resumes in the next task, never here")
}

// ---- Synchronous IPC rendezvous (Phase 3b-iii) ----
//
// An endpoint is a symbolic id; its wait queue is the set of tasks Blocked
// on it (found by scanning the task array). send/recv either deliver-and-
// wake a waiting peer or block the caller until one arrives. The message
// rides registers only — no memory is touched.

/// The result of inspecting the rendezvous for a `recv`.
#[cfg(target_arch = "riscv64")]
enum RecvStep {
    BadCap,
    Got(Message),
    Block(EndpointId),
}

/// The result of inspecting the rendezvous for a `send`.
#[cfg(target_arch = "riscv64")]
enum SendStep {
    BadCap,
    Delivered,
    Block(EndpointId),
}

/// Write a received [`Message`] into the ABI return registers of `frame`:
/// a0 = badge, a1..a3 = data (`regs[9]` = a0, `regs[10]` = a1, …).
#[cfg(target_arch = "riscv64")]
fn write_message(frame: &mut crate::trap::TrapFrame, msg: Message) {
    frame.regs[9] = msg.badge;
    frame.regs[10] = msg.data[0];
    frame.regs[11] = msg.data[1];
    frame.regs[12] = msg.data[2];
}

/// First task (in slot order) blocked on `ep` as `role`, if any. The
/// endpoint's wait queue is implicit: the set of `Blocked` tasks.
#[cfg(target_arch = "riscv64")]
fn find_blocked(s: &Scheduler, ep: EndpointId, role: IpcRole) -> Option<usize> {
    let want = TaskState::Blocked { endpoint: ep, role };
    s.tasks
        .iter()
        .position(|slot| slot.as_ref().is_some_and(|t| t.state == want))
}

/// Park the current task in `state` and switch to the next runnable task.
/// The caller has NOT yet changed the task's state; this sets it to `state`,
/// switches away, and returns only when something wakes it (sets it `Ready`).
/// Called from the trap handler with interrupts off (like `exit_current`).
/// The always-`Ready` idle task guarantees a successor exists.
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
    // SAFETY: as in yield_now/exit_current — distinct 'static contexts, the
    // kernel is mapped in both address spaces, single hart, interrupts off.
    // Execution resumes here when a peer wakes this task.
    unsafe {
        // Re-arm the timer for the task we switch to (see yield_now): a tick
        // that came due during this blocking switch must not preempt the new
        // task at its first instruction.
        crate::timer::rearm();
        crate::csr::satp_write(switch.2);
        switch_context(switch.0, switch.1);
    }
}

/// Block the current task at an IPC rendezvous (`Blocked{endpoint, role}`).
#[cfg(target_arch = "riscv64")]
fn block_current(endpoint: EndpointId, role: IpcRole) {
    park_current(TaskState::Blocked { endpoint, role });
}

/// Service a `recv` syscall. `a0` (= `frame.regs[9]`) is the capability
/// index of the endpoint. If a sender is already waiting, take its message
/// and wake it; otherwise block until one arrives. On return the message
/// is in the ABI registers (or `a0 = usize::MAX` if the capability check
/// failed).
#[cfg(target_arch = "riscv64")]
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
                // A grant sender carries the delegated cap; move it to us to
                // install into our reply-slot below.
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
                // A grant set our pending_grant directly when it woke us; or, if
                // we picked up a blocked grant sender, it was moved onto us.
                if let Some(cap) = s.tasks[cur].as_mut().unwrap().pending_grant.take() {
                    install_cap(s, cur, reply_slot, cap);
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
    let resolved = SCHED.with(|s| {
        let cur = s.current;
        let ep = cap_lookup(&s.tasks[cur].as_ref().unwrap().caps, ep_idx)?;
        Some((cur, ep, s.tasks[cur].as_ref().unwrap().name))
    });
    match resolved {
        None => frame.regs[9] = usize::MAX,
        Some((cur, ep, name)) => {
            let n = revoke_endpoint(ep, cur);
            crate::println!("cap: '{name}' revoked endpoint {ep} from {n} holder(s)");
            frame.regs[9] = n;
        }
    }
}

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
    let ep_idx = frame.regs[9]; // a0
    let src_slot = frame.regs[10]; // a1
    let badge = frame.regs[11]; // a2
    enum G {
        BadCap,
        Delivered,
        Block(EndpointId),
    }
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

/// Receive a [`Message`](crate::task::Message) on the endpoint named by
/// capability `cap_idx`, for a **kernel** (S-mode) task (which cannot use the
/// U-mode `ecall` IPC path). Blocks until a sender arrives, then returns its
/// message. The sender uses the ordinary [`ipc_send`]; delivery is identical
/// regardless of the receiver's privilege. Panics if the caller lacks the
/// capability (a kernel-config bug).
#[cfg(target_arch = "riscv64")]
pub fn recv_message(cap_idx: usize) -> crate::task::Message {
    enum K {
        Got(crate::task::Message),
        Block(EndpointId),
    }
    let step = SCHED.with(|s| {
        let cur = s.current;
        let ep = cap_lookup(&s.tasks[cur].as_ref().unwrap().caps, cap_idx)
            .expect("recv_message: caller lacks the endpoint capability");
        match find_blocked(s, ep, IpcRole::Send) {
            Some(si) => {
                let msg = s.tasks[si].as_ref().unwrap().message;
                s.tasks[si].as_mut().unwrap().state = TaskState::Ready;
                K::Got(msg)
            }
            None => K::Block(ep),
        }
    });
    match step {
        K::Got(msg) => msg,
        K::Block(ep) => {
            block_current(ep, IpcRole::Recv);
            SCHED.with(|s| s.tasks[s.current].as_ref().unwrap().message)
        }
    }
}

/// Make a synchronous `call` (send a request, then block for the reply) from a
/// **kernel** (S-mode) task — the client counterpart to [`recv_message`]. The
/// `badge` crosses to the server; the return value is the server's reply badge.
/// Reuses the same call/reply rendezvous and one-shot reply-cap machinery as
/// the U-mode `call` syscall ([`ipc_call`]). Panics if the caller lacks the
/// capability (a kernel-config bug).
#[cfg(target_arch = "riscv64")]
pub fn call_message(cap_idx: usize, badge: usize) -> usize {
    enum K {
        AwaitReply,
        Queue(EndpointId),
    }
    let msg = Message { badge, data: [0, 0, 0] };
    let step = SCHED.with(|s| {
        let cur = s.current;
        let ep = cap_lookup(&s.tasks[cur].as_ref().unwrap().caps, cap_idx)
            .expect("call_message: caller lacks the endpoint capability");
        match find_blocked(s, ep, IpcRole::Recv) {
            Some(ri) => {
                // A server is recv-blocked: hand it the request, bind us as its
                // caller (it mints the reply cap on wake), and ready it.
                s.tasks[ri].as_mut().unwrap().message = msg;
                s.tasks[ri].as_mut().unwrap().caller = Some(cur);
                s.tasks[ri].as_mut().unwrap().state = TaskState::Ready;
                K::AwaitReply
            }
            None => {
                // No server yet: queue our request; a server's recv binds us
                // and moves us to AwaitingReply.
                s.tasks[cur].as_mut().unwrap().message = msg;
                K::Queue(ep)
            }
        }
    });
    match step {
        K::AwaitReply => park_current(TaskState::AwaitingReply),
        K::Queue(ep) => block_current(ep, IpcRole::Call),
    }
    SCHED.with(|s| s.tasks[s.current].as_ref().unwrap().message.badge)
}

/// Service a `send` syscall. `a0` = capability index, `a1` = badge,
/// `a2..a4` = data. If a receiver is waiting, deliver to it and wake it;
/// otherwise block until one arrives. `a0` becomes `0` on success or
/// `usize::MAX` if the capability check failed.
#[cfg(target_arch = "riscv64")]
pub fn ipc_send(frame: &mut crate::trap::TrapFrame) {
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
                    "ipc: '{}' send rejected (no capability)",
                    s.tasks[cur].as_ref().unwrap().name
                );
                return SendStep::BadCap;
            }
        };
        match find_blocked(s, ep, IpcRole::Recv) {
            Some(ri) => {
                crate::println!(
                    "ipc: '{}' -> '{}' badge {:#x}",
                    s.tasks[cur].as_ref().unwrap().name,
                    s.tasks[ri].as_ref().unwrap().name,
                    msg.badge
                );
                s.tasks[ri].as_mut().unwrap().message = msg;
                s.tasks[ri].as_mut().unwrap().state = TaskState::Ready;
                SendStep::Delivered
            }
            None => {
                s.tasks[cur].as_mut().unwrap().message = msg;
                SendStep::Block(ep)
            }
        }
    });
    match step {
        SendStep::BadCap => frame.regs[9] = usize::MAX,
        SendStep::Delivered => frame.regs[9] = 0,
        SendStep::Block(ep) => {
            block_current(ep, IpcRole::Send);
            frame.regs[9] = 0; // a receiver took our message and woke us
        }
    }
}

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

/// The `seal` syscall (Phase 14): AEAD-encrypt the plaintext word in `a0` on the
/// encrypted channel, gated by a `Session` capability at slot `SESSION_CAP`. On
/// success `a0 = 0`, `a1` = ciphertext, `a2`/`a3` = tag, `a4` = nonce;
/// `a0 = usize::MAX` if the caller lacks the capability.
#[cfg(target_arch = "riscv64")]
pub fn seal(frame: &mut crate::trap::TrapFrame) {
    const SESSION_CAP: usize = 0;
    let ok = SCHED.with(|s| {
        crate::cap::has_session(&s.tasks[s.current].as_ref().unwrap().caps, SESSION_CAP)
    });
    if !ok {
        let name = SCHED.with(|s| s.tasks[s.current].as_ref().unwrap().name);
        crate::println!("crypto: '{name}' seal refused (no Session capability)");
        frame.regs[9] = usize::MAX;
        return;
    }
    let plain = frame.regs[9].to_le_bytes();
    let (ct, tag, nonce) = crate::channel::seal_word(plain);
    frame.regs[9] = 0;
    frame.regs[10] = usize::from_le_bytes(ct);
    frame.regs[11] = usize::from_le_bytes(tag[..8].try_into().unwrap());
    frame.regs[12] = usize::from_le_bytes(tag[8..].try_into().unwrap());
    frame.regs[13] = nonce as usize;
}

/// The `open` syscall (Phase 14): AEAD-decrypt the ciphertext word in `a0` (tag
/// in `a1`/`a2`, nonce in `a3`), gated by a `Session` capability at slot
/// `SESSION_CAP`. On success `a0 = 0`, `a1` = plaintext; `a0 = usize::MAX` on a
/// bad tag or no capability.
#[cfg(target_arch = "riscv64")]
pub fn open(frame: &mut crate::trap::TrapFrame) {
    const SESSION_CAP: usize = 0;
    let ok = SCHED.with(|s| {
        crate::cap::has_session(&s.tasks[s.current].as_ref().unwrap().caps, SESSION_CAP)
    });
    if !ok {
        frame.regs[9] = usize::MAX;
        return;
    }
    let ct = frame.regs[9].to_le_bytes();
    let mut tag = [0u8; 16];
    tag[..8].copy_from_slice(&frame.regs[10].to_le_bytes());
    tag[8..].copy_from_slice(&frame.regs[11].to_le_bytes());
    let nonce = frame.regs[12] as u64;
    match crate::channel::open_word(ct, tag, nonce) {
        Some(plain) => {
            frame.regs[9] = 0;
            frame.regs[10] = usize::from_le_bytes(plain);
        }
        None => frame.regs[9] = usize::MAX,
    }
}

/// Service a `getrandom` syscall: if the caller holds a `Randomness`
/// capability at index `a0` (= `frame.regs[9]`), fill `a1..a4` with 32 fresh
/// bytes from the kernel entropy pool and set `a0 = 0`; otherwise set `a0 =
/// usize::MAX` and return no bytes. Every outcome is logged with the caller's
/// name.
#[cfg(target_arch = "riscv64")]
pub fn getrandom(frame: &mut crate::trap::TrapFrame) {
    let cap_idx = frame.regs[9];
    let ok = SCHED.with(|s| {
        let t = s.tasks[s.current].as_ref().unwrap();
        if crate::cap::has_randomness(&t.caps, cap_idx) {
            crate::println!("rng: served 32 bytes to '{}'", t.name);
            true
        } else {
            crate::println!("rng: request rejected (no capability)");
            false
        }
    });
    if !ok {
        frame.regs[9] = usize::MAX;
        return;
    }
    let bytes = crate::entropy::next_seed();
    let word = |i: usize| {
        let mut c = [0u8; 8];
        c.copy_from_slice(&bytes[i..i + 8]);
        u64::from_le_bytes(c) as usize
    };
    frame.regs[9] = 0; // a0 = ok
    frame.regs[10] = word(0); // a1
    frame.regs[11] = word(8); // a2
    frame.regs[12] = word(16); // a3
    frame.regs[13] = word(24); // a4
}

/// Wake the task blocked in `wait_irq` for `irq` (set it `Ready`) and return
/// its name (for logging). `None` if no task is waiting — the masked source
/// stays pending and is redelivered at the next `wait_irq`.
#[cfg(target_arch = "riscv64")]
pub fn wake_irq(irq: u32) -> Option<&'static str> {
    SCHED.with(|s| {
        let want = TaskState::WaitingIrq(irq);
        let pos = s
            .tasks
            .iter()
            .position(|slot| slot.as_ref().is_some_and(|t| t.state == want))?;
        s.tasks[pos].as_mut().unwrap().state = TaskState::Ready;
        Some(s.tasks[pos].as_ref().unwrap().name)
    })
}

/// Service a `wait_irq` syscall: if the caller holds an `Interrupt` capability
/// at index `a0` (= `frame.regs[9]`), unmask that IRQ in the PLIC and block the
/// task until the interrupt handler wakes it (`a0 = 0` on return). Otherwise
/// `a0 = usize::MAX`. Runs in the trap handler with interrupts off, so the
/// just-unmasked interrupt is delivered only after we have blocked.
#[cfg(target_arch = "riscv64")]
pub fn wait_irq(frame: &mut crate::trap::TrapFrame) {
    let cap_idx = frame.regs[9];
    let irq = SCHED.with(|s| {
        crate::cap::interrupt_irq(&s.tasks[s.current].as_ref().unwrap().caps, cap_idx)
    });
    match irq {
        None => frame.regs[9] = usize::MAX,
        Some(irq) => {
            // Complete the previous claim (the handler claims but does not
            // complete — claim's "in service" state masks re-delivery while the
            // U-mode driver acks the device). Completing here re-arms the source;
            // the source stays enabled at the PLIC throughout (QEMU only asserts
            // SEIP on the rising edge of an *enabled* source, so we never unmask
            // after the device has already asserted). The first call completes
            // nothing (harmless).
            crate::plic::complete(irq);
            park_current(TaskState::WaitingIrq(irq));
            frame.regs[9] = 0; // woken by the interrupt handler
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(name: &'static str, state: TaskState) -> Task {
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
            caller: None,
            pending_grant: None,
        }
    }

    /// Three ready tasks in slots 0..3, slot 3 empty; `current` running.
    fn three_tasks(current: usize) -> Scheduler {
        let mut s = Scheduler::new();
        s.tasks[0] = Some(task("A", TaskState::Ready));
        s.tasks[1] = Some(task("B", TaskState::Ready));
        s.tasks[2] = Some(task("C", TaskState::Ready));
        s.current = current;
        s.tasks[current].as_mut().unwrap().state = TaskState::Running;
        s
    }

    #[test]
    fn rotates_to_the_next_ready_task() {
        let s = three_tasks(0);
        assert_eq!(s.pick_next(), 1);
    }

    #[test]
    fn wraps_around_and_skips_empty_slots() {
        // current = 2; slot 3 is empty, so the next ready is slot 0.
        let s = three_tasks(2);
        assert_eq!(s.pick_next(), 0);
    }

    #[test]
    fn single_task_returns_itself() {
        let mut s = Scheduler::new();
        s.tasks[0] = Some(task("solo", TaskState::Running));
        s.current = 0;
        assert_eq!(s.pick_next(), 0);
    }

    #[test]
    fn no_other_ready_task_returns_current() {
        // All three present but the other two are Running (artificial) —
        // no Ready peer, so pick_next keeps the current task.
        let mut s = three_tasks(0);
        s.tasks[1].as_mut().unwrap().state = TaskState::Running;
        s.tasks[2].as_mut().unwrap().state = TaskState::Running;
        assert_eq!(s.pick_next(), 0);
    }

    #[test]
    fn pick_next_skips_exited_slots() {
        // current = 0 running; slot 1 Exited, slot 2 Ready -> pick 2.
        let mut s = three_tasks(0);
        s.tasks[1].as_mut().unwrap().state = TaskState::Exited;
        assert_eq!(s.pick_next(), 2);
    }

    #[test]
    fn pick_next_skips_blocked_slots() {
        // current = 0 running; slot 1 Blocked on recv, slot 2 Ready -> pick 2.
        let mut s = three_tasks(0);
        s.tasks[1].as_mut().unwrap().state =
            TaskState::Blocked { endpoint: 0, role: IpcRole::Recv };
        assert_eq!(s.pick_next(), 2);
    }

    #[test]
    fn full_rotation_visits_every_task_once() {
        let mut s = three_tasks(0);
        let mut order = alloc_order(&mut s);
        order.sort_unstable();
        assert_eq!(order, [0, 1, 2]);
    }

    // Simulate three cooperative yields and record who runs.
    fn alloc_order(s: &mut Scheduler) -> [usize; 3] {
        let mut seen = [0usize; 3];
        for slot in seen.iter_mut() {
            let next = s.pick_next();
            s.tasks[s.current].as_mut().unwrap().state = TaskState::Ready;
            s.tasks[next].as_mut().unwrap().state = TaskState::Running;
            s.current = next;
            *slot = next;
        }
        seen
    }
}
