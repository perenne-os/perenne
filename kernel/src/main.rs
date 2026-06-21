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
    use kernel_arch_riscv64::{cap::Capability, console, dt, mem, println, sched, timer, trap};
    use kernel_common::PROJECT_NAME;

    /// Rust entry, called from the boot assembly with the arguments
    /// OpenSBI gave us. Never returns: a kernel has nowhere to return to.
    #[no_mangle]
    extern "C" fn kmain(hartid: usize, dtb: usize) -> ! {
        println!();
        println!("{GREETING} from {PROJECT_NAME} - Phase 4a (hart {hartid})");

        trap::init();
        // Deliberate breakpoint: proves the handler catches an exception
        // and execution RESUMES past it (the smoke test's
        // "survived breakpoint" line can only print if recovery worked).
        unsafe { core::arch::asm!("ebreak") };
        println!("survived breakpoint");

        // Phase 4a: learn the machine from the device tree instead of
        // hardcoding QEMU's. SAFETY: `dtb` is the firmware-provided FDT
        // pointer; the MMU is still off, so the physical read is valid, and
        // the frame allocator (armed inside mem::init below) has not yet
        // touched the blob's memory.
        let machine = unsafe { dt::from_ptr(dtb) };
        // Phase 4b: switch the console from the SBI firmware path to the
        // discovered UART. The MMU is still off, so these first direct writes
        // hit the UART's physical MMIO; mem::init maps the page next.
        console::use_uart(machine.uart_base, machine.uart_reg_shift);
        println!("console: ns16550a @ {:#x} (device tree)", machine.uart_base);
        println!(
            "dt: {} MiB RAM @ {:#x}, timebase {} Hz",
            machine.ram_size >> 20,
            machine.ram_base,
            machine.timebase_hz
        );

        mem::init(machine.ram_base + machine.ram_size, machine.uart_base);
        println!(
            "paging: sv39 on ({} of {} frames free)",
            mem::free_frames(),
            mem::total_frames()
        );
        wx_probe();
        frame_roundtrip();
        pqc_demo();

        // First user-space component (ADR 0007): the RTC server owns the
        // goldfish RTC — its MMIO is mapped R-U into THIS component only — and
        // serves time-reads over a capability-checked endpoint; the kernel
        // never touches the RTC. rtc_client holds the endpoint cap and asks;
        // rogue does not and is refused. The server is slot 0, so enter() runs
        // it first — it recv's and blocks before the client sends.
        use core::mem::size_of;
        let ustack = |base: usize| (base, base + size_of::<UStack>());
        const NO_DEVICE: (usize, usize) = (0, 0);
        let rtc = (machine.rtc_base, machine.rtc_base + 0x1000); // one MMIO page

        let us = ustack(core::ptr::addr_of!(US_RTC) as usize);
        let rtc_srv = sched::spawn_user("rtc", rtc_server, us.1,
            core::ptr::addr_of!(KS_RTC) as usize + TASK_STACK,
            mem::build_user_space(us, rtc)); // RTC MMIO mapped R-U into the server only
        sched::grant_cap(rtc_srv, EP_CAP, Capability::Endpoint(EP0));

        let cu = ustack(core::ptr::addr_of!(US_CLIENT) as usize);
        let client = sched::spawn_user("client", rtc_client, cu.1,
            core::ptr::addr_of!(KS_CLIENT) as usize + TASK_STACK,
            mem::build_user_space(cu, NO_DEVICE));
        sched::grant_cap(client, EP_CAP, Capability::Endpoint(EP0));

        // rogue gets NO endpoint capability — its send must be refused.
        let ru = ustack(core::ptr::addr_of!(US_ROGUE) as usize);
        let _rogue = sched::spawn_user("rogue", rogue_task, ru.1,
            core::ptr::addr_of!(KS_ROGUE) as usize + TASK_STACK,
            mem::build_user_space(ru, NO_DEVICE));

        // Phase 5b — the caged fix. The healer (the acting agent, in user
        // space) blocks on the crash endpoint before either patient runs, so
        // it is waiting when they crash.
        let hu = ustack(core::ptr::addr_of!(US_HEALER) as usize);
        let healer = sched::spawn_user("healer", healer_task, hu.1,
            core::ptr::addr_of!(KS_HEALER) as usize + TASK_STACK,
            mem::build_user_space(hu, NO_DEVICE));
        sched::grant_cap(healer, CRASH_CAP, Capability::Endpoint(sched::CRASH_EP));

        // A patient with a transient fault: crashes once, then recovers.
        let tu = ustack(core::ptr::addr_of!(US_TRANSIENT) as usize);
        let transient = sched::spawn_user("transient", transient_task, tu.1,
            core::ptr::addr_of!(KS_TRANSIENT) as usize + TASK_STACK,
            mem::build_user_space(tu, NO_DEVICE));
        // The healer holds transient's Restart cap at cap slot 1; the crash
        // notification carries badge 1 so the healer uses that exact cap.
        sched::grant_cap(healer, 1, Capability::Restart(transient));
        sched::set_crash_badge(transient, 1);

        // A patient that always crashes: exercises the retry bound.
        let fu = ustack(core::ptr::addr_of!(US_FLAKY) as usize);
        let flaky = sched::spawn_user("flaky", flaky_task, fu.1,
            core::ptr::addr_of!(KS_FLAKY) as usize + TASK_STACK,
            mem::build_user_space(fu, NO_DEVICE));
        sched::grant_cap(healer, 2, Capability::Restart(flaky));
        sched::set_crash_badge(flaky, 2);

        sched::spawn("idle", idle, core::ptr::addr_of!(KS_IDLE) as usize + TASK_STACK);

        timer::init(machine.timebase_hz);
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

    /// The healer's cap-table slot holding its `Endpoint(CRASH_EP)` capability
    /// (it `recv`s on this to learn of crashes). Restart caps live at slots
    /// 1.. so a crash notification's badge is directly the cap slot to use.
    const CRASH_CAP: usize = 0;

    /// Per-task kernel stack size (also the trap stack a U-mode task's
    /// traps land on). 16 KiB; per-task guard pages stay deferred.
    const TASK_STACK: usize = 16 * 1024;
    type KStack = [u8; TASK_STACK];
    static mut KS_RTC: KStack = [0; TASK_STACK];
    static mut KS_CLIENT: KStack = [0; TASK_STACK];
    static mut KS_ROGUE: KStack = [0; TASK_STACK];
    static mut KS_HEALER: KStack = [0; TASK_STACK];
    static mut KS_TRANSIENT: KStack = [0; TASK_STACK];
    static mut KS_FLAKY: KStack = [0; TASK_STACK];
    static mut KS_IDLE: KStack = [0; TASK_STACK];

    /// A page-aligned U-mode stack (2 pages), so each task's stack occupies
    /// its own pages — the unit of isolation (3b-ii). These tasks pass the
    /// whole IPC message in registers, so they need no user data page.
    const USER_STACK_SIZE: usize = 8 * 1024;
    #[repr(C, align(4096))]
    struct UStack([u8; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_RTC: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_CLIENT: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_ROGUE: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_HEALER: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_TRANSIENT: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_FLAKY: UStack = UStack([0; USER_STACK_SIZE]);

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

    /// restart syscall (a7 = 6): a0 = cap index of a Restart capability.
    /// Returns a0: 0 on success, or `usize::MAX` if the capability check
    /// failed or the retry bound was reached.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and the bound.
    #[inline(always)]
    unsafe fn sys_restart(cap: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 6usize,
            inout("a0") cap => ret,
            options(nostack),
        );
        ret
    }

    /// The RTC time server: a user-space driver that exclusively owns the
    /// goldfish real-time clock — its MMIO is mapped R-U into THIS component
    /// only (3b-ii isolation). It receives one request over its endpoint,
    /// reads the clock, and reports the value as its exit code (the kernel
    /// formats and prints it). The kernel never touches the RTC.
    ///
    /// Two U-mode codegen rules shape this: (1) the MMIO read uses inline asm,
    /// not `core::ptr::read_volatile`, because in a debug build that `#[inline]`
    /// core fn may NOT be inlined and would become a call into kernel `.text`,
    /// which a U-mode task cannot fetch; (2) we report via the exit code rather
    /// than formatting a string here, so there is no buffer/`.rodata`/builtin
    /// hazard at all — the kernel does the formatting.
    ///
    /// The base 0x101000 is the goldfish-rtc MMIO the kernel discovered from
    /// the device tree and mapped into our address space (passing the base to
    /// the component is future work).
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn rtc_server() -> ! {
        // SAFETY: we hold the endpoint cap at EP_CAP; recv blocks for a request.
        let _req = unsafe { sys_recv(EP_CAP) };
        let low: u32;
        let high: u32;
        // SAFETY: the goldfish RTC page is mapped R-U in our address space;
        // reading TIME_LOW (offset 0) latches TIME_HIGH (offset 4).
        unsafe {
            core::arch::asm!(
                "lw {lo}, 0({base})",
                "lw {hi}, 4({base})",
                base = in(reg) 0x10_1000usize,
                lo = out(reg) low,
                hi = out(reg) high,
                options(nostack),
            );
        }
        let t = ((high as usize) << 32) | (low as usize);
        // SAFETY: report the clock as our exit code; the kernel prints it. A
        // large, non-zero nanosecond count proves we read live hardware.
        unsafe { sys_exit(t) }
    }

    /// A client of the RTC server: request the time once (badge 1 =
    /// "report"), then exit. Holds the endpoint capability.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn rtc_client() -> ! {
        // SAFETY: we hold the endpoint cap at EP_CAP.
        unsafe {
            sys_send(EP_CAP, 1);
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

    /// A deliberately faulty component: it reads a kernel address it does not
    /// own, faults (LoadPageFault), and is contained — the "patient" the
    /// self-healing organism diagnoses. Inline asm (not read_volatile) keeps
    /// the load in `.user_text` (a U-mode task can't call kernel code).
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn flaky_task() -> ! {
        let _v: u8;
        // SAFETY: the deliberate fault. 0x80200000 is the kernel .text base
        // (no U bit); the U-mode load faults before completing and the kernel
        // contains this component. Control never returns here.
        unsafe {
            core::arch::asm!(
                "lb {v}, 0({p})",
                v = out(reg) _v,
                p = in(reg) 0x8020_0000usize,
                options(nostack),
            );
            sys_exit(0) // unreachable: the load faults first
        }
    }

    /// The self-healer (Phase 5b) — the acting agent, in user space. It blocks
    /// on the crash endpoint; each crash notification's badge IS the cap index
    /// of the crashed component's Restart capability, so it simply asks the
    /// kernel to restart that component. The kernel is the cage: it
    /// capability-checks, enforces the retry bound, and logs. Register-only —
    /// no `.rodata`, no printing (the kernel logs the actions).
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn healer_task() -> ! {
        loop {
            // SAFETY: we hold Endpoint(CRASH_EP) at CRASH_CAP; recv blocks
            // until the kernel reports a crash. The returned badge is the cap
            // index of the crashed component's Restart capability.
            let cap_idx = unsafe { sys_recv(CRASH_CAP) };
            // SAFETY: ask the kernel to apply the playbook (a caged restart).
            unsafe { sys_restart(cap_idx) };
        }
    }

    /// A patient with a TRANSIENT fault: it crashes on its first run and
    /// recovers after the healer restarts it. The kernel passes the launch
    /// generation in `a0` (0 = first run, >0 = a restart); on the first run we
    /// fault like `flaky`, and on any restart we run to completion and exit 0
    /// — proving the component serves again. Register-only (reads `a0` via
    /// inline asm before any other code; the no-arg `-> !` prologue does not
    /// touch `a0`).
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn transient_task() -> ! {
        let generation: usize;
        // SAFETY: read the launch generation the kernel placed in a0.
        unsafe {
            core::arch::asm!("mv {g}, a0", g = out(reg) generation, options(nomem, nostack, preserves_flags));
        }
        if generation == 0 {
            let _v: u8;
            // SAFETY: the deliberate first-run fault (a transient bug). The
            // U-mode load of kernel .text faults (LoadPageFault); the kernel
            // contains, diagnoses, and (via the healer) restarts us.
            unsafe {
                core::arch::asm!(
                    "lb {v}, 0({p})",
                    v = out(reg) _v,
                    p = in(reg) 0x8020_0000usize,
                    options(nostack),
                );
            }
        }
        // Recovered (this is a restart): do our work and exit cleanly.
        // SAFETY: exit is always sound.
        unsafe { sys_exit(0) }
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
