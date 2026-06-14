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

/// A task's scheduling state. Only the two states Phase 2c needs:
/// blocking/sleeping and exit arrive with the concepts that need them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Runnable, not currently on the CPU.
    Ready,
    /// Currently on the CPU (exactly one task at a time, single hart).
    Running,
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
}
