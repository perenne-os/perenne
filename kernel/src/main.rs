//! Kernel entry point.
//!
//! Bare-metal (`target_os = "none"`, i.e. the riscv64 cross-build):
//! a freestanding binary — no std, no main. OpenSBI hands control to
//! `_start` (boot.rs), which calls [`bare::kmain`].
//!
//! On the host this compiles to a tiny stub `main` instead, so the
//! Phase 0 promise — `cargo build` / `cargo test` stay green on the
//! host — still holds.
#![cfg_attr(target_os = "none", no_std)]
#![cfg_attr(target_os = "none", no_main)]

#[cfg(target_os = "none")]
mod boot;

#[cfg(target_os = "none")]
mod bare {
    use core::panic::PanicInfo;

    use kernel::GREETING;
    use kernel_arch_riscv64::{mem, println, sched, timer, trap};
    use kernel_common::PROJECT_NAME;

    /// Rust entry, called from the boot assembly with the arguments
    /// OpenSBI gave us. Never returns: a kernel has nowhere to return to.
    #[no_mangle]
    extern "C" fn kmain(hartid: usize, _dtb: usize) -> ! {
        println!();
        println!("{GREETING} from {PROJECT_NAME} - Phase 2c (hart {hartid})");

        trap::init();
        // Deliberate breakpoint: proves the handler catches an exception
        // and execution RESUMES past it (the smoke test's
        // "survived breakpoint" line can only print if recovery worked).
        unsafe { core::arch::asm!("ebreak") };
        println!("survived breakpoint");

        mem::init();
        println!(
            "paging: sv39 on ({} of {} frames free)",
            mem::free_frames(),
            mem::total_frames()
        );
        wx_probe();
        frame_roundtrip();

        // Phase 2c: spawn three tasks and hand the CPU to the scheduler.
        // Interrupts are enabled here (timer::start) BEFORE entering, so
        // the cooperative round-robin runs in the sub-millisecond window
        // before the first tick (~1 s away); preemption then takes over.
        // addr_of! takes each static stack's address without forming a
        // reference (no unsafe needed, no static_mut_refs lint); the top
        // of the array is this task's initial stack pointer.
        sched::spawn("A", task_a, core::ptr::addr_of!(STACK_A) as usize + TASK_STACK);
        sched::spawn("B", task_b, core::ptr::addr_of!(STACK_B) as usize + TASK_STACK);
        sched::spawn("C", task_c, core::ptr::addr_of!(STACK_C) as usize + TASK_STACK);
        timer::start();
        println!("(scheduler starting; heartbeat ~1/s; exit QEMU with Ctrl-A then X)");
        sched::enter()
    }

    /// 2b's deliberate fault (like 2a's ebreak): prove the MMU blocks
    /// writes to read-only memory. The store is inline asm so Rust never
    /// sees a write through a shared reference — that would be UB even
    /// though the store faults before retiring.
    fn wx_probe() {
        static RODATA_PROBE: u64 = 0x600D_C0DE;
        trap::expect_wx_fault();
        // SAFETY: the store targets .rodata, mapped R-- — it faults, the
        // trap handler consumes the probe flag and skips the instruction.
        unsafe {
            core::arch::asm!(
                "sd zero, 0({addr})",
                addr = in(reg) core::ptr::addr_of!(RODATA_PROBE) as usize,
                options(nostack),
            );
        }
        assert!(
            !trap::wx_fault_pending(),
            "W^X broken: rodata write did not fault"
        );
        // SAFETY: reading our own static; volatile so the check can't be
        // const-folded away.
        let value = unsafe { core::ptr::read_volatile(&RODATA_PROBE) };
        assert_eq!(value, 0x600D_C0DE, "W^X broken: rodata was modified");
        println!("wx: rodata write blocked");
    }

    /// Prove the allocator round-trips: alloc -> write -> free ->
    /// re-alloc returns the same (re-zeroed) frame.
    fn frame_roundtrip() {
        let first = mem::frame::alloc_zeroed().expect("frame alloc failed");
        let p = first.0 as *mut u64;
        // SAFETY: `first` is a 4 KiB frame we own, identity-mapped RW.
        unsafe {
            assert_eq!(p.read_volatile(), 0, "frame not zeroed on alloc");
            p.write_volatile(0x600D_F00D);
            assert_eq!(p.read_volatile(), 0x600D_F00D, "frame not writable");
        }
        mem::frame::free(first);
        let second = mem::frame::alloc_zeroed().expect("frame re-alloc failed");
        assert_eq!(first, second, "first-fit should recycle the freed frame");
        // SAFETY: same frame, still mapped RW.
        unsafe {
            assert_eq!(p.read_volatile(), 0, "recycled frame not re-zeroed");
        }
        mem::frame::free(second);
        println!("frames: alloc/free ok");
    }

    use core::sync::atomic::{AtomicBool, Ordering};

    /// Per-task kernel stack size. 16 KiB is ample for these print loops;
    /// per-task guard pages are deferred (see the Phase 2c spec §3.5).
    const TASK_STACK: usize = 16 * 1024;

    static mut STACK_A: [u8; TASK_STACK] = [0; TASK_STACK];
    static mut STACK_B: [u8; TASK_STACK] = [0; TASK_STACK];
    static mut STACK_C: [u8; TASK_STACK] = [0; TASK_STACK];

    /// Set by task C when it stops yielding. A and B only ever observe it
    /// as `true` if a timer preemption schedules them while C hogs the
    /// CPU — which is exactly the preemption proof.
    static HOGGING: AtomicBool = AtomicBool::new(false);

    extern "C" fn task_a() -> ! {
        worker("A")
    }

    extern "C" fn task_b() -> ! {
        worker("B")
    }

    /// The cooperative citizens: two visible steps yielding between each
    /// (proving round-robin), then spin yielding. When C starts hogging,
    /// the next time preemption lets this task run it prints the proof
    /// line once, then goes quiet.
    fn worker(name: &str) -> ! {
        for n in 0..2 {
            println!("sched: {name} step {n}");
            sched::yield_now();
        }
        loop {
            if HOGGING.load(Ordering::Acquire) {
                println!("sched: {name} preempted the hog");
                loop {
                    sched::yield_now();
                }
            }
            sched::yield_now();
        }
    }

    /// The hog: two cooperative steps, then a tight loop that NEVER
    /// yields. Without preemption the kernel would be stuck here forever;
    /// the timer tick is what lets A and B run again.
    extern "C" fn task_c() -> ! {
        for n in 0..2 {
            println!("sched: C step {n}");
            sched::yield_now();
        }
        println!("sched: C hogging (no yield)");
        HOGGING.store(true, Ordering::Release);
        loop {
            core::hint::spin_loop();
        }
    }

    /// Halt this hart: `wfi` sleeps until an interrupt; the trap handler
    /// runs on each timer tick, then the loop goes back to sleep.
    fn park() -> ! {
        loop {
            unsafe { core::arch::asm!("wfi") };
        }
    }

    /// Freestanding binaries must provide their own panic behavior:
    /// report on the console, then park. No unwinding (panic = abort).
    #[panic_handler]
    fn panic(info: &PanicInfo) -> ! {
        println!("KERNEL PANIC: {info}");
        park()
    }
}

#[cfg(not(target_os = "none"))]
fn main() {
    println!("kernel host stub - the real kernel runs under QEMU: ./tools/run-qemu.ps1");
}
