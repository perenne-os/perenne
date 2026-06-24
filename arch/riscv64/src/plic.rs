//! The RISC-V PLIC (platform-level interrupt controller) — routes device
//! interrupts to the kernel. Pure offset arithmetic is host-tested; the gated
//! functions access the PLIC MMIO (mapped by `mem::init` into every tree).
//!
//! We target hart 0's S-mode context (context 1 on QEMU `virt`). The source
//! enable bit is toggled per-IRQ: `wait_irq` unmasks, the handler masks on
//! claim (the device line stays asserted until the U-mode driver acks it).

/// hart 0's S-mode interrupt context on QEMU `virt`.
pub const CONTEXT: usize = 1;

/// Byte offset of source `irq`'s priority register.
pub const fn priority_offset(irq: u32) -> usize {
    irq as usize * 4
}
/// Byte offset of the enable *word* holding `irq`'s bit for `ctx`.
pub const fn enable_offset(ctx: usize, irq: u32) -> usize {
    0x2000 + ctx * 0x80 + (irq as usize / 32) * 4
}
/// Byte offset of `ctx`'s priority threshold register.
pub const fn threshold_offset(ctx: usize) -> usize {
    0x20_0000 + ctx * 0x1000
}
/// Byte offset of `ctx`'s claim/complete register.
pub const fn claim_offset(ctx: usize) -> usize {
    0x20_0004 + ctx * 0x1000
}

#[cfg(target_arch = "riscv64")]
use core::sync::atomic::{AtomicUsize, Ordering};

#[cfg(target_arch = "riscv64")]
static PLIC_BASE: AtomicUsize = AtomicUsize::new(0);

#[cfg(target_arch = "riscv64")]
fn base() -> usize {
    PLIC_BASE.load(Ordering::Acquire)
}
// SAFETY (all): the PLIC base is mapped R-W into every address space by
// `mem::init`/`map_kernel_sections` before any of these run, and `init` has
// stored a non-zero base.
#[cfg(target_arch = "riscv64")]
unsafe fn r(off: usize) -> u32 {
    unsafe { core::ptr::read_volatile((base() + off) as *const u32) }
}
#[cfg(target_arch = "riscv64")]
unsafe fn w(off: usize, v: u32) {
    unsafe { core::ptr::write_volatile((base() + off) as *mut u32, v) };
}

/// Record the PLIC base and accept any priority on our context (threshold 0).
/// Leaves all sources disabled (the enable bit is managed by `enable`/`disable`).
#[cfg(target_arch = "riscv64")]
pub fn init(plic_base: usize) {
    PLIC_BASE.store(plic_base, Ordering::Release);
    unsafe { w(threshold_offset(CONTEXT), 0) };
}

/// Give `irq` a non-zero priority so it can be delivered.
#[cfg(target_arch = "riscv64")]
pub fn set_priority(irq: u32, priority: u32) {
    unsafe { w(priority_offset(irq), priority) };
}

/// Unmask `irq` for our context.
#[cfg(target_arch = "riscv64")]
pub fn enable(irq: u32) {
    let off = enable_offset(CONTEXT, irq);
    unsafe { w(off, r(off) | (1 << (irq % 32))) };
}

/// Mask `irq` for our context.
#[cfg(target_arch = "riscv64")]
pub fn disable(irq: u32) {
    let off = enable_offset(CONTEXT, irq);
    unsafe { w(off, r(off) & !(1 << (irq % 32))) };
}

/// Claim the highest-priority pending interrupt for our context (0 = none).
#[cfg(target_arch = "riscv64")]
pub fn claim() -> u32 {
    unsafe { r(claim_offset(CONTEXT)) }
}

/// Signal completion of `irq` to the PLIC.
#[cfg(target_arch = "riscv64")]
pub fn complete(irq: u32) {
    unsafe { w(claim_offset(CONTEXT), irq) };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offsets_match_the_spike_verified_layout() {
        assert_eq!(priority_offset(8), 0x20);
        assert_eq!(enable_offset(1, 8), 0x2080);
        assert_eq!(threshold_offset(1), 0x20_1000);
        assert_eq!(claim_offset(1), 0x20_1004);
    }
}
