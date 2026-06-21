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
    use kernel_arch_riscv64::{cap::Capability, console, dt, entropy, mem, println, sched, task::Message, timer, trap, virtio};
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

        // Discover the virtio-rng device before paging is on (direct physical
        // MMIO reads), like the device-tree read above.
        // SAFETY: the discovered virtio-mmio bases address real register pages.
        let rng_base = unsafe { virtio::find_rng(&machine.virtio_mmio[..machine.virtio_mmio_count]) };

        mem::init(machine.ram_base + machine.ram_size, machine.uart_base);
        println!(
            "paging: sv39 on ({} of {} frames free)",
            mem::free_frames(),
            mem::total_frames()
        );
        wx_probe();
        frame_roundtrip();

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

        // The entropy component (a user-space virtio-rng driver) + its kernel
        // consumer, if the device is present. The consumer (earlier slot)
        // recv-blocks before the component sends.
        if let Some(rng) = rng_base {
            let dma_pa = mem::frame::alloc_zeroed().expect("no DMA frame for entropy").0;
            let consumer = sched::spawn("pqc", pqc_consumer,
                core::ptr::addr_of!(KS_PQC) as usize + PQC_STACK);
            sched::grant_cap(consumer, ENTROPY_CAP, Capability::Endpoint(ENTROPY_EP));

            let eu = ustack(core::ptr::addr_of!(US_ENTROPY) as usize);
            let entropy = sched::spawn_user("entropy", entropy_component, eu.1,
                core::ptr::addr_of!(KS_ENTROPY) as usize + TASK_STACK,
                mem::build_virtio_space(eu, (rng, rng + 0x1000), (dma_pa, dma_pa + 0x1000)));
            sched::grant_cap(entropy, ENTROPY_CAP, Capability::Endpoint(ENTROPY_EP));
            sched::set_launch_args(entropy, rng, dma_pa);
        } else {
            println!("entropy: no virtio-rng device found");
        }

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

    /// The demo endpoint id and the capability-table slot it is installed in.
    const EP0: usize = 0;
    const EP_CAP: usize = 0;

    /// The healer's cap-table slot holding its `Endpoint(CRASH_EP)` capability
    /// (it `recv`s on this to learn of crashes). Restart caps live at slots
    /// 1.. so a crash notification's badge is directly the cap slot to use.
    const CRASH_CAP: usize = 0;

    /// The endpoint the entropy component delivers seeds on, and the cap slot
    /// it (and the consumer) hold it in.
    const ENTROPY_EP: usize = 2;
    const ENTROPY_CAP: usize = 0;

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
    static mut KS_ENTROPY: KStack = [0; TASK_STACK];
    static mut KS_IDLE: KStack = [0; TASK_STACK];

    /// The PQC consumer runs ML-KEM-768, which in a debug build needs a much
    /// larger stack than a normal task (the boot stack was bumped to 512 KiB
    /// for the same reason in Phase 3c). Give its kernel task its own big stack.
    const PQC_STACK: usize = 512 * 1024;
    static mut KS_PQC: [u8; PQC_STACK] = [0; PQC_STACK];

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
    #[link_section = ".user_data"]
    static mut US_ENTROPY: UStack = UStack([0; USER_STACK_SIZE]);

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

    /// call syscall (a7 = 7): a0 = cap, a1 = badge, a2..a4 = data (zero here).
    /// Sends the request and blocks for the reply; returns the reply badge in
    /// a0 (reply data words are discarded here), or `usize::MAX` on bad cap.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and blocks us until a
    /// server replies.
    #[inline(always)]
    unsafe fn sys_call(cap: usize, badge: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 7usize,
            inout("a0") cap => ret,
            inout("a1") badge => _,
            inout("a2") 0usize => _,
            inout("a3") 0usize => _,
            in("a4") 0usize,
            options(nostack),
        );
        ret
    }

    /// reply syscall (a7 = 8): a0 = badge, a1..a3 = data (zero here). Answers
    /// the caller the kernel recorded. Returns 0, or `usize::MAX` if there is
    /// no pending caller.
    ///
    /// # Safety
    /// Always sound; the kernel routes the reply to the recorded caller.
    #[inline(always)]
    unsafe fn sys_reply(badge: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 8usize,
            inout("a0") badge => ret,
            in("a1") 0usize,
            in("a2") 0usize,
            in("a3") 0usize,
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

    /// send syscall (a7 = 4) carrying a full 32-byte payload: a0 = cap, a1 =
    /// badge (= word 0), a2..a4 = words 1..3. Returns a0.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and may block us.
    #[inline(always)]
    unsafe fn sys_send4(cap: usize, w0: usize, w1: usize, w2: usize, w3: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 4usize,
            inout("a0") cap => ret,
            in("a1") w0,
            in("a2") w1,
            in("a3") w2,
            in("a4") w3,
            options(nostack),
        );
        ret
    }

    /// MMIO register write (32-bit).
    /// # Safety: `base+off` must be a mapped device register.
    #[inline(always)]
    unsafe fn mmio_w(base: usize, off: usize, v: u32) {
        core::arch::asm!("sw {v}, 0({a})", v = in(reg) v, a = in(reg) base + off, options(nostack));
    }
    /// MMIO register read (32-bit).
    #[inline(always)]
    unsafe fn mmio_r(base: usize, off: usize) -> u32 {
        let v;
        core::arch::asm!("lw {v}, 0({a})", v = out(reg) v, a = in(reg) base + off, options(nostack));
        v
    }
    /// DMA stores (the rings/descriptor live in a mapped DMA page).
    #[inline(always)]
    unsafe fn dma_w64(addr: usize, v: u64) {
        core::arch::asm!("sd {v}, 0({a})", v = in(reg) v, a = in(reg) addr, options(nostack));
    }
    #[inline(always)]
    unsafe fn dma_w32(addr: usize, v: u32) {
        core::arch::asm!("sw {v}, 0({a})", v = in(reg) v, a = in(reg) addr, options(nostack));
    }
    #[inline(always)]
    unsafe fn dma_w16(addr: usize, v: u16) {
        core::arch::asm!("sh {v}, 0({a})", v = in(reg) v, a = in(reg) addr, options(nostack));
    }
    /// DMA reads.
    #[inline(always)]
    unsafe fn dma_r64(addr: usize) -> u64 {
        let v;
        core::arch::asm!("ld {v}, 0({a})", v = out(reg) v, a = in(reg) addr, options(nostack));
        v
    }
    #[inline(always)]
    unsafe fn dma_r16(addr: usize) -> u16 {
        let v;
        core::arch::asm!("lhu {v}, 0({a})", v = out(reg) v, a = in(reg) addr, options(nostack));
        v
    }
    #[inline(always)]
    unsafe fn dma_fence() {
        core::arch::asm!("fence", options(nostack));
    }

    /// The RTC time server: a user-space driver that exclusively owns the
    /// goldfish real-time clock — its MMIO is mapped R-U into THIS component
    /// only (3b-ii isolation). It is a real call/reply server: it loops,
    /// receiving a request, reading the clock, and `reply`ing the time to the
    /// caller (the kernel routes the reply to whoever called). The kernel never
    /// touches the RTC.
    ///
    /// The MMIO read uses inline asm, not `core::ptr::read_volatile`, because
    /// in a debug build that `#[inline]` core fn may NOT be inlined and would
    /// become a call into kernel `.text`, which a U-mode task cannot fetch.
    ///
    /// The base 0x101000 is the goldfish-rtc MMIO the kernel discovered from
    /// the device tree and mapped into our address space.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn rtc_server() -> ! {
        loop {
            // SAFETY: we hold the endpoint cap at EP_CAP; recv blocks for a
            // request, and the kernel records the caller so our reply reaches it.
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
            // SAFETY: reply the live clock to the caller; the client's `call`
            // returns it. Then loop to serve the next request.
            unsafe { sys_reply(t) };
        }
    }

    /// A client of the RTC server: `call` it (badge 1 = "report time") and
    /// receive the live clock back, then exit with it — proving the value
    /// crossed back from the server to the caller via call/reply.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn rtc_client() -> ! {
        // SAFETY: we hold the endpoint cap at EP_CAP; call sends the request and
        // blocks for the reply (the clock value).
        unsafe {
            let t = sys_call(EP_CAP, 1);
            sys_exit(t)
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

    /// The entropy component (user-space virtio-rng driver). The kernel maps
    /// the device's MMIO page and a zeroed DMA frame RW-U (identity) into this
    /// task and passes their bases at launch (a1 = mmio, a2 = dma). It drives
    /// the modern virtio-mmio v2 handshake + one split virtqueue entirely in
    /// inline asm (no kernel `.text`/`.rodata`), draws 32 random bytes twice,
    /// and sends each draw (32 bytes = badge + 3 words) to the `pqc` consumer.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn entropy_component() -> ! {
        let mmio: usize;
        let dma: usize;
        // SAFETY: read the launch args the kernel placed in a1/a2.
        unsafe {
            core::arch::asm!("mv {m}, a1", "mv {d}, a2",
                m = out(reg) mmio, d = out(reg) dma,
                options(nomem, nostack, preserves_flags));
        }
        let desc = dma + virtio::VQ_DESC_OFF;
        let avail = dma + virtio::VQ_AVAIL_OFF;
        let used = dma + virtio::VQ_USED_OFF;
        let buf = dma + virtio::VQ_BUF_OFF;
        // SAFETY: mmio + dma are mapped RW-U into this task; the sequence is
        // the spike-verified modern virtio-mmio v2 bring-up.
        unsafe {
            // Status handshake.
            mmio_w(mmio, virtio::STATUS, 0);
            mmio_w(mmio, virtio::STATUS, virtio::STATUS_ACK);
            mmio_w(mmio, virtio::STATUS, virtio::STATUS_ACK | virtio::STATUS_DRIVER);
            // Feature negotiation: accept only VIRTIO_F_VERSION_1 (bit 32).
            mmio_w(mmio, virtio::DEVICE_FEATURES_SEL, 1);
            let fhi = mmio_r(mmio, virtio::DEVICE_FEATURES);
            mmio_w(mmio, virtio::DRIVER_FEATURES_SEL, 1);
            mmio_w(mmio, virtio::DRIVER_FEATURES, fhi & virtio::F_VERSION_1_HI);
            mmio_w(mmio, virtio::DRIVER_FEATURES_SEL, 0);
            mmio_w(mmio, virtio::DRIVER_FEATURES, 0);
            mmio_w(mmio, virtio::STATUS,
                virtio::STATUS_ACK | virtio::STATUS_DRIVER | virtio::STATUS_FEATURES_OK);
            // Queue 0 setup (modern: independent ring addresses).
            mmio_w(mmio, virtio::QUEUE_SEL, 0);
            mmio_w(mmio, virtio::QUEUE_NUM, virtio::VQ_SIZE);
            mmio_w(mmio, virtio::QUEUE_DESC_LOW, desc as u32);
            mmio_w(mmio, virtio::QUEUE_DESC_HIGH, (desc >> 32) as u32);
            mmio_w(mmio, virtio::QUEUE_DRIVER_LOW, avail as u32);
            mmio_w(mmio, virtio::QUEUE_DRIVER_HIGH, (avail >> 32) as u32);
            mmio_w(mmio, virtio::QUEUE_DEVICE_LOW, used as u32);
            mmio_w(mmio, virtio::QUEUE_DEVICE_HIGH, (used >> 32) as u32);
            mmio_w(mmio, virtio::QUEUE_READY, 1);
            mmio_w(mmio, virtio::STATUS,
                virtio::STATUS_ACK | virtio::STATUS_DRIVER | virtio::STATUS_FEATURES_OK | virtio::STATUS_DRIVER_OK);
            // Descriptor 0: device writes 32 bytes into `buf`.
            dma_w64(desc, buf as u64);
            dma_w32(desc + 8, 32);
            dma_w16(desc + 12, virtio::VIRTQ_DESC_F_WRITE);
            dma_w16(desc + 14, 0);
            // Two draws.
            let mut idx: u16 = 0;
            let mut n = 0;
            while n < 2 {
                dma_w16(avail + 4, 0); // avail.ring[0] = descriptor 0
                dma_fence();
                idx += 1;
                dma_w16(avail + 2, idx); // avail.idx
                dma_fence();
                mmio_w(mmio, virtio::QUEUE_NOTIFY, 0);
                loop {
                    dma_fence();
                    if dma_r16(used + 2) == idx {
                        break;
                    }
                }
                let w0 = dma_r64(buf) as usize;
                let w1 = dma_r64(buf + 8) as usize;
                let w2 = dma_r64(buf + 16) as usize;
                let w3 = dma_r64(buf + 24) as usize;
                sys_send4(ENTROPY_CAP, w0, w1, w2, w3);
                n += 1;
            }
            sys_exit(0)
        }
    }

    /// Rebuild a 32-byte ML-KEM seed from an IPC message (badge + 3 data words,
    /// each a little-endian `u64`).
    fn seed_from_message(m: &Message) -> [u8; 32] {
        let words = [m.badge as u64, m.data[0] as u64, m.data[1] as u64, m.data[2] as u64];
        let mut out = [0u8; 32];
        for (i, w) in words.iter().enumerate() {
            out[i * 8..i * 8 + 8].copy_from_slice(&w.to_le_bytes());
        }
        out
    }

    /// The entropy consumer (kernel task): seeds the kernel entropy pool from
    /// the virtio-rng component's device draws, proves the pool serves entropy
    /// on demand (one device seed yields a stream) and reseeds with fresh
    /// entropy, then keys the ML-KEM-768 round-trip from a pool draw — so the
    /// post-quantum demo is seeded by the reseedable pool, not a one-shot read.
    /// Then idles cooperatively (kernel tasks never return).
    extern "C" fn pqc_consumer() -> ! {
        // Seed the pool from the first device draw.
        let d1 = sched::recv_message(ENTROPY_CAP);
        entropy::reseed(seed_from_message(&d1));
        println!("entropy: pool seeded from virtio-rng");

        // The pool serves entropy on demand: one device seed yields a stream.
        let a = entropy::next_seed();
        let b = entropy::next_seed();
        if a != b {
            println!("entropy: pool serves on demand (draws differ)");
        } else {
            println!("entropy: WARNING pool draws identical");
        }

        // Fold a second device draw in — reseeding mixes new entropy with state.
        let d2 = sched::recv_message(ENTROPY_CAP);
        entropy::reseed(seed_from_message(&d2));
        println!("entropy: pool reseeded from virtio-rng");

        // Key ML-KEM from a pool draw (not the raw device bytes).
        let seed = entropy::next_seed();
        match kernel_crypto::ml_kem768_agree(seed) {
            Some(_) => println!("pqc: ML-KEM-768 round-trip ok (pool-seeded)"),
            None => println!("pqc: ML-KEM-768 FAIL (secrets disagreed)"),
        }
        loop {
            sched::yield_now();
            // SAFETY: wait for the next interrupt between yields.
            unsafe { core::arch::asm!("wfi") };
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
