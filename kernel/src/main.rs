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
    use kernel_arch_riscv64::{cap::Capability, mem, println, sched, timer, trap};
    use kernel_common::PROJECT_NAME;

    /// Rust entry, called from the boot assembly with the arguments
    /// OpenSBI gave us. Never returns: a kernel has nowhere to return to.
    #[no_mangle]
    extern "C" fn kmain(hartid: usize, _dtb: usize) -> ! {
        println!();
        println!("{GREETING} from {PROJECT_NAME} - Phase 3b-iii (hart {hartid})");

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
        pqc_demo();

        // Phase 3b-iii: two isolated U-mode components communicate ONLY
        // through a capability-checked synchronous endpoint. Each task runs
        // in its own address space (3b-ii). server/client are granted the
        // endpoint capability at boot; rogue is not. Spawn order puts server
        // in slot 0, so enter() runs it first — it recv's and blocks until
        // the client sends. These tasks don't print, so they have no user
        // data page (the (0, 0) data region maps nothing).
        use core::mem::size_of;
        let ustack = |base: usize| (base, base + size_of::<UStack>());
        const NO_DATA: (usize, usize) = (0, 0);

        let su = ustack(core::ptr::addr_of!(US_SERVER) as usize);
        let server = sched::spawn_user("server", server_task, su.1,
            core::ptr::addr_of!(KS_SERVER) as usize + TASK_STACK,
            mem::build_user_space(su, NO_DATA));
        sched::grant_cap(server, EP_CAP, Capability::Endpoint(EP0));

        let cu = ustack(core::ptr::addr_of!(US_CLIENT) as usize);
        let client = sched::spawn_user("client", client_task, cu.1,
            core::ptr::addr_of!(KS_CLIENT) as usize + TASK_STACK,
            mem::build_user_space(cu, NO_DATA));
        sched::grant_cap(client, EP_CAP, Capability::Endpoint(EP0));

        // rogue gets NO endpoint capability — its send must be rejected.
        let ru = ustack(core::ptr::addr_of!(US_ROGUE) as usize);
        let _rogue = sched::spawn_user("rogue", rogue_task, ru.1,
            core::ptr::addr_of!(KS_ROGUE) as usize + TASK_STACK,
            mem::build_user_space(ru, NO_DATA));

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

    /// Phase 3c: prove the post-quantum KEM runs on the bare kernel — an
    /// ML-KEM-768 round-trip whose two shared secrets must agree. The seed
    /// is FIXED and NOT secret (real entropy seeding is deferred); this
    /// proves the algorithm runs no_std/no-alloc on the kernel, not that it
    /// is securely keyed.
    fn pqc_demo() {
        const PQC_DEMO_SEED: [u8; 32] = [0x3c; 32];
        match kernel_crypto::ml_kem768_agree(PQC_DEMO_SEED) {
            Some(_) => println!("pqc: ML-KEM-768 round-trip ok (shared secret agreed)"),
            None => println!("pqc: ML-KEM-768 FAIL (secrets disagreed)"),
        }
    }

    /// The demo endpoint id and the capability-table slot it is installed in.
    const EP0: usize = 0;
    const EP_CAP: usize = 0;

    /// Per-task kernel stack size (also the trap stack a U-mode task's
    /// traps land on). 16 KiB; per-task guard pages stay deferred.
    const TASK_STACK: usize = 16 * 1024;
    type KStack = [u8; TASK_STACK];
    static mut KS_SERVER: KStack = [0; TASK_STACK];
    static mut KS_CLIENT: KStack = [0; TASK_STACK];
    static mut KS_ROGUE: KStack = [0; TASK_STACK];
    static mut KS_IDLE: KStack = [0; TASK_STACK];

    /// A page-aligned U-mode stack (2 pages), so each task's stack occupies
    /// its own pages — the unit of isolation (3b-ii). These tasks pass the
    /// whole IPC message in registers, so they need no user data page.
    const USER_STACK_SIZE: usize = 8 * 1024;
    #[repr(C, align(4096))]
    struct UStack([u8; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_SERVER: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_CLIENT: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_ROGUE: UStack = UStack([0; USER_STACK_SIZE]);

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

    /// send syscall (a7 = 4): a0 = cap index, a1 = badge, a2..a4 = data
    /// (zero here). Returns a0: 0 on success, or `usize::MAX` if the caller
    /// lacks the capability.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and may block this
    /// task until a receiver arrives.
    #[inline(always)]
    unsafe fn sys_send(cap: usize, badge: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 4usize,
            inout("a0") cap => ret,
            in("a1") badge,
            in("a2") 0usize,
            in("a3") 0usize,
            in("a4") 0usize,
            options(nostack),
        );
        ret
    }

    /// recv syscall (a7 = 5): a0 = cap index. Returns the badge in a0 (the
    /// data words come back in a1..a3, unused here). Blocks until a sender
    /// arrives.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and may block this
    /// task until a sender arrives.
    #[inline(always)]
    unsafe fn sys_recv(cap: usize) -> usize {
        let badge;
        core::arch::asm!(
            "ecall",
            in("a7") 5usize,
            inout("a0") cap => badge,
            out("a1") _,
            out("a2") _,
            out("a3") _,
            options(nostack),
        );
        badge
    }

    /// The server component: receive one message on the endpoint (blocking
    /// until the client sends), then exit with the received badge as its
    /// code — proving the value arrived across the address-space boundary.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn server_task() -> ! {
        // SAFETY: recv blocks until a sender arrives; we hold the endpoint
        // capability at EP_CAP. The badge is the value the client sent.
        unsafe {
            let badge = sys_recv(EP_CAP);
            sys_exit(badge)
        }
    }

    /// The client component: send one message (badge 0x42) to the endpoint,
    /// then exit cleanly. The server is waiting, so the send delivers and
    /// wakes it without blocking.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn client_task() -> ! {
        // SAFETY: we hold the endpoint capability at EP_CAP.
        unsafe {
            sys_send(EP_CAP, 0x42);
            sys_exit(0)
        }
    }

    /// The rogue: it was granted NO endpoint capability, so its send is
    /// rejected (returns usize::MAX). It exits 7 to prove the capability
    /// check enforced — it could not reach the endpoint server/client share.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn rogue_task() -> ! {
        // SAFETY: send is always sound; here it returns an error because we
        // hold no capability at EP_CAP.
        unsafe {
            let r = sys_send(EP_CAP, 0xdead);
            sys_exit(if r == usize::MAX { 7 } else { 0 })
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
