//! Tasks (in-kernel threads) and the register context a switch saves.
//!
//! A `Context` is the *callee-saved* register set — `ra`, `sp`, and
//! `s0..s11`. That is all a voluntary switch must preserve: the RISC-V
//! C calling convention says the caller already saved anything else it
//! cares about across a call, and `switch_context` (in `sched`) is
//! reached by a call. Involuntary preemption is different — it saves the
//! full 31-GPR `TrapFrame` in the trap entry assembly — but the *switch*
//! itself still only juggles this callee-saved block.
//!
//! Pure here (host-testable). The field order is a binary contract with
//! the `switch_context` assembly; the test at the bottom pins it.

use crate::cap::Capability;

/// The saved callee-saved registers of a parked task.
///
/// Field order and size are the contract with `sched`'s `switch_context`
/// assembly: `ra` at offset 0, `sp` at 8, then `s0..s11` at 16..104.
#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct Context {
    /// Return address — where `switch_context`'s `ret` resumes the task.
    pub ra: usize,
    /// The task's stack pointer.
    pub sp: usize,
    /// Callee-saved `s0..s11` (x8, x9, x18..x27).
    pub s: [usize; 12],
}

impl Context {
    /// An all-zero context. `spawn` (in `sched`) fills `ra`/`sp`/`s[0]`
    /// to forge the first run of a never-run task.
    pub const fn zeroed() -> Self {
        Self { ra: 0, sp: 0, s: [0; 12] }
    }
}

/// A task's scheduling state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Runnable, not currently on the CPU.
    Ready,
    /// Currently on the CPU (exactly one task at a time, single hart).
    Running,
    /// Terminated (clean `exit` or a fatal U-mode fault). Never scheduled
    /// again; the slot is skipped by `pick_next`. Stacks are not reclaimed
    /// (reaping is deferred).
    Exited,
    /// Waiting at an IPC rendezvous on `endpoint` as a sender or receiver.
    /// Non-`Ready`, so `pick_next` skips it until a peer wakes it.
    Blocked { endpoint: crate::cap::EndpointId, role: IpcRole },
    /// A caller whose request a server has picked up, now blocked until that
    /// server `reply`s. On no endpoint queue (so no second server can re-match
    /// it); `pick_next` skips it. (Phase: call/reply IPC.)
    AwaitingReply,
}

/// Number of capability slots in each task's table (its CSpace).
pub const CAP_SLOTS: usize = 4;

/// A register-only IPC message: a badge plus three data words, transferred
/// sender→receiver with no memory access.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Message {
    pub badge: usize,
    pub data: [usize; 3],
}

impl Message {
    /// The all-zero message (a task's default in-flight slot).
    pub const EMPTY: Self = Self { badge: 0, data: [0; 3] };
}

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

/// Which side of an IPC rendezvous a blocked task is waiting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcRole {
    Send,
    Recv,
    /// A caller queued at an endpoint waiting for a server to pick up its
    /// request (a server's `recv` matches `Send` or `Call`).
    Call,
}

/// Why a U-mode task stopped running. Passed to `sched::exit_current` by the
/// trap handler for the `exit` syscall or a fatal U-mode fault.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExitReason {
    /// The task called `exit(code)`.
    Exited(usize),
    /// The task was killed by a fatal U-mode trap (e.g. touching kernel
    /// memory). Carries the decoded cause for the diagnostic line.
    Killed(crate::trap::Cause),
}

/// Compute the `sstatus` value to load before `sret`ing into U-mode,
/// starting from the current `sstatus`. Clears SPP (bit 8) so `sret`
/// drops to U-mode, and sets SPIE (bit 5) so interrupts are enabled after
/// the return. All other bits (notably SUM = 0) are preserved.
pub fn user_sstatus(current: usize) -> usize {
    (current & !(1 << 8)) | (1 << 5)
}

/// Forge the first-run [`Context`] of a U-mode task. The first
/// `switch_context` into this context "returns" into `tramp`
/// (`sched::user_trampoline`), which reads `s0`/`s1`/`s2` and `sret`s to
/// U-mode. Both stack pointers are rounded down to a 16-byte boundary
/// (RISC-V ABI). Pure (host-tested); `sched::spawn_user` supplies `tramp`.
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

/// An in-kernel task: its parked context, its state, the top of its
/// static stack (kept for diagnostics), and a name for the demo output.
#[derive(Debug)]
pub struct Task {
    pub context: Context,
    pub state: TaskState,
    /// Top of the task's static stack (informational; `context.sp` is
    /// what actually drives execution).
    pub stack_top: usize,
    pub name: &'static str,
    /// The `satp` (address space) to load when this task runs. Kernel tasks
    /// carry the master kernel `satp`; U-mode tasks carry their private one.
    pub satp: usize,
    /// This task's capability table (CSpace). A syscall names a kernel
    /// object by an index into this table — the only authority it has.
    pub caps: [Option<Capability>; CAP_SLOTS],
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
    /// When this task is a server currently handling a `call`, the scheduler
    /// slot of the caller it must `reply` to (set on receiving a Call, cleared
    /// by `reply`). `None` otherwise. (Phase: call/reply IPC.)
    pub caller: Option<usize>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn context_layout_matches_switch_asm() {
        // If any of these change, sched's switch_context assembly must
        // change with them (and vice versa).
        assert_eq!(core::mem::size_of::<Context>(), 112);
        assert_eq!(core::mem::offset_of!(Context, ra), 0);
        assert_eq!(core::mem::offset_of!(Context, sp), 8);
        assert_eq!(core::mem::offset_of!(Context, s), 16);
    }

    #[test]
    fn zeroed_context_is_all_zero() {
        let c = Context::zeroed();
        assert_eq!(c.ra, 0);
        assert_eq!(c.sp, 0);
        assert_eq!(c.s, [0; 12]);
    }

    #[test]
    fn user_sstatus_drops_to_user_with_interrupts_on() {
        // Start from SPP = 1 (in S-mode) with SPIE clear.
        let s = user_sstatus(1 << 8);
        assert_eq!(s & (1 << 8), 0, "SPP must be 0 so sret enters U-mode");
        assert_ne!(s & (1 << 5), 0, "SPIE must be 1 so interrupts resume");
    }

    #[test]
    fn user_sstatus_preserves_other_bits() {
        // SUM (bit 18) must NOT be turned on by the forge.
        let s = user_sstatus(1 << 18);
        assert_ne!(s & (1 << 18), 0, "unrelated bits preserved");
        assert_eq!(s & (1 << 8), 0);
        assert_ne!(s & (1 << 5), 0);
    }

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

    #[test]
    fn can_restart_respects_the_bound() {
        assert!(can_restart(0, 2));
        assert!(can_restart(1, 2));
        assert!(!can_restart(2, 2), "at the bound: refuse");
        assert!(!can_restart(3, 2), "over the bound: refuse");
    }
}
