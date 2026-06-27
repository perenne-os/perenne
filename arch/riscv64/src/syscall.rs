//! System calls — the only way a U-mode task reaches the kernel.
//!
//! A user task executes `ecall`, which traps as `scause = 8` ("environment
//! call from U-mode"). The ABI: `a7` = syscall number, `a0..` = arguments,
//! and the return value goes back in `a0`. Three calls now exist: `print` (1)
//! and `exit` (2) from Phase 3a, and `yield` (3) added in Phase 3b-i.
//!
//! Pure here (host-testable): decoding the syscall number and the
//! confused-deputy guard that validates a user-supplied buffer lies inside
//! the task's own memory. The gated dispatcher below reads user memory
//! (inside a `SUM` window) and writes the console.

/// A decoded syscall request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Syscall {
    /// `print(ptr, len)` — write `len` bytes at `ptr` to the console.
    Print,
    /// `exit(code)` — terminate the calling task.
    Exit,
    /// `yield()` — give up the CPU to the next ready task.
    Yield,
    /// `send(cap, badge, data..)` — synchronous IPC send.
    Send,
    /// `recv(cap)` — synchronous IPC receive.
    Recv,
    /// `restart(cap)` — the self-healer asks the kernel to restart the
    /// component named by a Restart capability (Phase 5b).
    Restart,
    /// `call(cap, badge, data..)` — send a request and block for the reply.
    Call,
    /// `reply(badge, data..)` — answer the caller the kernel recorded.
    Reply,
    /// `getrandom(cap)` — draw 32 bytes from the kernel entropy pool.
    Getrandom,
    /// `wait_irq(cap)` — block until the device interrupt named by the cap.
    WaitIrq,
    /// `grant(ep_cap, src_cap_slot, badge)` — delegate (copy) the capability in
    /// the sender's `src_cap_slot` to a peer recv-blocked on `ep_cap`.
    Grant,
    /// An unrecognized syscall number (a user bug, not a kernel bug).
    Unknown(usize),
}

/// Map a raw `a7` syscall number to a [`Syscall`].
pub fn decode_syscall(a7: usize) -> Syscall {
    match a7 {
        1 => Syscall::Print,
        2 => Syscall::Exit,
        3 => Syscall::Yield,
        4 => Syscall::Send,
        5 => Syscall::Recv,
        6 => Syscall::Restart,
        7 => Syscall::Call,
        8 => Syscall::Reply,
        9 => Syscall::Getrandom,
        10 => Syscall::WaitIrq,
        11 => Syscall::Grant,
        n => Syscall::Unknown(n),
    }
}

