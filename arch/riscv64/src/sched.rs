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

/// Maximum concurrent tasks: the 3b-i demo runs five (idle + four U-mode
/// tasks) plus one slot of headroom.
pub const MAX_TASKS: usize = 6;

/// The run queue: a fixed array of optional tasks and the index of the
/// one currently running.
pub struct Scheduler {
    tasks: [Option<Task>; MAX_TASKS],
    current: usize,
}

impl Scheduler {
    /// An empty scheduler; `spawn` fills slots and `enter` starts it.
    pub const fn new() -> Self {
        Self { tasks: [None, None, None, None, None, None], current: 0 }
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

/// Reached only if a task's entry function returns — a Phase 2c bug
/// (entries must loop forever; task exit is deferred).
#[cfg(target_arch = "riscv64")]
#[no_mangle]
extern "C" fn task_has_returned() -> ! {
    panic!("task entry returned (task exit is not implemented in Phase 2c)");
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
pub fn spawn(name: &'static str, entry: extern "C" fn() -> !, stack_top: usize) {
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
        });
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
        Some((old, new))
    });

    if let Some((old, new)) = switch {
        // SAFETY: both pointers address `Context`s inside the 'static
        // SCHED, valid for the whole program. `next != current` above
        // guarantees `old` and `new` are distinct slots, so they never
        // alias. Single hart + the disabled interrupts above mean no
        // other code touches them while the assembly runs. Execution
        // resumes here when this task is picked again.
        unsafe { switch_context(old, new) };
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

    let first = SCHED.with(|s| {
        s.current = 0;
        let t = s.tasks[0].as_mut().expect("enter() with no spawned task");
        t.state = TaskState::Running;
        core::ptr::addr_of!(t.context)
    });

    let mut throwaway = Context::zeroed();
    // SAFETY: `first` addresses a Context in the 'static SCHED; the
    // throwaway is a valid (never-restored) save target on this stack.
    unsafe { switch_context(core::ptr::addr_of_mut!(throwaway), first) };
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
) {
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
        );
        s.tasks[slot] = Some(Task {
            context,
            state: TaskState::Ready,
            stack_top: kstack_top,
            name,
        });
    });
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
        match reason {
            ExitReason::Exited(code) => {
                crate::println!("sched: task '{}' exited (code {code})", s.tasks[current].as_ref().unwrap().name)
            }
            ExitReason::Killed(cause) => {
                crate::println!("sched: task '{}' killed by {cause:?}", s.tasks[current].as_ref().unwrap().name)
            }
        }
        s.tasks[current].as_mut().unwrap().state = TaskState::Exited;
        let next = s.pick_next();
        assert_ne!(next, current, "exit_current: no runnable successor (idle must be Ready)");
        s.tasks[next].as_mut().unwrap().state = TaskState::Running;
        s.current = next;
        // Save into the dying slot's own context (never resumed).
        let old = core::ptr::addr_of_mut!(s.tasks[current].as_mut().unwrap().context);
        let new = core::ptr::addr_of!(s.tasks[next].as_ref().unwrap().context);
        (old, new)
    });
    // SAFETY: the `assert_ne!` above guarantees `old` and `new` are distinct
    // slots, so they never alias; both are 'static contexts in SCHED. The
    // dying (old) slot is never run again, so its saved state is irrelevant.
    // Single hart + interrupts off in the trap handler mean nothing else
    // touches them. Control resumes in `next`.
    unsafe { switch_context(switch.0, switch.1) };
    unreachable!("exit_current resumes in the next task, never here")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task(name: &'static str, state: TaskState) -> Task {
        Task { context: Context::zeroed(), state, stack_top: 0, name }
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
