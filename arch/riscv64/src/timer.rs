//! Timer heartbeat via the SBI TIME extension.
//!
//! SBI timers are one-shot: each interrupt must arm the next one, so
//! the "periodic" tick is really a chain of deadlines computed from the
//! `time` CSR.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::{csr, sbi};

/// The `time` CSR's tick rate on QEMU virt: 10 MHz. **QEMU-specific
/// constant** — real hardware (Phase 4) must read the timebase from the
/// device tree instead.
const TIMEBASE_HZ: u64 = 10_000_000;

/// One heartbeat per second (in `time` ticks).
const TICK_INTERVAL: u64 = TIMEBASE_HZ;

/// Monotonic count of timer interrupts since boot.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Arm the first tick and enable timer interrupts.
/// [`crate::trap::init`] must have been called first — enabling
/// interrupts with no handler installed turns the first tick into a
/// wild jump.
pub fn start() {
    arm_next();
    // SAFETY: the caller contract above is exactly the safety condition
    // of these two CSR writes.
    unsafe {
        csr::sie_enable_timer();
        csr::sstatus_enable_interrupts();
    }
}

/// Called by the trap dispatcher on each supervisor timer interrupt.
pub(crate) fn on_tick() {
    let n = TICKS.fetch_add(1, Ordering::Relaxed) + 1;
    crate::println!("tick: {n}");
    arm_next();
}

/// Schedule the next interrupt one interval from now.
fn arm_next() {
    sbi::set_timer(csr::time() + TICK_INTERVAL);
}
