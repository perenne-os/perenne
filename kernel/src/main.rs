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
        println!("{GREETING} from {PROJECT_NAME} - Phase 3b-i (hart {hartid})");

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

        // Phase 3b-i: one run queue holding a kernel idle task and four
        // U-mode tasks. ping/pong cooperate via the yield syscall and exit
        // cleanly; bad reaches into kernel memory and is contained; hog
        // never yields, so the first timer tick preempts a U-mode task.
        // Spawn order puts ping in slot 0, so enter() runs it first.
        // addr_of! forms each static stack's top address WITHOUT a reference
        // (no unsafe, no static_mut_refs lint) — the existing 2c/3a pattern.
        sched::spawn_user("ping", user_ping,
            core::ptr::addr_of!(US_PING) as usize + USER_STACK_SIZE,
            core::ptr::addr_of!(KS_PING) as usize + TASK_STACK);
        sched::spawn_user("pong", user_pong,
            core::ptr::addr_of!(US_PONG) as usize + USER_STACK_SIZE,
            core::ptr::addr_of!(KS_PONG) as usize + TASK_STACK);
        sched::spawn_user("hog", user_hog,
            core::ptr::addr_of!(US_HOG) as usize + USER_STACK_SIZE,
            core::ptr::addr_of!(KS_HOG) as usize + TASK_STACK);
        sched::spawn_user("bad", user_bad,
            core::ptr::addr_of!(US_BAD) as usize + USER_STACK_SIZE,
            core::ptr::addr_of!(KS_BAD) as usize + TASK_STACK);
        sched::spawn("idle", idle, core::ptr::addr_of!(IDLE_STACK) as usize + TASK_STACK);

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

    /// Per-task kernel stack size (also the trap stack a U-mode task's
    /// traps land on). 16 KiB; per-task guard pages stay deferred.
    const TASK_STACK: usize = 16 * 1024;
    type KStack = [u8; TASK_STACK];

    /// One kernel/trap stack per U-mode task, plus one for the idle task.
    static mut KS_PING: KStack = [0; TASK_STACK];
    static mut KS_PONG: KStack = [0; TASK_STACK];
    static mut KS_HOG: KStack = [0; TASK_STACK];
    static mut KS_BAD: KStack = [0; TASK_STACK];
    static mut IDLE_STACK: KStack = [0; TASK_STACK];

    /// U-mode task stacks live in `.user_data` so mem::init maps them RW-U.
    const USER_STACK_SIZE: usize = 8 * 1024;
    type UStack = [u8; USER_STACK_SIZE];
    #[link_section = ".user_data"]
    static mut US_PING: UStack = [0; USER_STACK_SIZE];
    #[link_section = ".user_data"]
    static mut US_PONG: UStack = [0; USER_STACK_SIZE];
    #[link_section = ".user_data"]
    static mut US_HOG: UStack = [0; USER_STACK_SIZE];
    #[link_section = ".user_data"]
    static mut US_BAD: UStack = [0; USER_STACK_SIZE];

    /// Messages the U-mode tasks ask the kernel to print. In `.user_data`
    /// (R-U) so the confused-deputy guard accepts their pointers and the
    /// SUM-window copy can read them. Each ends in '\n'; lengths are exact.
    #[link_section = ".user_data"]
    static PING_MSG: [[u8; 13]; 2] = [*b"user: ping 0\n", *b"user: ping 1\n"];
    #[link_section = ".user_data"]
    static PONG_MSG: [[u8; 13]; 2] = [*b"user: pong 0\n", *b"user: pong 1\n"];
    #[link_section = ".user_data"]
    static HOG_MSG: [[u8; 13]; 2] = [*b"user: hog  0\n", *b"user: hog  1\n"];

    /// Print syscall (a7 = 1): a0 = ptr, a1 = len. `inline(always)` so it
    /// folds into the user entry and the `.user_text` page stays
    /// self-contained (no call into kernel `.text`). a0 is in/out: the
    /// kernel returns the byte count there, which we discard.
    ///
    /// # Safety
    /// `ptr`/`len` must describe the buffer the kernel should print; the
    /// kernel validates the range before reading it.
    #[inline(always)]
    unsafe fn sys_print(ptr: *const u8, len: usize) {
        core::arch::asm!(
            "ecall",
            in("a7") 1usize,
            inout("a0") ptr => _,
            in("a1") len,
            options(nostack),
        );
    }

    /// Exit syscall (a7 = 2): a0 = code. Never returns.
    ///
    /// # Safety
    /// Always sound; the kernel terminates the task and never resumes it.
    #[inline(always)]
    unsafe fn sys_exit(code: usize) -> ! {
        core::arch::asm!(
            "ecall",
            in("a7") 2usize,
            in("a0") code,
            options(nostack, noreturn),
        );
    }

    /// Yield syscall (a7 = 3): give up the CPU; returns when rescheduled.
    ///
    /// # Safety
    /// Always sound; the kernel reschedules and resumes this task later.
    #[inline(always)]
    unsafe fn sys_yield() {
        core::arch::asm!("ecall", in("a7") 3usize, options(nostack));
    }

    /// Cooperating U-mode task: print two lines, yielding to its peer
    /// between each, then exit cleanly.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_ping() -> ! {
        // SAFETY: PING_MSG is in .user_data (R-U); we pass addresses the
        // kernel validates and reads. yield/exit are always sound.
        unsafe {
            sys_print(PING_MSG[0].as_ptr(), PING_MSG[0].len());
            sys_yield();
            sys_print(PING_MSG[1].as_ptr(), PING_MSG[1].len());
            sys_yield();
            sys_exit(0)
        }
    }

    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_pong() -> ! {
        // SAFETY: see `user_ping`.
        unsafe {
            sys_print(PONG_MSG[0].as_ptr(), PONG_MSG[0].len());
            sys_yield();
            sys_print(PONG_MSG[1].as_ptr(), PONG_MSG[1].len());
            sys_yield();
            sys_exit(0)
        }
    }

    /// The hog: two cooperative rounds, then spin forever without yielding.
    /// The timer must preempt it (it is the sole non-yielder once ping/pong
    /// exit), which proves a U-mode task is preemptible.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_hog() -> ! {
        // SAFETY: see `user_ping`.
        unsafe {
            sys_print(HOG_MSG[0].as_ptr(), HOG_MSG[0].len());
            sys_yield();
            sys_print(HOG_MSG[1].as_ptr(), HOG_MSG[1].len());
            sys_yield();
        }
        loop {
            core::hint::spin_loop();
        }
    }

    /// The misbehaving U-mode task: load a byte from kernel address space.
    /// The kernel page is non-U, so the U-mode load faults and the kernel
    /// contains the task (it never reaches the exit below); the scheduler
    /// keeps running.
    ///
    /// We use inline asm for the load instead of `read_volatile` to guarantee
    /// the instruction is emitted here (in `.user_text`) and not called out to
    /// a kernel-text helper — in a debug build the latter would generate a
    /// `jalr` into kernel `.text`, which faults as InstructionPageFault before
    /// the load can fault as LoadPageFault.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_bad() -> ! {
        // 0x80200000 is the kernel .text base (mapped R-X-G, no U bit).
        // The lb faults in U-mode with LoadPageFault before retiring.
        // Control never returns here; the scheduler contains this task.
        unsafe {
            core::arch::asm!(
                "lb {tmp}, 0({addr})",
                addr = in(reg) 0x8020_0000usize,
                tmp = out(reg) _,
                options(nostack),
            );
            sys_exit(0) // unreachable: the load above faults first
        }
    }

    /// The idle kernel (S-mode) task: cooperatively yield when other tasks
    /// are ready; `wfi`-sleep when it is the only runnable task. Never
    /// exits, so it is always a valid successor for `exit_current` and keeps
    /// the system alive after every U-mode task has finished.
    extern "C" fn idle() -> ! {
        loop {
            sched::yield_now();
            // SAFETY: wfi just waits for the next interrupt (the timer).
            unsafe { core::arch::asm!("wfi") };
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
