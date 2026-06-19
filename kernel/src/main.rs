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

        // Phase 3b-ii: each U-mode task runs in its OWN address space. We
        // build a private page table per task (kernel cloned in, shared
        // .user_text, plus only that task's stack + data page), then spawn
        // it with that satp. ping/pong cooperate via yield and exit; hog is
        // preempted; snoop follows a pointer (in its own page) into pong's
        // data page — unmapped in snoop's space — and is contained. idle is
        // a kernel task on the master satp. Built here on the master satp
        // (new page-table frames come from free RAM, which only it maps).
        use core::mem::size_of;
        let region = |base: usize, size: usize| (base, base + size);

        let us_ping = region(core::ptr::addr_of!(US_PING) as usize, size_of::<UStack>());
        let ud_ping = region(core::ptr::addr_of!(UD_PING) as usize, size_of::<UData>());
        let ping_satp = mem::build_user_space(us_ping, ud_ping);
        sched::spawn_user("ping", user_ping, us_ping.1,
            core::ptr::addr_of!(KS_PING) as usize + TASK_STACK, ping_satp);

        let us_pong = region(core::ptr::addr_of!(US_PONG) as usize, size_of::<UStack>());
        let ud_pong = region(core::ptr::addr_of!(UD_PONG) as usize, size_of::<UData>());
        let pong_satp = mem::build_user_space(us_pong, ud_pong);
        sched::spawn_user("pong", user_pong, us_pong.1,
            core::ptr::addr_of!(KS_PONG) as usize + TASK_STACK, pong_satp);

        let us_hog = region(core::ptr::addr_of!(US_HOG) as usize, size_of::<UStack>());
        let ud_hog = region(core::ptr::addr_of!(UD_HOG) as usize, size_of::<UData>());
        let hog_satp = mem::build_user_space(us_hog, ud_hog);
        sched::spawn_user("hog", user_hog, us_hog.1,
            core::ptr::addr_of!(KS_HOG) as usize + TASK_STACK, hog_satp);

        // snoop's data page holds a pointer to pong's data page; snoop's
        // tree maps SNOOP_TARGET but NOT pong's page, so following it faults.
        let us_snoop = region(core::ptr::addr_of!(US_SNOOP) as usize, size_of::<UStack>());
        let snoop_data = region(core::ptr::addr_of!(SNOOP_TARGET) as usize, size_of::<Snoop>());
        let snoop_satp = mem::build_user_space(us_snoop, snoop_data);
        sched::spawn_user("snoop", user_snoop, us_snoop.1,
            core::ptr::addr_of!(KS_SNOOP) as usize + TASK_STACK, snoop_satp);

        sched::spawn("idle", idle, core::ptr::addr_of!(KS_IDLE) as usize + TASK_STACK);

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
    /// In .bss → mapped global by map_kernel_sections, so a task traps onto
    /// its own kernel stack in its own address space.
    static mut KS_PING: KStack = [0; TASK_STACK];
    static mut KS_PONG: KStack = [0; TASK_STACK];
    static mut KS_HOG: KStack = [0; TASK_STACK];
    static mut KS_SNOOP: KStack = [0; TASK_STACK];
    static mut KS_IDLE: KStack = [0; TASK_STACK];

    /// A page-aligned U-mode stack (2 pages). Page alignment is required so
    /// each task's stack occupies its OWN pages — the unit of isolation.
    const USER_STACK_SIZE: usize = 8 * 1024;
    #[repr(C, align(4096))]
    struct UStack([u8; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_PING: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_PONG: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_HOG: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_SNOOP: UStack = UStack([0; USER_STACK_SIZE]);

    /// A page-aligned 4 KiB U-mode data page. Each task gets its own so its
    /// data is page-isolated from every other task. The message bytes sit
    /// at the start; the rest is zero.
    #[repr(C, align(4096))]
    struct UData([u8; 4096]);

    /// Fill a 4 KiB page with `prefix` at the front, zero after (const so
    /// the page is a link-time constant in .user_data, not a runtime write).
    const fn page_with(prefix: &[u8]) -> [u8; 4096] {
        let mut p = [0u8; 4096];
        let mut i = 0;
        while i < prefix.len() {
            p[i] = prefix[i];
            i += 1;
        }
        p
    }

    #[link_section = ".user_data"]
    static UD_PING: UData = UData(page_with(b"user: ping\n"));
    #[link_section = ".user_data"]
    static UD_PONG: UData = UData(page_with(b"user: pong\n"));
    #[link_section = ".user_data"]
    static UD_HOG: UData = UData(page_with(b"user: hog\n"));

    /// Length of each task's message (bytes to print): "user: ping\n" and
    /// "user: pong\n" are 11; "user: hog\n" is 10.
    const PING_LEN: usize = 11;
    const PONG_LEN: usize = 11;
    const HOG_LEN: usize = 10;

    /// A page-aligned page holding a pointer into pong's data page. Mapped
    /// into snoop's address space (R-U); pong's page is NOT — so snoop can
    /// load the pointer from its own page but faults when it dereferences.
    #[repr(C, align(4096))]
    struct Snoop(&'static u8);
    #[link_section = ".user_data"]
    static SNOOP_TARGET: Snoop = Snoop(&UD_PONG.0[0]);

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

    /// Cooperating U-mode task: print its own data page twice, yielding to
    /// its peer between each, then exit cleanly. In its own address space.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_ping() -> ! {
        // SAFETY: UD_PING is mapped R-U in this task's space; the kernel
        // validates the pointer (it lies in .user_data) and reads it.
        unsafe {
            sys_print(core::ptr::addr_of!(UD_PING) as *const u8, PING_LEN);
            sys_yield();
            sys_print(core::ptr::addr_of!(UD_PING) as *const u8, PING_LEN);
            sys_yield();
            sys_exit(0)
        }
    }

    /// Peer of `user_ping`: same protocol — the two interleave to prove
    /// round-robin, now each in its own address space.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_pong() -> ! {
        // SAFETY: see `user_ping`.
        unsafe {
            sys_print(core::ptr::addr_of!(UD_PONG) as *const u8, PONG_LEN);
            sys_yield();
            sys_print(core::ptr::addr_of!(UD_PONG) as *const u8, PONG_LEN);
            sys_yield();
            sys_exit(0)
        }
    }

    /// The hog: two cooperative rounds, then spin forever without yielding.
    /// The timer preempts it (proving a U-mode task is preemptible, now
    /// across an address-space switch).
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_hog() -> ! {
        // SAFETY: see `user_ping`.
        unsafe {
            sys_print(core::ptr::addr_of!(UD_HOG) as *const u8, HOG_LEN);
            sys_yield();
            sys_print(core::ptr::addr_of!(UD_HOG) as *const u8, HOG_LEN);
            sys_yield();
        }
        loop {
            core::hint::spin_loop();
        }
    }

    /// The cross-task snooper: follow a pointer (kept in our OWN mapped page)
    /// into another task's data page. That page is not mapped in our address
    /// space, so the load faults in U-mode and the kernel contains us while
    /// the others keep running — the isolation proof.
    ///
    /// `lb` is inline asm (not `read_volatile`, which a debug build may turn
    /// into a `jalr` into kernel text → the wrong fault). It faults in U-mode
    /// before retiring; control never returns here.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_snoop() -> ! {
        let target: *const u8 = SNOOP_TARGET.0;
        let _v: u8;
        // SAFETY: deliberate cross-task isolation probe; the lb faults in
        // U-mode (the target page is unmapped in our space) before it
        // completes, so control never returns.
        unsafe {
            core::arch::asm!("lb {v}, 0({p})", v = out(reg) _v, p = in(reg) target, options(nostack));
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
