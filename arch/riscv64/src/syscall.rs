//! System calls — the only way a U-mode task reaches the kernel.
//!
//! A user task executes `ecall`, which traps as `scause = 8` ("environment
//! call from U-mode"). The ABI: `a7` = syscall number, `a0..` = arguments,
//! and the return value goes back in `a0`. Two calls exist in Phase 3a:
//! `print` (1) and `exit` (2).
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
    /// An unrecognized syscall number (a user bug, not a kernel bug).
    Unknown(usize),
}

/// Map a raw `a7` syscall number to a [`Syscall`].
pub fn decode_syscall(a7: usize) -> Syscall {
    match a7 {
        1 => Syscall::Print,
        2 => Syscall::Exit,
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
}
