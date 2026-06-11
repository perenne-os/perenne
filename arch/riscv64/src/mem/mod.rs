//! Memory management: physical frame allocation and Sv39 paging.
//!
//! Same layout discipline as [`crate::trap`]: pure logic is ungated and
//! unit-tested on the host; everything touching real memory or CSRs is
//! gated to `target_arch = "riscv64"`.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

pub mod frame;
pub mod paging;

/// Interior mutability for a single-hart kernel: gives `&mut` access to
/// a static without a real lock. `timer.rs` gets away with a bare
/// `AtomicU64`, but a bitmap-plus-counters struct can't be a single
/// atomic, so the exclusivity argument moves here and is enforced:
/// re-entrant access (e.g. a trap handler interrupting a call in
/// progress) panics. In 2b trap context never allocates; the panic
/// keeps that invariant honest if a later phase forgets.
pub struct SingleHartCell<T> {
    inner: UnsafeCell<T>,
    in_use: AtomicBool,
}

// SAFETY: one hart, and `with` panics on re-entry, so the `&mut` handed
// to the closure is never aliased.
unsafe impl<T> Sync for SingleHartCell<T> {}

impl<T> SingleHartCell<T> {
    pub const fn new(value: T) -> Self {
        Self { inner: UnsafeCell::new(value), in_use: AtomicBool::new(false) }
    }

    /// Run `f` with exclusive access to the value.
    ///
    /// If `f` panics the cell stays locked and every later call panics
    /// as "re-entrant" — acceptable because a kernel panic is already
    /// fatal (abort, no unwinding), so the stuck flag is unreachable.
    pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
        assert!(
            !self.in_use.swap(true, Ordering::Acquire),
            "re-entrant access to single-hart cell"
        );
        // SAFETY: the in_use flag guarantees no other &mut exists (see
        // the Sync impl note).
        let result = f(unsafe { &mut *self.inner.get() });
        self.in_use.store(false, Ordering::Release);
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_gives_mutable_access() {
        static CELL: SingleHartCell<u32> = SingleHartCell::new(7);
        let seen = CELL.with(|v| {
            *v += 1;
            *v
        });
        assert_eq!(seen, 8);
    }

    #[test]
    #[should_panic(expected = "re-entrant")]
    fn reentrant_access_panics() {
        static CELL: SingleHartCell<u32> = SingleHartCell::new(0);
        CELL.with(|_| CELL.with(|_| {}));
    }
}
