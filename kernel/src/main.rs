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
    use kernel_arch_riscv64::{println, timer, trap};
    use kernel_common::PROJECT_NAME;

    /// Rust entry, called from the boot assembly with the arguments
    /// OpenSBI gave us. Never returns: a kernel has nowhere to return to.
    #[no_mangle]
    extern "C" fn kmain(hartid: usize, _dtb: usize) -> ! {
        println!();
        println!("{GREETING} from {PROJECT_NAME} - Phase 2a (hart {hartid})");

        trap::init();
        // Deliberate breakpoint: proves the handler catches an exception
        // and execution RESUMES past it (the smoke test's
        // "survived breakpoint" line can only print if recovery worked).
        unsafe { core::arch::asm!("ebreak") };
        println!("survived breakpoint");

        timer::start();
        println!("(kernel idles; heartbeat ~1/s; exit QEMU with Ctrl-A then X)");
        park()
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