/// The confused-deputy guard: is `[ptr, ptr + len)` fully inside the
/// half-open user region `[lo, hi)`?
///
/// Rejects (returns `false`) on: a pointer below `lo`, an end past `hi`,
/// and `ptr + len` overflowing `usize` (a wrap that could otherwise slip
/// a kernel address past a naive `end <= hi` check). A zero-length buffer
/// at a valid `ptr` is accepted.
pub fn validate_user_buffer(lo: usize, hi: usize, ptr: usize, len: usize) -> bool {
    match ptr.checked_add(len) {
        Some(end) => ptr >= lo && end <= hi,
        None => false, // ptr + len wrapped — reject outright
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_known_syscalls() {
        assert_eq!(decode_syscall(1), Syscall::Print);
        assert_eq!(decode_syscall(2), Syscall::Exit);
    }

    #[test]
    fn decodes_unknown_syscall() {
        assert_eq!(decode_syscall(99), Syscall::Unknown(99));
    }

    #[test]
    fn decodes_yield_syscall() {
        assert_eq!(decode_syscall(3), Syscall::Yield);
    }

    #[test]
    fn accepts_a_buffer_inside_the_region() {
        // region [0x1000, 0x2000); buffer [0x1100, 0x1110) fits.
        assert!(validate_user_buffer(0x1000, 0x2000, 0x1100, 0x10));
    }

    #[test]
    fn accepts_a_buffer_flush_against_the_end() {
        // ends exactly at hi (half-open: 0x1ff0 + 0x10 == 0x2000).
        assert!(validate_user_buffer(0x1000, 0x2000, 0x1ff0, 0x10));
    }

    #[test]
    fn rejects_a_buffer_starting_below_the_region() {
        assert!(!validate_user_buffer(0x1000, 0x2000, 0x0ff0, 0x10));
    }

    #[test]
    fn rejects_a_buffer_overrunning_the_end() {
        // ends at 0x2001, one past hi.
        assert!(!validate_user_buffer(0x1000, 0x2000, 0x1ff1, 0x10));
    }

    #[test]
    fn rejects_a_length_that_wraps_usize() {
        // ptr + len overflows; a naive end-check could wrap below hi.
        assert!(!validate_user_buffer(0x1000, 0x2000, 0x1100, usize::MAX));
    }

    #[test]
    fn accepts_a_zero_length_buffer_at_a_valid_pointer() {
        assert!(validate_user_buffer(0x1000, 0x2000, 0x1100, 0));
    }

    #[test]
    fn decodes_send_and_recv() {
        assert_eq!(decode_syscall(4), Syscall::Send);
        assert_eq!(decode_syscall(5), Syscall::Recv);
    }

    #[test]
    fn decodes_restart_syscall() {
        assert_eq!(decode_syscall(6), Syscall::Restart);
    }

    #[test]
    fn decodes_call_and_reply() {
        assert_eq!(decode_syscall(7), Syscall::Call);
        assert_eq!(decode_syscall(8), Syscall::Reply);
    }

    #[test]
    fn decodes_getrandom_syscall() {
        assert_eq!(decode_syscall(9), Syscall::Getrandom);
    }

    #[test]
    fn decodes_wait_irq_syscall() {
        assert_eq!(decode_syscall(10), Syscall::WaitIrq);
        assert_eq!(decode_syscall(11), Syscall::Grant);
    }
}

/// What the trap handler should do after a syscall returns.
#[cfg(target_arch = "riscv64")]
pub enum Outcome {
    /// Resume the user task (the handler advances `sepc` past the `ecall`).
    Resume,
    /// The task asked to exit with this code.
    Exit(usize),
    /// The task asked to yield; the handler advances `sepc`, then reschedules.
    Yield,
}

/// Largest `print` we copy in one syscall. The demo strings are short; a
/// longer buffer is silently truncated to this (a real kernel would loop).
#[cfg(target_arch = "riscv64")]
const PRINT_MAX: usize = 256;

/// Service a U-mode `ecall`. Reads the ABI registers from `frame`,
/// dispatches, and writes the return value into the `a0` slot. Reading
/// user memory for `print` happens only inside a validated `SUM` window.
///
/// Register/`TrapFrame` mapping: `regs[n-1]` holds `x_n`, so `a0` = `x10`
/// is `regs[9]`, `a1` = `x11` is `regs[10]`, `a7` = `x17` is `regs[16]`.
#[cfg(target_arch = "riscv64")]
pub fn dispatch(frame: &mut crate::trap::TrapFrame) -> Outcome {
    let a7 = frame.regs[16];
    let a0 = frame.regs[9];
    let a1 = frame.regs[10];
    match decode_syscall(a7) {
        Syscall::Print => {
            let written = sys_print(a0, a1);
            frame.regs[9] = written; // return value in a0
            Outcome::Resume
        }
        Syscall::Exit => Outcome::Exit(a0),
        Syscall::Yield => Outcome::Yield,
        Syscall::Send => {
            crate::sched::ipc_send(frame);
            Outcome::Resume
        }
        Syscall::Recv => {
            crate::sched::ipc_recv(frame);
            Outcome::Resume
        }
        Syscall::Restart => {
            crate::sched::restart(frame);
            Outcome::Resume
        }
        Syscall::Call => {
            crate::sched::ipc_call(frame);
            Outcome::Resume
        }
        Syscall::Reply => {
            crate::sched::ipc_reply(frame);
            Outcome::Resume
        }
        Syscall::Getrandom => {
            crate::sched::getrandom(frame);
            Outcome::Resume
        }
        Syscall::WaitIrq => {
            crate::sched::wait_irq(frame);
            Outcome::Resume
        }
        Syscall::Unknown(_) => {
            frame.regs[9] = usize::MAX; // -1: unknown syscall
            Outcome::Resume
        }
    }
}

/// Validate, then copy `[ptr, ptr+len)` out of user memory and print it.
/// Returns the number of bytes written, or `usize::MAX` if validation
/// failed (the confused-deputy guard refused the pointer).
#[cfg(target_arch = "riscv64")]
fn sys_print(ptr: usize, len: usize) -> usize {
    let (lo, hi) = crate::mem::user_data_bounds();
    if !validate_user_buffer(lo, hi, ptr, len) {
        return usize::MAX;
    }
    let n = core::cmp::min(len, PRINT_MAX);
    let mut buf = [0u8; PRINT_MAX];
    // SAFETY: the range is validated to lie within the user data region,
    // which is mapped R+U. SUM is opened only for this copy and cleared
    // immediately after, so the kernel cannot read kernel memory here.
    unsafe {
        crate::csr::sstatus_set_sum();
        for i in 0..n {
            buf[i] = core::ptr::read_volatile((ptr + i) as *const u8);
        }
        crate::csr::sstatus_clear_sum();
    }
    // Print as a lossy string; the demo message is valid UTF-8.
    crate::print!("{}", core::str::from_utf8(&buf[..n]).unwrap_or("<non-utf8>"));
    n
}
