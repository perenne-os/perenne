//! Timer heartbeat via the SBI TIME extension.
//!
//! SBI timers are one-shot: each interrupt must arm the next one, so
//! the "periodic" tick is really a chain of deadlines computed from the
//! `time` CSR.

use core::sync::atomic::{AtomicU64, Ordering};

use crate::{csr, sbi};

/// Ticks between heartbeats (= the timebase frequency = one second),
/// learned from the device tree by [`init`]. Zero until then — call `init`
/// before [`start`]. (Phase 4a: replaces the old hardcoded 10 MHz QEMU
/// constant; see [`crate::dt`].)
static TICK_INTERVAL: AtomicU64 = AtomicU64::new(0);

/// Monotonic count of timer interrupts since boot.
static TICKS: AtomicU64 = AtomicU64::new(0);

/// Record the platform timer frequency (from the device tree) as the
/// one-second tick interval. Call once, before [`start`].
pub fn init(timebase_hz: u64) {
    TICK_INTERVAL.store(timebase_hz, Ordering::Relaxed);
}

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
    let n = TICKS.fetch_add(1, Ordering::Relaxed) + 1; // Relaxed: single hart; no ordering between TICKS and other state needed.
    crate::println!("tick: {n}");
    arm_next();
}

/// Re-arm the timer one interval from now — also clears a currently-pending
/// timer interrupt (`sip.STIP` deasserts once `mtimecmp` is in the future).
/// The scheduler calls this when switching to a new task so a timer that came
/// due while interrupts were masked for the switch is not delivered at the new
/// task's very first instruction (which would preempt it before it runs and,
/// with an unlucky schedule, starve it — see learning note 0023).
pub fn rearm() {
    arm_next();
}

/// Schedule the next interrupt one interval from now.
fn arm_next() {
    // Arm from *now* rather than from the previous deadline, so one slow
    // tick does not cause the next to fire immediately. This accumulates
    // handler latency as drift (~microseconds on QEMU); acceptable for a
    // heartbeat. A real clock would use `prev_deadline + TICK_INTERVAL`.
    // If the computed deadline is somehow already past, SBI fires the
    // interrupt immediately — benign by contract.
    let interval = TICK_INTERVAL.load(Ordering::Relaxed);
    sbi::set_timer(csr::time() + interval);
}
