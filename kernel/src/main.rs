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
    use kernel_arch_riscv64::{cap::Capability, channel, console, csr, dt, entropy, heal, mem, plic, println, sched, shell, task::Message, timer, trap, virtio};
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
        let rng_base = unsafe { virtio::find_device(&machine.virtio_mmio[..machine.virtio_mmio_count], virtio::DEVICE_ID_RNG) };
        let blk_base = unsafe { virtio::find_device(&machine.virtio_mmio[..machine.virtio_mmio_count], virtio::DEVICE_ID_BLK) };
        let net_base = unsafe { virtio::find_device(&machine.virtio_mmio[..machine.virtio_mmio_count], virtio::DEVICE_ID_NET) };

        mem::init(machine.ram_base + machine.ram_size, machine.uart_base, machine.plic_base);
        // Phase 15: bring up virtio-net and resolve the gateway (10.0.2.2) by
        // ARP — the OS's first network exchange.
        if let Some(net) = net_base {
            unsafe { net_resolve_gateway(net) };
        } else {
            println!("net: no virtio-net device found");
        }
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

        // Phase 8 — capability delegation. `needy` holds NO RTC cap; it obtains
        // Endpoint(EP0) by delegation from `broker` over the grant channel, then
        // calls the RTC server with it. Spawn needy before broker so needy is
        // recv-blocked on the grant channel when broker grants.
        let cu = ustack(core::ptr::addr_of!(US_CLIENT) as usize);
        let needy = sched::spawn_user("needy", needy_task, cu.1,
            core::ptr::addr_of!(KS_CLIENT) as usize + TASK_STACK,
            mem::build_user_space(cu, NO_DEVICE));
        sched::grant_cap(needy, GRANT_CHAN_CAP, Capability::Endpoint(GRANT_EP));

        let bru = ustack(core::ptr::addr_of!(US_BROKER) as usize);
        let broker = sched::spawn_user("broker", broker_task, bru.1,
            core::ptr::addr_of!(KS_BROKER) as usize + TASK_STACK,
            mem::build_user_space(bru, NO_DEVICE));
        sched::grant_cap(broker, GRANT_CHAN_CAP, Capability::Endpoint(GRANT_EP));
        sched::grant_cap(broker, BROKER_RTC_SLOT, Capability::Endpoint(EP0));

        // Phase 13 — capability revocation. The lease server answers the tenant
        // once, then revokes the leased endpoint; the tenant's second call fails.
        // Spawn the server first so it recv-blocks before the tenant calls.
        let lsu = ustack(core::ptr::addr_of!(US_LEASE) as usize);
        let lease = sched::spawn_user("lease", lease_server, lsu.1,
            core::ptr::addr_of!(KS_LEASE) as usize + TASK_STACK,
            mem::build_user_space(lsu, NO_DEVICE));
        sched::grant_cap(lease, LEASE_CAP, Capability::Endpoint(LEASE_EP));

        let tnu = ustack(core::ptr::addr_of!(US_TENANT) as usize);
        let tenant = sched::spawn_user("tenant", tenant_task, tnu.1,
            core::ptr::addr_of!(KS_TENANT) as usize + TASK_STACK,
            mem::build_user_space(tnu, NO_DEVICE));
        sched::grant_cap(tenant, LEASE_CAP, Capability::Endpoint(LEASE_EP));

        // Phase 14 — encrypted IPC channel. Establish the session key here on the
        // large boot stack: ML-KEM keygen is too stack-hungry to run lazily in
        // the seal/open syscall (which executes on a task's 16 KiB trap stack).
        // (Fixed seed for now; pool-seeding the channel key is deferred — it
        // needs a large-stack establishment after the pool is seeded.)
        channel::establish([0x14u8; 32]);
        // The sealer seals a known message and sends the ciphertext to the
        // opener, which verifies it (and that a tampered ciphertext is rejected);
        // `nocap` proves the Session gate. Spawn the opener first so it
        // recv-blocks before the sealer sends.
        let opu = ustack(core::ptr::addr_of!(US_OPENER) as usize);
        let opener = sched::spawn_user("opener", opener_task, opu.1,
            core::ptr::addr_of!(KS_OPENER) as usize + TASK_STACK,
            mem::build_user_space(opu, NO_DEVICE));
        sched::grant_cap(opener, 0, Capability::Session);
        sched::grant_cap(opener, CHAN_CAP, Capability::Endpoint(CHAN_EP));

        let seu = ustack(core::ptr::addr_of!(US_SEALER) as usize);
        let sealer = sched::spawn_user("sealer", sealer_task, seu.1,
            core::ptr::addr_of!(KS_SEALER) as usize + TASK_STACK,
            mem::build_user_space(seu, NO_DEVICE));
        sched::grant_cap(sealer, 0, Capability::Session);
        sched::grant_cap(sealer, CHAN_CAP, Capability::Endpoint(CHAN_EP));

        let ncu = ustack(core::ptr::addr_of!(US_NOCAP) as usize);
        let _nocap = sched::spawn_user("nocap", nocap_task, ncu.1,
            core::ptr::addr_of!(KS_NOCAP) as usize + TASK_STACK,
            mem::build_user_space(ncu, NO_DEVICE));
        // nocap gets NO Session cap — its seal must be refused.

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
        // Phase 6c: the patient blocks on the KB-ready gate on its first run.
        sched::grant_cap(transient, GATE_CAP, Capability::Endpoint(GATE_EP));

        // A patient that always crashes: exercises the retry bound.
        let fu = ustack(core::ptr::addr_of!(US_FLAKY) as usize);
        let flaky = sched::spawn_user("flaky", flaky_task, fu.1,
            core::ptr::addr_of!(KS_FLAKY) as usize + TASK_STACK,
            mem::build_user_space(fu, NO_DEVICE));
        sched::grant_cap(healer, 2, Capability::Restart(flaky));
        sched::set_crash_badge(flaky, 2);
        // Phase 6c: same KB-ready gate (first run only).
        sched::grant_cap(flaky, GATE_CAP, Capability::Endpoint(GATE_EP));

        // Phase 7 — the novel patient: an illegal-instruction crash with no KB
        // entry at first boot. Gated on the KB-ready gate like the others. No
        // Restart cap: it just needs to be contained and diagnosed (None on
        // boot 1 -> recorded; matched on boot 2).
        let xu = ustack(core::ptr::addr_of!(US_NOVEL) as usize);
        let novel = sched::spawn_user("novel", novel_task, xu.1,
            core::ptr::addr_of!(KS_NOVEL) as usize + TASK_STACK,
            mem::build_user_space(xu, NO_DEVICE));
        sched::grant_cap(novel, GATE_CAP, Capability::Endpoint(GATE_EP));

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

            // Interrupt path: route the RNG's IRQ through the PLIC and grant the
            // component the authority to wait on it (cap slot 1).
            let n = machine.virtio_mmio_count;
            let rng_irq = virtio::irq_for_base(&machine.virtio_mmio[..n], &machine.virtio_mmio_irq[..n], rng)
                .expect("rng has no IRQ in the device tree");
            plic::init(machine.plic_base);
            plic::set_priority(rng_irq, 1);
            // Enable the source now and leave it enabled: QEMU's PLIC only
            // asserts SEIP on the rising edge of an enabled source, so the
            // handler/wait_irq use claim's in-service state to mask, never the
            // enable bit.
            plic::enable(rng_irq);
            // SAFETY: the trap handler and the PLIC are now set up to service it.
            unsafe { csr::sie_enable_external() };
            sched::grant_cap(entropy, IRQ_CAP, Capability::Interrupt(rng_irq));
        } else {
            println!("entropy: no virtio-rng device found");
        }

        // A U-mode component that draws from the entropy pool via the
        // capability-gated getrandom syscall (it runs after pqc has seeded the
        // pool). It holds a Randomness capability; a request without one is
        // refused.
        let nu = ustack(core::ptr::addr_of!(US_RNGUSER) as usize);
        let rnguser = sched::spawn_user("rnguser", rnguser_task, nu.1,
            core::ptr::addr_of!(KS_RNGUSER) as usize + TASK_STACK,
            mem::build_user_space(nu, NO_DEVICE));
        sched::grant_cap(rnguser, RNG_CAP, Capability::Randomness);

        // The deferrer demo: a server that holds two calls in flight and replies
        // out of order (proves one-shot reply caps). Spawn the server before its
        // clients so it recv-blocks first.
        let du = ustack(core::ptr::addr_of!(US_DEFERRER) as usize);
        let deferrer = sched::spawn_user("deferrer", deferrer_task, du.1,
            core::ptr::addr_of!(KS_DEFERRER) as usize + TASK_STACK,
            mem::build_user_space(du, NO_DEVICE));
        sched::grant_cap(deferrer, DEFER_CAP, Capability::Endpoint(DEFER_EP));

        let dau = ustack(core::ptr::addr_of!(US_DCLIENTA) as usize);
        let dclient_a = sched::spawn_user("dclientA", dclient_a_task, dau.1,
            core::ptr::addr_of!(KS_DCLIENTA) as usize + TASK_STACK,
            mem::build_user_space(dau, NO_DEVICE));
        sched::grant_cap(dclient_a, DEFER_CAP, Capability::Endpoint(DEFER_EP));

        let dbu = ustack(core::ptr::addr_of!(US_DCLIENTB) as usize);
        let dclient_b = sched::spawn_user("dclientB", dclient_b_task, dbu.1,
            core::ptr::addr_of!(KS_DCLIENTB) as usize + TASK_STACK,
            mem::build_user_space(dbu, NO_DEVICE));
        sched::grant_cap(dclient_b, DEFER_CAP, Capability::Endpoint(DEFER_EP));

        // Phase 6b — a minimal filesystem. The blk driver is now a call/reply
        // server (recv block N -> virtio read into its identity-mapped DMA page
        // -> reply). The kernel `fs` task is the client: it calls the server to
        // read blocks, finds a file by name, and prints its contents off disk.
        // Spawn the server first (lower slot) so it recv-blocks before fs calls.
        // Phase 6c — the KB loader. Spawned unconditionally so it always
        // releases the gated patients; it reads the KB only if a disk backs
        // the FS (KB_HAS_BLK, set in the blk block below).
        let kb_loader = sched::spawn("kb", kb_loader_task,
            core::ptr::addr_of!(KS_FS) as usize + TASK_STACK);
        sched::grant_cap(kb_loader, GATE_LOADER_CAP, Capability::Endpoint(GATE_EP));

        if let Some(blk) = blk_base {
            let dma_pa = mem::frame::alloc_zeroed().expect("no DMA frame for blk").0;
            // SAFETY: set once at boot before the loader runs; single hart.
            unsafe { BLK_DMA_PA = dma_pa; }
            // SAFETY: set once at boot before the loader runs; single hart.
            unsafe { KB_HAS_BLK = true; }

            let bu = ustack(core::ptr::addr_of!(US_BLK) as usize);
            let blkdev = sched::spawn_user("blk", blk_component, bu.1,
                core::ptr::addr_of!(KS_BLK) as usize + TASK_STACK,
                mem::build_virtio_space(bu, (blk, blk + 0x1000), (dma_pa, dma_pa + 0x1000)));
            sched::grant_cap(blkdev, BLK_EP_CAP, Capability::Endpoint(BLK_EP));

            let n = machine.virtio_mmio_count;
            let blk_irq = virtio::irq_for_base(&machine.virtio_mmio[..n], &machine.virtio_mmio_irq[..n], blk)
                .expect("blk has no IRQ in the device tree");
            sched::grant_cap(blkdev, BLK_IRQ_CAP, Capability::Interrupt(blk_irq));
            sched::set_launch_args(blkdev, blk, dma_pa);
            // PLIC setup (idempotent if the rng path already did init + sie).
            plic::init(machine.plic_base);
            plic::set_priority(blk_irq, 1);
            plic::enable(blk_irq);
            // SAFETY: the trap handler and the PLIC are set up to service it.
            unsafe { csr::sie_enable_external() };

            // The KB loader is the FS client: grant it the blk service cap.
            sched::grant_cap(kb_loader, BLK_EP_CAP, Capability::Endpoint(BLK_EP));

            // Phase 7 — the KB-writer: records a novel contained crash to disk.
            let kb_writer = sched::spawn("kbw", kb_writer_task,
                core::ptr::addr_of!(KS_KBW) as usize + TASK_STACK);
            sched::grant_cap(kb_writer, BLK_EP_CAP, Capability::Endpoint(BLK_EP));
        } else {
            println!("blk: no virtio-blk device found");
        }

        // Phase 9 — the diagnosis-aware shell: a kernel task that polls UART RX
        // and queries the self-healing organism (kb / diag). The UART is
        // kernel-owned, so the shell drives the existing console and reads
        // `heal` directly. (It polls rather than using the UART RX interrupt —
        // see `shell::shell_task` and learning note 0020 for why character input
        // does not suit QEMU's edge-delivered PLIC.)
        shell::init(machine.uart_base, machine.uart_reg_shift);
        let _shell = sched::spawn("shell", shell::shell_task,
            core::ptr::addr_of!(KS_SHELL) as usize + TASK_STACK);

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

    /// The cap slot the RTC server lets the kernel mint its reply cap into.
    const RTC_REPLY_SLOT: usize = 1;

    /// The broker→needy delegation channel (Phase 8). Distinct from the RTC
    /// endpoint EP0, whose cap the broker delegates over it.
    const GRANT_EP: usize = 6;
    /// needy/broker cap slots for the grant channel and the delegated cap.
    const GRANT_CHAN_CAP: usize = 0; // Endpoint(GRANT_EP), held by both
    const BROKER_RTC_SLOT: usize = 1; // broker's Endpoint(EP0) to delegate
    const NEEDY_RTC_SLOT: usize = 1; // where needy receives the delegated RTC cap
    const NEEDY_EMPTY_SLOT: usize = 3; // an empty slot the broker's bad grant names

    /// The lease endpoint for the Phase 13 revocation demo (distinct from EP0 and
    /// the grant channel). Both the lease server and the tenant hold a cap to it
    /// at cap slot 0; the server answers one call then revokes it.
    const LEASE_EP: usize = 7;
    const LEASE_CAP: usize = 0; // Endpoint(LEASE_EP), held by server and tenant
    const LEASE_REPLY_SLOT: usize = 1; // the server's reply-cap slot
    /// The tenant's exit code when it used the cap once and its second (revoked)
    /// use was rejected.
    const TENANT_REVOKED_CODE: usize = 13;

    /// The encrypted-channel endpoint (Phase 14): sealer -> opener. The sealer
    /// and opener hold a Session cap at slot 0 and Endpoint(CHAN_EP) at slot 1.
    const CHAN_EP: usize = 8;
    const CHAN_CAP: usize = 1; // Endpoint(CHAN_EP) (Session is slot 0)
    const CHAN_REPLY_SLOT: usize = 2; // opener's recv reply slot (unused for Send)
    /// The known plaintext the sealer encrypts (a recognizable word). Kept ≤ 32
    /// bits so U-mode code materializes it inline (`lui`+`addi`) rather than via a
    /// `ld` from the kernel's `.rodata` constant pool (unmapped in U-mode).
    const CHAN_PLAINTEXT: usize = 0x1234_5678;
    /// opener's exit code when the message verified AND a tamper was rejected.
    const CHAN_OK_CODE: usize = 14;
    /// nocap's exit code when its seal was refused.
    const NOCAP_CODE: usize = 15;

    /// The healer's cap-table slot holding its `Endpoint(CRASH_EP)` capability
    /// (it `recv`s on this to learn of crashes). Restart caps live at slots
    /// 1.. so a crash notification's badge is directly the cap slot to use.
    const CRASH_CAP: usize = 0;

    /// The endpoint the entropy component delivers seeds on, and the cap slot
    /// it (and the consumer) hold it in.
    const ENTROPY_EP: usize = 2;
    const ENTROPY_CAP: usize = 0;

    /// The `rnguser` component's cap slot holding its `Randomness` capability.
    const RNG_CAP: usize = 0;

    /// The entropy component's cap slot holding its `Interrupt(rng_irq)` cap.
    const IRQ_CAP: usize = 1;

    /// The endpoint the deferrer demo uses, and the cap slot the deferrer and
    /// its clients hold it in.
    const DEFER_EP: usize = 3;
    const DEFER_CAP: usize = 0;

    /// The blk service endpoint (the kernel FS calls it; the blk server recvs).
    const BLK_EP: usize = 4;
    /// blk cap slots: 0 = the service endpoint, 1 = its Interrupt cap, 2 = the
    /// one-shot Reply cap the server's recv mints per call.
    const BLK_EP_CAP: usize = 0;
    const BLK_IRQ_CAP: usize = 1;
    const BLK_REPLY_SLOT: usize = 2;
    /// Badge bit that asks the blk server to WRITE block N from the DMA data
    /// page (otherwise it reads into it). Block numbers are small, so the high
    /// bit is free.
    const BLK_WRITE_FLAG: usize = 1 << 31;

    /// The net service endpoint (the kernel `net_resolver` calls it; the U-mode
    /// `net` driver recvs). Mirrors the blk cap layout exactly.
    const NET_EP: usize = 9;
    /// net cap slots: 0 = the service endpoint, 1 = its Interrupt cap, 2 = the
    /// one-shot Reply cap the driver's recv mints per call.
    const NET_EP_CAP: usize = 0;
    const NET_IRQ_CAP: usize = 1;
    const NET_REPLY_SLOT: usize = 2;

    /// Phase 6c — the KB-ready gate. Patients block on this endpoint on their
    /// first run so they cannot crash before the loader builds the on-disk KB
    /// table; the loader releases them once it has.
    const GATE_EP: usize = 5;
    const GATE_CAP: usize = 0; // a patient's gate-endpoint cap slot
    const GATE_REPLY_SLOT: usize = 1; // a patient's reply cap slot
    const GATE_LOADER_CAP: usize = 1; // the loader's gate-endpoint cap slot
    /// Set true at boot if a virtio-blk device backs the FS (the loader needs
    /// it to read the KB; without it the loader just releases the patients).
    static mut KB_HAS_BLK: bool = false;

    /// Physical address of the blk DMA frame (identity-mapped); the FS reads
    /// sector bytes from `BLK_DMA_PA + BLK_DATA_OFF`. Set by `kmain`.
    static mut BLK_DMA_PA: usize = 0;
    /// Physical address of the net DMA frame (identity-mapped); the resolver
    /// builds the ARP request into `NET_DMA_PA + 0xC00 + 12` and reads the reply
    /// from `NET_DMA_PA + 0x400 + 12`. Set by `kmain`.
    static mut NET_DMA_PA: usize = 0;
    /// The block currently resident in the DMA data page (the one-block cache);
    /// `-1` = none. Re-reading the same block skips the IPC round-trip.
    static mut FS_CACHED_BLOCK: i64 = -1;
    /// Kernel buffer a read file is copied into (out of the reused DMA page).
    const FS_FILEBUF_LEN: usize = 4096;
    static mut FS_FILEBUF: [u8; FS_FILEBUF_LEN] = [0; FS_FILEBUF_LEN];

    /// Per-task kernel stack size (also the trap stack a U-mode task's
    /// traps land on). 16 KiB; per-task guard pages stay deferred.
    const TASK_STACK: usize = 16 * 1024;
    type KStack = [u8; TASK_STACK];
    static mut KS_RTC: KStack = [0; TASK_STACK];
    static mut KS_CLIENT: KStack = [0; TASK_STACK];
    static mut KS_BROKER: KStack = [0; TASK_STACK];
    static mut KS_LEASE: KStack = [0; TASK_STACK];
    static mut KS_TENANT: KStack = [0; TASK_STACK];
    static mut KS_OPENER: KStack = [0; TASK_STACK];
    static mut KS_SEALER: KStack = [0; TASK_STACK];
    static mut KS_NOCAP: KStack = [0; TASK_STACK];
    static mut KS_ROGUE: KStack = [0; TASK_STACK];
    static mut KS_HEALER: KStack = [0; TASK_STACK];
    static mut KS_TRANSIENT: KStack = [0; TASK_STACK];
    static mut KS_FLAKY: KStack = [0; TASK_STACK];
    static mut KS_ENTROPY: KStack = [0; TASK_STACK];
    static mut KS_RNGUSER: KStack = [0; TASK_STACK];
    static mut KS_DEFERRER: KStack = [0; TASK_STACK];
    static mut KS_DCLIENTA: KStack = [0; TASK_STACK];
    static mut KS_DCLIENTB: KStack = [0; TASK_STACK];
    static mut KS_BLK: KStack = [0; TASK_STACK];
    static mut KS_NET: KStack = [0; TASK_STACK];
    static mut KS_NETRES: KStack = [0; TASK_STACK];
    static mut KS_FS: KStack = [0; TASK_STACK];
    static mut KS_KBW: KStack = [0; TASK_STACK];
    static mut KS_NOVEL: KStack = [0; TASK_STACK];
    static mut KS_SHELL: KStack = [0; TASK_STACK];
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
    static mut US_BROKER: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_LEASE: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_TENANT: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_OPENER: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_SEALER: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_NOCAP: UStack = UStack([0; USER_STACK_SIZE]);
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
    #[link_section = ".user_data"]
    static mut US_RNGUSER: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_DEFERRER: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_DCLIENTA: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_DCLIENTB: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_BLK: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_NET: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_NOVEL: UStack = UStack([0; USER_STACK_SIZE]);

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

    /// grant syscall (a7 = 11): a0 = endpoint cap to send over, a1 = the
    /// sender's source cap slot to delegate, a2 = badge. Returns a0 = 0 on
    /// success, or `usize::MAX` if the endpoint cap or the source slot is
    /// invalid.
    ///
    /// # Safety
    /// Always sound; the kernel validates both capabilities.
    #[inline(always)]
    unsafe fn sys_grant(ep: usize, src_slot: usize, badge: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 11usize,
            inout("a0") ep => ret,
            in("a1") src_slot,
            in("a2") badge,
            options(nostack),
        );
        ret
    }

    /// revoke syscall (a7 = 12): a0 = the slot of an Endpoint cap the caller
    /// holds. Revokes that endpoint from every other holder. Returns the count
    /// revoked, or `usize::MAX` if the caller lacks the capability.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability.
    #[inline(always)]
    unsafe fn sys_revoke(ep: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 12usize,
            inout("a0") ep => ret,
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

    /// reply syscall (a7 = 8): a0 = reply-cap slot, a1 = badge, a2..a4 = data
    /// (zero here). Wakes the caller named by the one-shot Reply cap and
    /// consumes it. Returns 0, or `usize::MAX` if the slot holds no reply cap.
    ///
    /// # Safety
    /// Always sound; the kernel validates the reply capability.
    #[inline(always)]
    unsafe fn sys_reply(reply_slot: usize, badge: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 8usize,
            inout("a0") reply_slot => ret,
            in("a1") badge,
            in("a2") 0usize,
            in("a3") 0usize,
            options(nostack),
        );
        ret
    }

    /// getrandom syscall (a7 = 9): a0 = cap index of a Randomness capability.
    /// Returns (status, 4 words): status a0 = 0 on success (the 4 words are 32
    /// random bytes) or `usize::MAX` if the caller lacks the capability.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and fills the words.
    #[inline(always)]
    unsafe fn sys_getrandom(cap: usize) -> (usize, [usize; 4]) {
        let status;
        let w0;
        let w1;
        let w2;
        let w3;
        core::arch::asm!(
            "ecall",
            in("a7") 9usize,
            inout("a0") cap => status,
            out("a1") w0,
            out("a2") w1,
            out("a3") w2,
            out("a4") w3,
            options(nostack),
        );
        (status, [w0, w1, w2, w3])
    }

    /// wait_irq syscall (a7 = 10): a0 = cap index of an Interrupt capability.
    /// Blocks until the device interrupt fires; returns a0 = 0, or `usize::MAX`
    /// if the caller lacks the capability.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and blocks us.
    #[inline(always)]
    unsafe fn sys_wait_irq(cap: usize) -> usize {
        let ret;
        core::arch::asm!(
            "ecall",
            in("a7") 10usize,
            inout("a0") cap => ret,
            options(nostack),
        );
        ret
    }

    /// recv syscall (a7 = 5): a0 = endpoint cap index, a1 = reply slot (where
    /// the kernel installs a one-shot Reply cap if the message is a Call; for a
    /// one-way Send it is unused). Returns the badge in a0.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and may block us.
    #[inline(always)]
    unsafe fn sys_recv(cap: usize, reply_slot: usize) -> usize {
        let badge;
        core::arch::asm!(
            "ecall",
            in("a7") 5usize,
            inout("a0") cap => badge,
            inout("a1") reply_slot => _,
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

    /// recv syscall capturing the 3 data words (a7 = 5): a0 = endpoint cap, a1 =
    /// reply slot. Returns (badge, w0, w1, w2) — the message's badge + 3 data
    /// words (the kernel delivers data into a1..a3).
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and may block us.
    #[inline(always)]
    unsafe fn sys_recv4(cap: usize, reply_slot: usize) -> (usize, usize, usize, usize) {
        let badge;
        let w0;
        let w1;
        let w2;
        core::arch::asm!(
            "ecall",
            in("a7") 5usize,
            inout("a0") cap => badge,
            inout("a1") reply_slot => w0,
            out("a2") w1,
            out("a3") w2,
            options(nostack),
        );
        (badge, w0, w1, w2)
    }

    /// seal syscall (a7 = 13): a0 = plaintext word. Returns (status, ciphertext,
    /// tag_lo, tag_hi, nonce); status `usize::MAX` if the caller lacks Session.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability.
    #[inline(always)]
    unsafe fn sys_seal(plain: usize) -> (usize, usize, usize, usize, usize) {
        let status;
        let ct;
        let tl;
        let th;
        let nonce;
        core::arch::asm!(
            "ecall",
            in("a7") 13usize,
            inout("a0") plain => status,
            out("a1") ct,
            out("a2") tl,
            out("a3") th,
            out("a4") nonce,
            options(nostack),
        );
        (status, ct, tl, th, nonce)
    }

    /// open syscall (a7 = 14): a0 = ciphertext, a1 = tag_lo, a2 = tag_hi, a3 =
    /// nonce. Returns (status, plaintext); status `usize::MAX` on a bad tag/no cap.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and the tag.
    #[inline(always)]
    unsafe fn sys_open(ct: usize, tag_lo: usize, tag_hi: usize, nonce: usize) -> (usize, usize) {
        let status;
        let plain;
        core::arch::asm!(
            "ecall",
            in("a7") 14usize,
            inout("a0") ct => status,
            inout("a1") tag_lo => plain,
            in("a2") tag_hi,
            in("a3") nonce,
            options(nostack),
        );
        (status, plain)
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
    unsafe fn dma_fence() {
        core::arch::asm!("fence", options(nostack));
    }
    #[inline(always)]
    unsafe fn dma_w8(addr: usize, v: u8) {
        core::arch::asm!("sb {v}, 0({a})", v = in(reg) v, a = in(reg) addr, options(nostack));
    }
    #[inline(always)]
    unsafe fn dma_r8(addr: usize) -> u8 {
        let v;
        core::arch::asm!("lbu {v}, 0({a})", v = out(reg) v, a = in(reg) addr, options(nostack));
        v
    }

    /// The virtio-mmio v2 status handshake + queue-0 setup (identical for every
    /// modern virtio device). `dma` is the identity-mapped frame holding the
    /// rings. Spike-verified against virtio-rng and virtio-blk.
    #[inline(always)]
    unsafe fn virtio_queue_init(mmio: usize, dma: usize) {
        let desc = dma + virtio::VQ_DESC_OFF;
        let avail = dma + virtio::VQ_AVAIL_OFF;
        let used = dma + virtio::VQ_USED_OFF;
        mmio_w(mmio, virtio::STATUS, 0);
        mmio_w(mmio, virtio::STATUS, virtio::STATUS_ACK);
        mmio_w(mmio, virtio::STATUS, virtio::STATUS_ACK | virtio::STATUS_DRIVER);
        mmio_w(mmio, virtio::DEVICE_FEATURES_SEL, 1);
        let fhi = mmio_r(mmio, virtio::DEVICE_FEATURES);
        mmio_w(mmio, virtio::DRIVER_FEATURES_SEL, 1);
        mmio_w(mmio, virtio::DRIVER_FEATURES, fhi & virtio::F_VERSION_1_HI);
        mmio_w(mmio, virtio::DRIVER_FEATURES_SEL, 0);
        mmio_w(mmio, virtio::DRIVER_FEATURES, 0);
        mmio_w(mmio, virtio::STATUS,
            virtio::STATUS_ACK | virtio::STATUS_DRIVER | virtio::STATUS_FEATURES_OK);
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
    }

    /// Phase 15 — the OS's first network exchange: bring up virtio-net (the
    /// modern two-queue handshake), transmit an ARP request for the gateway
    /// (`10.0.2.2`), and parse the reply (its MAC) from the RX queue. The frame
    /// format is the host-tested `kernel_common::net` (ARP) logic. Polls the used
    /// ring (the reply arrives within a few microseconds on QEMU SLIRP).
    ///
    /// This driver currently runs in the kernel (it uses the pure `arp` logic
    /// directly); moving it to an unprivileged user-space component like the
    /// rng/blk drivers (ADR 0007) is a deferred refinement.
    unsafe fn net_resolve_gateway(mmio: usize) {
        use kernel_common::net;
        mem::map_device(mmio);
        let dma = mem::frame::alloc_zeroed().expect("no DMA frame for net").0;
        let rx_desc = dma + 0x000;
        let rx_avail = dma + 0x080;
        let rx_used = dma + 0x100;
        let tx_desc = dma + 0x200;
        let tx_avail = dma + 0x280;
        let tx_used = dma + 0x300;
        let _ = tx_used;
        let rx_buf = dma + 0x400;
        let tx_buf = dma + 0xC00;

        // --- modern virtio-mmio bring-up, two queues (0=RX, 1=TX) ---
        mmio_w(mmio, virtio::STATUS, 0);
        mmio_w(mmio, virtio::STATUS, virtio::STATUS_ACK);
        mmio_w(mmio, virtio::STATUS, virtio::STATUS_ACK | virtio::STATUS_DRIVER);
        mmio_w(mmio, virtio::DEVICE_FEATURES_SEL, 1);
        let fhi = mmio_r(mmio, virtio::DEVICE_FEATURES);
        mmio_w(mmio, virtio::DRIVER_FEATURES_SEL, 1);
        mmio_w(mmio, virtio::DRIVER_FEATURES, fhi & virtio::F_VERSION_1_HI);
        mmio_w(mmio, virtio::DRIVER_FEATURES_SEL, 0);
        mmio_w(mmio, virtio::DRIVER_FEATURES, 0);
        mmio_w(mmio, virtio::STATUS, virtio::STATUS_ACK | virtio::STATUS_DRIVER | virtio::STATUS_FEATURES_OK);
        for (q, d, a, u) in [(0u32, rx_desc, rx_avail, rx_used), (1, tx_desc, tx_avail, tx_used)] {
            mmio_w(mmio, virtio::QUEUE_SEL, q);
            mmio_w(mmio, virtio::QUEUE_NUM, virtio::VQ_SIZE);
            mmio_w(mmio, virtio::QUEUE_DESC_LOW, d as u32);
            mmio_w(mmio, virtio::QUEUE_DESC_HIGH, (d >> 32) as u32);
            mmio_w(mmio, virtio::QUEUE_DRIVER_LOW, a as u32);
            mmio_w(mmio, virtio::QUEUE_DRIVER_HIGH, (a >> 32) as u32);
            mmio_w(mmio, virtio::QUEUE_DEVICE_LOW, u as u32);
            mmio_w(mmio, virtio::QUEUE_DEVICE_HIGH, (u >> 32) as u32);
            mmio_w(mmio, virtio::QUEUE_READY, 1);
        }
        mmio_w(mmio, virtio::STATUS, virtio::STATUS_ACK | virtio::STATUS_DRIVER | virtio::STATUS_FEATURES_OK | virtio::STATUS_DRIVER_OK);

        // --- post one RX buffer (device-writable) ---
        dma_w64(rx_desc, rx_buf as u64);
        dma_w32(rx_desc + 8, 2048);
        dma_w16(rx_desc + 12, virtio::VIRTQ_DESC_F_WRITE);
        dma_w16(rx_desc + 14, 0);
        dma_w16(rx_avail + 4, 0); // ring[0] -> desc 0
        dma_fence();
        dma_w16(rx_avail + 2, 1); // avail.idx
        dma_fence();

        // --- build the ARP request after a 12-byte (zeroed) virtio_net_hdr ---
        let src_mac = [0x52u8, 0x54, 0x00, 0x12, 0x34, 0x56];
        let txf = core::slice::from_raw_parts_mut((tx_buf + 12) as *mut u8, 64);
        let len = net::build_request(&src_mac, [10, 0, 2, 15], [10, 0, 2, 2], txf);
        dma_w64(tx_desc, tx_buf as u64);
        dma_w32(tx_desc + 8, (12 + len) as u32);
        dma_w16(tx_desc + 12, 0); // device reads
        dma_w16(tx_desc + 14, 0);
        dma_w16(tx_avail + 4, 0);
        dma_fence();
        dma_w16(tx_avail + 2, 1);
        dma_fence();

        // --- notify TX, poll RX used ring for the reply ---
        mmio_w(mmio, virtio::QUEUE_NOTIFY, 1);
        let mut got = false;
        for _ in 0..5_000_000u64 {
            let is = mmio_r(mmio, virtio::INTERRUPT_STATUS);
            if is != 0 {
                mmio_w(mmio, virtio::INTERRUPT_ACK, is);
            }
            let rx_idx = core::ptr::read_volatile((rx_used + 2) as *const u16);
            if rx_idx != 0 {
                let rxf = core::slice::from_raw_parts((rx_buf + 12) as *const u8, 64);
                if let Some(mac) = net::parse_reply(rxf, [10, 0, 2, 2]) {
                    println!(
                        "net: resolved 10.0.2.2 -> {:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}",
                        mac[0], mac[1], mac[2], mac[3], mac[4], mac[5]
                    );
                    got = true;
                    break;
                }
            }
        }
        if !got {
            println!("net: spike got no ARP reply");
        }
    }

    /// Issue one virtio-blk request for `sector` (`write` = T_OUT else T_IN),
    /// publishing the 3-descriptor chain and blocking on the device IRQ.
    /// Returns the device status byte (0 = OK). `avail_idx` is the current
    /// available-ring index.
    #[inline(always)]
    unsafe fn blk_req(mmio: usize, dma: usize, write: bool, avail_idx: u16, sector: u64) -> u8 {
        let desc = dma + virtio::VQ_DESC_OFF;
        let avail = dma + virtio::VQ_AVAIL_OFF;
        let hdr = dma + virtio::BLK_HDR_OFF;
        let data = dma + virtio::BLK_DATA_OFF;
        let status = dma + virtio::BLK_STATUS_OFF;
        // request header
        dma_w32(hdr, if write { virtio::BLK_T_OUT } else { virtio::BLK_T_IN });
        dma_w32(hdr + 4, 0);
        dma_w64(hdr + 8, sector);
        dma_w8(status, 0xff); // sentinel
        // desc 0: header (device reads)
        dma_w64(desc, hdr as u64);
        dma_w32(desc + 8, 16);
        dma_w16(desc + 12, virtio::VIRTQ_DESC_F_NEXT);
        dma_w16(desc + 14, 1);
        // desc 1: data (device WRITEs it on a read)
        dma_w64(desc + 16, data as u64);
        dma_w32(desc + 24, virtio::BLK_SECTOR_SIZE as u32);
        let data_flags = virtio::VIRTQ_DESC_F_NEXT
            | if write { 0 } else { virtio::VIRTQ_DESC_F_WRITE };
        dma_w16(desc + 28, data_flags);
        dma_w16(desc + 30, 2);
        // desc 2: status (device WRITEs)
        dma_w64(desc + 32, status as u64);
        dma_w32(desc + 40, 1);
        dma_w16(desc + 44, virtio::VIRTQ_DESC_F_WRITE);
        dma_w16(desc + 46, 0);
        // publish the head descriptor, notify, and wait for the interrupt
        dma_w16(avail + 4 + (avail_idx as usize % virtio::VQ_SIZE as usize) * 2, 0);
        dma_fence();
        dma_w16(avail + 2, avail_idx + 1);
        dma_fence();
        mmio_w(mmio, virtio::QUEUE_NOTIFY, 0);
        sys_wait_irq(BLK_IRQ_CAP);
        let is = mmio_r(mmio, virtio::INTERRUPT_STATUS);
        mmio_w(mmio, virtio::INTERRUPT_ACK, is);
        dma_r8(status)
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
            let _req = unsafe { sys_recv(EP_CAP, RTC_REPLY_SLOT) };
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
            unsafe { sys_reply(RTC_REPLY_SLOT, t) };
        }
    }

    /// `needy` (Phase 8): an RTC client that holds NO RTC capability. It blocks
    /// receiving on the grant channel (naming NEEDY_RTC_SLOT as where the kernel
    /// installs the delegated cap), then `call`s the RTC server on that
    /// now-delegated capability and exits with the live clock — proof that
    /// authority reached it only by runtime delegation.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn needy_task() -> ! {
        // SAFETY: we hold the grant-channel cap; recv blocks until the broker
        // delegates the RTC endpoint cap into NEEDY_RTC_SLOT (badge discarded),
        // then we call the RTC server on it.
        unsafe {
            let _ = sys_recv(GRANT_CHAN_CAP, NEEDY_RTC_SLOT);
            let t = sys_call(NEEDY_RTC_SLOT, 1);
            sys_exit(t)
        }
    }

    /// `broker` (Phase 8): holds the RTC endpoint cap (BROKER_RTC_SLOT) and the
    /// grant-channel cap (GRANT_CHAN_CAP). It first attempts a bad grant (an
    /// empty source slot → rejected, proving the unforgeability guard), then
    /// delegates the RTC endpoint cap to whoever is waiting on the grant channel
    /// (needy), and exits.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn broker_task() -> ! {
        // SAFETY: grant is always sound; the kernel validates both caps. The
        // first call delegates a slot we don't hold (rejected); the second
        // delegates our real RTC endpoint cap to the recv-blocked needy.
        unsafe {
            let _ = sys_grant(GRANT_CHAN_CAP, NEEDY_EMPTY_SLOT, 0);
            let _ = sys_grant(GRANT_CHAN_CAP, BROKER_RTC_SLOT, 1);
            sys_exit(0)
        }
    }

    /// `lease_server` (Phase 13): holds Endpoint(LEASE_EP) for recv + revoke
    /// authority. It answers the tenant's first call, then REVOKES LEASE_EP —
    /// clearing the tenant's leased cap while keeping its own — and exits.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn lease_server() -> ! {
        // SAFETY: we hold Endpoint(LEASE_EP); recv blocks for the tenant's call,
        // we reply, then revoke the endpoint from every other holder.
        unsafe {
            let _ = sys_recv(LEASE_CAP, LEASE_REPLY_SLOT); // tenant's call 1
            sys_reply(LEASE_REPLY_SLOT, 1);
            let _ = sys_revoke(LEASE_CAP);
            sys_exit(0)
        }
    }

    /// `tenant` (Phase 13): holds Endpoint(LEASE_EP). It calls the lease server
    /// twice; the first succeeds, the second fails because its cap was revoked in
    /// between. Exits with TENANT_REVOKED_CODE iff "call 1 ok, call 2 revoked".
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn tenant_task() -> ! {
        // SAFETY: call sends a request and blocks for the reply; on a revoked
        // cap the kernel returns usize::MAX.
        unsafe {
            let r1 = sys_call(LEASE_CAP, 1);
            let r2 = sys_call(LEASE_CAP, 2);
            if r1 != usize::MAX && r2 == usize::MAX {
                sys_exit(TENANT_REVOKED_CODE)
            } else {
                sys_exit(99)
            }
        }
    }

    /// `sealer` (Phase 14): holds a Session cap (slot 0) + Endpoint(CHAN_EP). It
    /// seals a known plaintext and sends {nonce, ciphertext, tag} to the opener.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn sealer_task() -> ! {
        // SAFETY: seal is Session-gated (we hold the cap); send4 carries the
        // nonce as the badge and the ciphertext+tag as the 3 data words.
        unsafe {
            let (_s, ct, tl, th, nonce) = sys_seal(CHAN_PLAINTEXT);
            let _ = sys_send4(CHAN_CAP, nonce, ct, tl, th);
            sys_exit(0)
        }
    }

    /// `opener` (Phase 14): holds a Session cap (slot 0) + Endpoint(CHAN_EP). It
    /// receives the sealed message, opens it (verifying the plaintext), then
    /// confirms a flipped ciphertext is rejected. Exits CHAN_OK_CODE iff both.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn opener_task() -> ! {
        // SAFETY: recv blocks for the sealer's message; open is Session-gated.
        unsafe {
            let (nonce, ct, tl, th) = sys_recv4(CHAN_CAP, CHAN_REPLY_SLOT);
            let (s, plain) = sys_open(ct, tl, th, nonce);
            let verified = s == 0 && plain == CHAN_PLAINTEXT;
            let (s2, _) = sys_open(ct ^ 1, tl, th, nonce); // tampered ciphertext
            let tamper_rejected = s2 == usize::MAX;
            if verified && tamper_rejected {
                sys_exit(CHAN_OK_CODE)
            } else {
                sys_exit(99)
            }
        }
    }

    /// `nocap` (Phase 14): holds NO Session cap — its seal is refused, proving
    /// the capability gate on the encrypted channel.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn nocap_task() -> ! {
        // SAFETY: seal is always sound; here it returns usize::MAX because we
        // hold no Session capability.
        unsafe {
            let (s, ..) = sys_seal(CHAN_PLAINTEXT);
            if s == usize::MAX {
                sys_exit(NOCAP_CODE)
            } else {
                sys_exit(98)
            }
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
        let generation: usize;
        // SAFETY: read the launch generation the kernel placed in a0.
        unsafe {
            core::arch::asm!("mv {g}, a0", g = out(reg) generation, options(nomem, nostack, preserves_flags));
        }
        if generation == 0 {
            // Phase 6c gate (first run only): block until the KB is loaded.
            // SAFETY: we hold Endpoint(GATE_EP) at GATE_CAP; recv then reply.
            unsafe {
                let _ = sys_recv(GATE_CAP, GATE_REPLY_SLOT);
                sys_reply(GATE_REPLY_SLOT, 0);
            }
        }
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
                // Block until the device interrupts (the kernel wakes us), then
                // ack the device to deassert its interrupt line. The device has
                // advanced the used ring by the time we wake.
                sys_wait_irq(IRQ_CAP);
                let status = mmio_r(mmio, virtio::INTERRUPT_STATUS);
                mmio_w(mmio, virtio::INTERRUPT_ACK, status);
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

    /// A U-mode component that draws from the kernel entropy pool via the
    /// capability-gated `getrandom` syscall. It proves both gating outcomes:
    /// a request with no capability (cap slot 99) is refused, and requests with
    /// its granted `Randomness` capability are served. It exits 0 iff the
    /// refusal and the two served draws behaved as expected. (The pool's
    /// liveness — distinct draws — is proven separately by the `pqc` demo; we
    /// don't re-check it here, to avoid coupling to scheduler ordering.)
    /// Register-only (no `.rodata`/buffer) — codegen-safe in U-mode.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn rnguser_task() -> ! {
        // SAFETY: getrandom is always sound; the kernel checks the capability.
        unsafe {
            let (bad, _) = sys_getrandom(99); // no capability at slot 99 -> refused
            let (ok1, _) = sys_getrandom(RNG_CAP); // served
            let (ok2, _) = sys_getrandom(RNG_CAP); // served again
            let good = bad == usize::MAX && ok1 == 0 && ok2 == 0;
            sys_exit(if good { 0 } else { 7 })
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
            let cap_idx = unsafe { sys_recv(CRASH_CAP, 0) };
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
            // Phase 6c gate: wait until the KB loader has built the on-disk
            // table so our crash is diagnosed against the real KB. First run
            // only — a restart (generation > 0) skips this.
            // SAFETY: we hold Endpoint(GATE_EP) at GATE_CAP; recv then reply.
            unsafe {
                let _ = sys_recv(GATE_CAP, GATE_REPLY_SLOT);
                sys_reply(GATE_REPLY_SLOT, 0);
            }
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

    /// A server that holds two calls in flight and replies OUT OF ORDER, proving
    /// one-shot reply capabilities. It receives a call into reply slot 1 and a
    /// second into reply slot 2 (holding a Reply cap for each), then replies to
    /// the second before the first. Each reply returns `request | 0x100`.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn deferrer_task() -> ! {
        // SAFETY: we hold the endpoint cap at DEFER_CAP; recv blocks for a call.
        unsafe {
            let a = sys_recv(DEFER_CAP, 1); // call A -> Reply cap in slot 1
            let b = sys_recv(DEFER_CAP, 2); // call B -> Reply cap in slot 2
            sys_reply(2, b | 0x100); // reply B first
            sys_reply(1, a | 0x100); // then A
            sys_exit(0)
        }
    }

    /// A client of the deferrer: call with badge 0xa1, exit with the reply
    /// (0x1a1 = 417) — proving its call was tracked independently of B's.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn dclient_a_task() -> ! {
        // SAFETY: we hold the endpoint cap at DEFER_CAP.
        unsafe { sys_exit(sys_call(DEFER_CAP, 0xa1)) }
    }

    /// A client of the deferrer: call with badge 0xb1, exit with the reply
    /// (0x1b1 = 433).
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn dclient_b_task() -> ! {
        // SAFETY: we hold the endpoint cap at DEFER_CAP.
        unsafe { sys_exit(sys_call(DEFER_CAP, 0xb1)) }
    }

    /// The blk component (user-space virtio-blk driver), now a call/reply
    /// **server**. The kernel maps the device's MMIO + a zeroed DMA frame into
    /// it (a1 = mmio, a2 = dma) and grants an Interrupt cap for `wait_irq`. It
    /// loops: receive a block number (the call badge), read that sector into the
    /// identity-mapped DMA data page, and reply with the device status byte
    /// (0 = OK). The kernel FS reads the data straight from the DMA page.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn blk_component() -> ! {
        let mmio: usize;
        let dma: usize;
        // SAFETY: read the launch args the kernel placed in a1/a2.
        unsafe {
            core::arch::asm!("mv {m}, a1", "mv {d}, a2",
                m = out(reg) mmio, d = out(reg) dma,
                options(nomem, nostack, preserves_flags));
        }
        // SAFETY: mmio + dma are mapped RW-U into this task; the sequence is the
        // spike-verified virtio-blk bring-up, then a recv/read/reply loop.
        unsafe {
            virtio_queue_init(mmio, dma);
            let mut avail_idx: u16 = 0;
            loop {
                // badge = block #, with BLK_WRITE_FLAG set for a write.
                let badge = sys_recv(BLK_EP_CAP, BLK_REPLY_SLOT);
                let write = badge & BLK_WRITE_FLAG != 0;
                let block = (badge & !BLK_WRITE_FLAG) as u64;
                let status = blk_req(mmio, dma, write, avail_idx, block);
                avail_idx = avail_idx.wrapping_add(1);
                sys_reply(BLK_REPLY_SLOT, status as usize);
            }
        }
    }

    /// The FS↔device boundary: read block `n` via the blk server into the
    /// identity-mapped DMA data page and return a view of it. `None` on a device
    /// I/O error. A trivial one-block cache skips the IPC if `n` is resident.
    fn fs_read_block(n: u32) -> Option<&'static [u8]> {
        // SAFETY: BLK_DMA_PA is the kernel-allocated, identity-mapped DMA frame;
        // the slice addresses real RAM. Single hart; the cache state is ours.
        unsafe {
            if FS_CACHED_BLOCK != n as i64 {
                let status = sched::call_message(BLK_EP_CAP, n as usize);
                if status != 0 {
                    FS_CACHED_BLOCK = -1;
                    return None;
                }
                FS_CACHED_BLOCK = n as i64;
            }
            Some(core::slice::from_raw_parts(
                (BLK_DMA_PA + virtio::BLK_DATA_OFF) as *const u8,
                virtio::BLK_SECTOR_SIZE,
            ))
        }
    }

    /// Write `bytes` (≤ one block; zero-padded) into block `n` via the blk
    /// server. Fills the shared DMA data page, then asks the server to write it.
    /// Returns false on a device error. Invalidates the one-block read cache,
    /// since the DMA page now holds `n`'s outgoing data.
    fn fs_write_block(n: u32, bytes: &[u8]) -> bool {
        // SAFETY: BLK_DMA_PA is the kernel-allocated, identity-mapped DMA frame;
        // single hart owns the page for the duration of this write.
        unsafe {
            let data = (BLK_DMA_PA + virtio::BLK_DATA_OFF) as *mut u8;
            let take = core::cmp::min(bytes.len(), virtio::BLK_SECTOR_SIZE);
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), data, take);
            for i in take..virtio::BLK_SECTOR_SIZE {
                core::ptr::write(data.add(i), 0);
            }
            FS_CACHED_BLOCK = -1; // DMA page no longer holds a cached read
            let status = sched::call_message(BLK_EP_CAP, (n as usize) | BLK_WRITE_FLAG);
            status == 0
        }
    }

    /// Append a file named `name` with `contents` to the on-disk volume. Reads
    /// the superblock + directory, plans the append (`fs::append_plan`), then
    /// writes data block(s) → directory → superblock LAST. The superblock write
    /// is the single commit point: a crash before it leaves the new blocks
    /// unreferenced (invisible), so existing data is never corrupted. Returns
    /// false if the plan is refused or any write errors (aborting before the
    /// commit). `contents` must fit in one block (a KB entry's frontmatter does).
    fn fs_append_file(name: &str, contents: &[u8]) -> Option<u32> {
        use kernel_common::fs;
        if contents.len() > fs::BLOCK_SIZE {
            return None;
        }
        // Copy the superblock and directory out of the reused DMA page before
        // any write overwrites it.
        let mut sb_buf = [0u8; fs::BLOCK_SIZE];
        match fs_read_block(0) {
            Some(b) => sb_buf.copy_from_slice(b),
            None => return None,
        }
        let sb = fs::Superblock::decode(&sb_buf)?;
        let mut dir_buf = [0u8; fs::BLOCK_SIZE];
        match fs_read_block(sb.dir_block) {
            Some(b) => dir_buf.copy_from_slice(b),
            None => return None,
        }
        let plan = fs::append_plan(&sb, &dir_buf, name, contents.len() as u32)?;
        // 1. data, 2. directory, 3. superblock (commit) — in that order.
        if !fs_write_block(plan.start_block, contents) {
            return None;
        }
        if !fs_write_block(sb.dir_block, &plan.dir_block) {
            return None;
        }
        let mut new_sb = [0u8; fs::BLOCK_SIZE];
        plan.new_superblock.encode(&mut new_sb);
        if fs_write_block(0, &new_sb) {
            Some(plan.start_block)
        } else {
            None
        }
    }

    /// Number of leading blocks of a KB entry the loader reads — enough to
    /// cover the YAML frontmatter (id/title/match-cause/first-playbook), which
    /// by convention sits at the very top of the file. Each block costs one
    /// blk-server round-trip (one device-IRQ wait), so we read only the head
    /// rather than the whole multi-block entry.
    const KB_HEAD_BLOCKS: usize = 2;

    /// Read the first `KB_HEAD_BLOCKS` blocks of a file extent (its frontmatter
    /// head) into `FS_FILEBUF` and return them. The directory entry is already
    /// in hand, so this skips the superblock/directory re-read `fs_read_file`
    /// would do per call.
    fn fs_read_extent_head(start_block: u32, byte_len: u32) -> Option<&'static [u8]> {
        use kernel_common::fs;
        let nblocks = (fs::block_count(byte_len) as usize).min(KB_HEAD_BLOCKS);
        let len = (byte_len as usize)
            .min(nblocks * fs::BLOCK_SIZE)
            .min(FS_FILEBUF_LEN);
        // SAFETY: single hart; FS_FILEBUF is ours for the duration of this read.
        unsafe {
            for i in 0..nblocks {
                let blk = fs_read_block(start_block + i as u32)?;
                let off = i * fs::BLOCK_SIZE;
                let take = core::cmp::min(fs::BLOCK_SIZE, len - off);
                FS_FILEBUF[off..off + take].copy_from_slice(&blk[..take]);
            }
            Some(&FS_FILEBUF[..len])
        }
    }

    /// The KB loader (Phase 6c): enumerate the on-disk directory, read and
    /// parse each `knowledge-base/entries/*.md`, and install the tokened ones
    /// into the self-healer's runtime table — so a later contained crash is
    /// diagnosed against the real, on-disk knowledge base. Then release the
    /// gated patients and idle.
    extern "C" fn kb_loader_task() -> ! {
        use kernel_common::{fs, kb};
        let mut scanned = 0usize;
        let mut loaded = 0usize;
        // SAFETY: single hart; KB_HAS_BLK set once at boot.
        let has_blk = unsafe { core::ptr::read(core::ptr::addr_of!(KB_HAS_BLK)) };
        if has_blk {
            'load: {
                let sb = match fs_read_block(0).and_then(fs::Superblock::decode) {
                    Some(sb) => sb,
                    None => {
                        println!("kb: bad superblock; KB not loaded");
                        break 'load;
                    }
                };
                let mut dir = [0u8; fs::BLOCK_SIZE];
                match fs_read_block(sb.dir_block) {
                    Some(b) => dir.copy_from_slice(b),
                    None => {
                        println!("kb: directory read failed; KB not loaded");
                        break 'load;
                    }
                }
                for i in 0..sb.dir_entries as usize {
                    let ent = match fs::dir_entry_at(&dir, i) {
                        Some(e) => e,
                        None => break,
                    };
                    scanned += 1;
                    // Read just this entry's frontmatter head (its extent is
                    // known from `ent`), keeping the boot-time read count small.
                    let bytes = match fs_read_extent_head(ent.start_block, ent.byte_len) {
                        Some(b) => b,
                        None => continue,
                    };
                    if let Some(rec) = kb::parse(bytes) {
                        if rec.match_cause.is_some()
                            && heal::install(rec.id, rec.title, rec.playbook, rec.match_cause, rec.seen, rec.escalated, ent.start_block)
                        {
                            loaded += 1;
                        }
                    }
                }
            }
        } else {
            println!("kb: no disk; KB not loaded");
        }
        println!(
            "heal: loaded {} KB entr{} from disk (scanned {})",
            loaded,
            if loaded == 1 { "y" } else { "ies" },
            scanned
        );
        // Release the gated patients now that the table is built
        // (transient, flaky, novel).
        sched::call_message(GATE_LOADER_CAP, 0);
        sched::call_message(GATE_LOADER_CAP, 0);
        sched::call_message(GATE_LOADER_CAP, 0);
        loop {
            sched::yield_now();
            // SAFETY: wait for the next interrupt between yields.
            unsafe { core::arch::asm!("wfi") };
        }
    }

    /// Default playbook the organism records for any newly-seen contained crash
    /// — the same caged, bounded restart KB-0005 prescribes.
    const DEFAULT_PLAYBOOK: &str = "Restart the component, up to a bounded number of retries.";

    /// The KB-writer task (Phase 7): drains the novel-cause mailbox the crash
    /// path fills, mints a KB entry for the unrecognized token, appends it to
    /// disk (`fs_append_file`), and installs it into the runtime table — so a
    /// later boot of the same image diagnoses the formerly-novel crash. Runs
    /// with interrupts on (I/O is forbidden in the crash path). Polls in a
    /// yield loop, like `idle`/the loader.
    extern "C" fn kb_writer_task() -> ! {
        use kernel_common::kb;
        loop {
            if let Some(token) = heal::take_novel() {
                // Mint the next id: "KB-NNNN" after the largest installed.
                let num = heal::max_kb_number() + 1;
                let mut id = [0u8; 8]; // "KB-NNNN"
                id[0] = b'K'; id[1] = b'B'; id[2] = b'-';
                id[3] = b'0' + ((num / 1000) % 10) as u8;
                id[4] = b'0' + ((num / 100) % 10) as u8;
                id[5] = b'0' + ((num / 10) % 10) as u8;
                id[6] = b'0' + (num % 10) as u8;
                let id = core::str::from_utf8(&id[..7]).unwrap_or("KB-0000");

                // Title: "Observed fault: <token> (auto-recorded)".
                let mut title = [0u8; 96];
                let mut tn = 0usize;
                for s in ["Observed fault: ", token, " (auto-recorded)"] {
                    let b = s.as_bytes();
                    let take = core::cmp::min(b.len(), title.len() - tn);
                    title[tn..tn + take].copy_from_slice(&b[..take]);
                    tn += take;
                }
                let title = core::str::from_utf8(&title[..tn]).unwrap_or("Observed fault");

                let mut doc = [0u8; kernel_common::fs::BLOCK_SIZE];
                if let Some(len) = kb::serialize(id, title, DEFAULT_PLAYBOOK, token, &mut doc) {
                    if let Some(start) = fs_append_file(id, &doc[..len]) {
                        // Install it now too, so a re-crash this boot matches.
                        heal::install(id, title, DEFAULT_PLAYBOOK, Some(token), 0, false, start);
                        println!("heal: recorded {id} ({token}) to disk");
                    } else {
                        println!("heal: could not record {id} ({token}) to disk");
                    }
                }
            }
            // Phase 10/11 — persist any entry whose seen-counter (and possibly
            // escalation) changed, in place.
            while let Some((id, start_block, seen, escalated)) = heal::dirty_entry() {
                let mut block = [0u8; kernel_common::fs::BLOCK_SIZE];
                match fs_read_block(start_block) {
                    Some(b) => block.copy_from_slice(b),
                    None => continue,
                }
                // `&` (not `&&`): write both fixed-width fields regardless.
                let ok = kb::set_seen_in_block(&mut block, seen)
                    & kb::set_escalated_in_block(&mut block, escalated);
                if ok && fs_write_block(start_block, &block) {
                    println!(
                        "heal: persisted {id} (seen {seen}{})",
                        if escalated { ", escalated" } else { "" }
                    );
                }
            }
            sched::yield_now();
            // SAFETY: wait for the next interrupt between polls.
            unsafe { core::arch::asm!("wfi") };
        }
    }

    /// The novel patient (Phase 7): a U-mode component that executes an illegal
    /// instruction (`unimp`) — a contained crash whose token has no KB entry at
    /// first boot. Gated on the KB-ready gate (first run) like the other
    /// patients, so it crashes only after the loader has built the table.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn novel_task() -> ! {
        let generation: usize;
        // SAFETY: read the launch generation the kernel placed in a0.
        unsafe {
            core::arch::asm!("mv {g}, a0", g = out(reg) generation, options(nomem, nostack, preserves_flags));
        }
        if generation == 0 {
            // SAFETY: we hold Endpoint(GATE_EP) at GATE_CAP; recv then reply.
            unsafe {
                let _ = sys_recv(GATE_CAP, GATE_REPLY_SLOT);
                sys_reply(GATE_REPLY_SLOT, 0);
            }
        }
        // SAFETY: `unimp` is the canonical illegal instruction; the U-mode trap
        // (scause 2) is contained by the kernel. Control never returns.
        unsafe {
            core::arch::asm!("unimp", options(nostack, noreturn));
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
