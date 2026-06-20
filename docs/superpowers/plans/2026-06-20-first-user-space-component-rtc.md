# First user-space component (RTC time server) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline, per the user's preference for this project) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move a real driver out of the kernel: a U-mode RTC server that exclusively owns the goldfish RTC (MMIO mapped only into its address space) and serves time-reads to other components over capability-checked IPC — the first realization of ADR 0007.

**Architecture:** Compose what we have — `dt` discovers the RTC base; `build_user_space` maps the RTC's MMIO R-U into the server component only (the existing per-component mapping from 3b-ii); the server reads the clock and prints it (3a `print` syscall) on a client's capability-checked request (3b-iii IPC). The kernel never touches the RTC.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU. Host tests: `cargo test -p kernel-arch-riscv64`. Bare: `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/superpowers/specs/2026-06-20-first-user-space-component-rtc-design.md`

**Verified during planning:** the goldfish RTC node is at base **0x101000** in the real QEMU DTB (found by `compatible` containing `goldfish`, same per-node/`END_NODE` path as the UART). The parser already has `read_cells` and the per-node UART buffering (Phase 4b); we add a parallel RTC match.

---

## Task 1: discover the RTC base from the device tree

**Files:**
- Modify: `arch/riscv64/src/dt.rs`

- [ ] **Step 1: Add `rtc_base` to `MachineInfo` and the failing assert**

In `arch/riscv64/src/dt.rs`, extend `MachineInfo`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MachineInfo {
    pub ram_base: usize,
    pub ram_size: usize,
    pub timebase_hz: u64,
    pub uart_base: usize,
    pub uart_reg_shift: u32,
    pub rtc_base: usize,
}
```

Add to the `parses_qemu_virt` test:

```rust
        assert_eq!(mi.rtc_base, 0x10_1000, "rtc base");
```

Run: `cargo test -p kernel-arch-riscv64 parses_qemu_virt`
Expected: FAIL — `MachineInfo` has no `rtc_base` (compile error).

- [ ] **Step 2: Parse the RTC node (goldfish, committed at `END_NODE`)**

In `parse`, add an RTC tentative-state pair alongside the existing UART ones.
Where the UART state is declared:

```rust
    let mut node_is_uart = false;
    let mut node_reg: Option<usize> = None;
    let mut node_shift: u32 = 0;
    let mut uart: Option<(usize, u32)> = None;
```

add:

```rust
    let mut node_is_rtc = false;
    let mut rtc: Option<usize> = None;
```

In the `FDT_BEGIN_NODE` arm, where it resets `node_is_uart`/`node_reg`/
`node_shift`, also reset the RTC flag:

```rust
                node_is_uart = false;
                node_reg = None;
                node_shift = 0;
                node_is_rtc = false;
```

In the `FDT_END_NODE` arm, where it commits the UART, also commit the RTC:

```rust
            FDT_END_NODE => {
                if node_is_uart && uart.is_none() {
                    if let Some(b) = node_reg {
                        uart = Some((b, node_shift));
                    }
                }
                if node_is_rtc && rtc.is_none() {
                    if let Some(b) = node_reg {
                        rtc = Some(b);
                    }
                }
                depth = depth.checked_sub(1)?;
            }
```

In the `FDT_PROP` arm, where it matches the UART `compatible`, add the RTC
match right after:

```rust
                if pname == b"compatible" && val.windows(7).any(|w| w == b"ns16550") {
                    node_is_uart = true;
                }
                if pname == b"compatible" && val.windows(8).any(|w| w == b"goldfish") {
                    node_is_rtc = true;
                }
```

Finally, add `rtc_base` to the returned struct:

```rust
    let (uart_base, uart_reg_shift) = uart?;
    Some(MachineInfo {
        ram_base: ram?.0,
        ram_size: ram?.1,
        timebase_hz: timebase?,
        uart_base,
        uart_reg_shift,
        rtc_base: rtc?,
    })
```

Update the `parse` doc comment to mention it also returns the RTC base
(matched by `compatible` containing `goldfish`).

- [ ] **Step 3: Run the tests**

Run: `cargo test -p kernel-arch-riscv64 dt::`
Expected: PASS — `parses_qemu_virt` (now incl. `rtc_base`), `rejects_bad_magic`,
`rejects_truncated_blob`. Then `cargo test -p kernel-arch-riscv64` → all green;
`cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf` → SUCCESS.

- [ ] **Step 4: Commit**

```bash
git add arch/riscv64/src/dt.rs
git commit -m "feat(dt): discover the goldfish RTC base from the device tree"
```

---

## Task 2: update the smoke test for the RTC component (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Replace the 3b-iii toy-IPC patterns with the RTC component's**

In `tools/test-qemu.ps1`, in the `$mustMatch` array, **remove** these three
3b-iii toy-demo lines:

```powershell
    "ipc: 'server' blocks on recv",
    "sched: task 'server' exited \(code 66\)",
    "sched: task 'client' exited \(code 0\)",
```

and the rogue line:

```powershell
    "sched: task 'rogue' exited \(code 7\)",
```

**Add**, in their place (e.g. just before `"console: ns16550a @ 0x10000000"`):

```powershell
    "ipc: 'rtc' blocks on recv",
    "rtc: 0x[0-9a-f]{16}",
    "ipc: 'rogue' send rejected \(no capability\)",
```

(Keep `hello world`, `trap: breakpoint`, `survived breakpoint`,
`paging: sv39 on`, `wx: rodata write blocked`, `frames: alloc/free ok`,
`pqc: ML-KEM-768 round-trip ok`, `console: ns16550a @ 0x10000000`,
`dt: 192 MiB RAM`, `tick: 2(?!\d)`.)

Update the header comment and PASS message to describe the new milestone:
"the first user-space component (ADR 0007) — an RTC driver running as an
unprivileged component that owns the clock and serves time-reads over a
capability-checked endpoint; a rogue without the capability is refused."

- [ ] **Step 2: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — the kernel still runs the 3b-iii toy demo, so
`ipc: 'rtc' blocks on recv` / `rtc: 0x…` are absent.

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts the RTC user-space component (red)"
```

---

## Task 3: the RTC server component + boot wiring (green)

**Files:**
- Modify: `arch/riscv64/src/mem/mod.rs` (rename `build_user_space` region `data → device`)
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Rename `build_user_space`'s second region to `device`**

In `arch/riscv64/src/mem/mod.rs`, `build_user_space` currently maps a second
R-U region named `data`. Rename it to `device` (a per-component R-U page —
read-only data or a device's MMIO) and update the doc; behavior is unchanged.
The signature and the mapping line become:

```rust
pub fn build_user_space(stack: (usize, usize), device: (usize, usize)) -> usize {
    use paging::{PTE_R, PTE_U, PTE_W, PTE_X};
    // SAFETY: a fresh zeroed root; map_kernel_sections + the user ranges are
    // valid; built on the master satp so the page-table writes land.
    unsafe {
        let root = frame::alloc_zeroed()
            .expect("no frame for user root page table")
            .0 as *mut paging::PageTable;
        map_kernel_sections(root);
        paging::map_range(root, sym!(__user_text_start), sym!(__user_text_end), PTE_R | PTE_X | PTE_U);
        paging::map_range(root, stack.0, stack.1, PTE_R | PTE_W | PTE_U);
        // A per-component R-U page: read-only data, or a device's MMIO that
        // this component exclusively owns (mapped here and nowhere else).
        paging::map_range(root, device.0, device.1, PTE_R | PTE_U);
        paging::make_satp(root as usize)
    }
}
```

Update the doc comment above it accordingly (the second region is now "a
per-component read-only page — data or a device MMIO").

- [ ] **Step 2: Rename the server's stacks and re-add the `print` syscall stub**

In `kernel/src/main.rs`, rename the server's static stacks `KS_SERVER → KS_RTC`
and `US_SERVER → US_RTC` (the `KS_CLIENT`/`US_CLIENT`, `KS_ROGUE`/`US_ROGUE`,
`KS_IDLE` stay). The static block becomes:

```rust
    static mut KS_RTC: KStack = [0; TASK_STACK];
    static mut KS_CLIENT: KStack = [0; TASK_STACK];
    static mut KS_ROGUE: KStack = [0; TASK_STACK];
    static mut KS_IDLE: KStack = [0; TASK_STACK];
```
```rust
    #[link_section = ".user_data"]
    static mut US_RTC: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_CLIENT: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_ROGUE: UStack = UStack([0; USER_STACK_SIZE]);
```

Add a `print` syscall stub (3b-iii removed it; the RTC server needs it) next
to `sys_exit`/`sys_send`/`sys_recv`:

```rust
    /// print syscall (a7 = 1): a0 = ptr, a1 = len. The kernel validates the
    /// pointer (must lie in .user_data) and copies it out under a SUM window.
    ///
    /// # Safety
    /// `ptr`/`len` must describe a buffer in this task's mapped memory.
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
```

- [ ] **Step 3: Replace the task functions (`server_task`/`client_task` → `rtc_server`/`rtc_client`; keep `rogue_task`, `idle`)**

Replace `server_task` and `client_task` with:

```rust
    /// The RTC time server: a user-space driver that exclusively owns the
    /// goldfish real-time clock — its MMIO is mapped R-U into THIS component
    /// only (3b-ii isolation). On each request it reads the clock and prints
    /// it. Loops forever, blocking on recv between requests. The kernel never
    /// touches the RTC.
    ///
    /// `RTC_BASE` is the goldfish-rtc MMIO base. The kernel discovered it from
    /// the device tree and mapped exactly this page into our address space; we
    /// use the QEMU value here. (Handing the base to the component — a virtual
    /// device window, or a Device capability carrying it — is future work.)
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn rtc_server() -> ! {
        const RTC_BASE: usize = 0x10_1000;
        loop {
            // SAFETY: we hold the endpoint cap at EP_CAP; recv blocks for a request.
            let _req = unsafe { sys_recv(EP_CAP) };
            // SAFETY: the goldfish RTC page is mapped R-U in our address space.
            // Reading TIME_LOW (offset 0) latches TIME_HIGH (offset 4).
            let (low, high) = unsafe {
                let lo = core::ptr::read_volatile(RTC_BASE as *const u32) as u64;
                let hi = core::ptr::read_volatile((RTC_BASE + 4) as *const u32) as u64;
                (lo, hi)
            };
            let t = (high << 32) | low;
            // Build "rtc: 0x<16 hex>\n" using only byte immediates and
            // arithmetic — a U-mode task must NOT read a .rodata constant
            // (kernel .rodata is not U-accessible). buf lives on our stack.
            let mut buf = [0u8; 24];
            buf[0] = b'r';
            buf[1] = b't';
            buf[2] = b'c';
            buf[3] = b':';
            buf[4] = b' ';
            buf[5] = b'0';
            buf[6] = b'x';
            let mut i = 0;
            while i < 16 {
                let nib = ((t >> ((15 - i) * 4)) & 0xf) as u8;
                buf[7 + i] = if nib < 10 { b'0' + nib } else { b'a' + (nib - 10) };
                i += 1;
            }
            buf[23] = b'\n';
            // SAFETY: buf is on our stack (in .user_data); the kernel
            // validates the pointer and copies it out under a SUM window.
            unsafe { sys_print(buf.as_ptr(), 24) };
        }
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
```

Leave `rogue_task` and `idle` unchanged.

- [ ] **Step 4: Rewrite the spawn block in `kmain`**

Replace the current 3b-iii spawn block (from the `// Phase 3b-iii:` comment
through the `sched::spawn("idle", ...)` line) with:

```rust
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

        sched::spawn("idle", idle, core::ptr::addr_of!(KS_IDLE) as usize + TASK_STACK);
```

- [ ] **Step 5: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS (no leftover `server_task`/`client_task`/`US_SERVER`/`KS_SERVER` references; `sys_print` resolves).

- [ ] **Step 6: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS` including `ipc: 'rtc' blocks on recv`,
`rtc: 0x<16 hex>` (the live clock the server read), and
`ipc: 'rogue' send rejected (no capability)`. If the `rtc:` value is all
zeros or absent, the RTC mapping/read is wrong; if the boot faults right after
the client sends, the device page isn't mapped into the server. Diagnose;
don't weaken the test.

- [ ] **Step 7: Commit**

```bash
git add arch/riscv64/src/mem/mod.rs kernel/src/main.rs
git commit -m "feat: first user-space component - RTC driver serves the clock over capability-checked IPC"
```

---

## Task 4: docs — learning note, roadmap, glossary; final verification

**Files:**
- Create: `docs/learning/0013-first-user-space-component.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md`

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0013-first-user-space-component.md`:

```markdown
# 0013 — The first user-space component: an RTC driver (ADR 0007)

**One-line:** a real driver now lives *outside* the kernel — an unprivileged
component that owns the clock and serves it over capability-checked IPC.

## What changed
- `dt::parse` also discovers the goldfish RTC base. A new U-mode `rtc_server`
  component has that device's MMIO mapped R-U into *its* address space only
  (the per-component mapping from 3b-ii). It loops: `recv` a request, read the
  clock, `print` the value.
- A `client` (holding the endpoint capability) asks for the time; a `rogue`
  (no capability) is refused. The kernel never touches the RTC.

## The point (ADR 0007)
A driver is just a task whose only authority is (a) a device's MMIO mapped
into its address space and (b) an IPC endpoint to receive requests on.
Isolation means only this component sees the device; capabilities mean only
endpoint-holders can call it. So adding a driver/feature neither grows nor
endangers the trusted core — it shrinks it. This is the substrate the
self-healer (Phase 5) will reuse.

## Built almost entirely by composition
No real new kernel mechanism: 3b-ii isolation maps the device into one
component, 3b-iii capabilities/IPC gate the calls, 3a's `print` shows the
result. The "device capability" is, for now, simply the boot-time mapping.

## Gotcha
A U-mode task must not read a `.rodata` constant (kernel `.rodata` isn't
U-accessible), so the server formats its output with byte immediates +
arithmetic on its own stack, not from a string literal.

## Proof
`ipc: 'rtc' blocks on recv`, then `rtc: 0x<live clock>` printed by the
server, then the `rogue` refused — all in QEMU, no board.
```

- [ ] **Step 2: Update the roadmap**

In `docs/roadmap/roadmap.md`, add a new section after the Phase 4 block and
before `## Phase 5 — Self-healing seed`:

```markdown
## User-space components — realizing ADR 0007

The payoff of the capability microkernel: features and drivers live *outside*
the kernel as capability-holding user-space components
([ADR 0007](../decisions/0007-extensibility-user-space-components.md)). Each
shrinks the trusted core and is bounded by what it was granted. This is also
the substrate the self-healer (Phase 5) runs on.

### First component — RTC time server  *(done — 2026-06-20)*

- **Goal:** move a real driver out of the kernel — a U-mode component that
  owns the goldfish RTC (its MMIO mapped into that component only) and serves
  time-reads over a capability-checked endpoint.
- **You learn:** a driver is an unprivileged task with a device mapping + an
  IPC endpoint; isolation + capabilities bound its authority (see
  [learning note 0013](../learning/0013-first-user-space-component.md)).
- **Done when:** `./tools/test-qemu.ps1` shows the RTC server block on recv,
  then read and print the live clock on a client's capability-checked request,
  with a rogue (no capability) refused. QEMU-only; no board.

(Next candidates: a virtio-rng entropy component — real crypto entropy,
retiring Phase 3c's fixed seed — and call/reply IPC so servers can return
values.)
```

- [ ] **Step 3: Add glossary entries**

In `docs/glossary.md`, add entries (in the file's format) for: **user-space
component / server** (an unprivileged task that provides a service or drives a
device, reached over capability-checked IPC — the unit of extension per
ADR 0007), **device driver (as a component)** (such a component whose
authority is a device's MMIO mapped into its address space, and nowhere
else), and **RTC (real-time clock)** (a hardware clock; here the goldfish RTC,
owned by the first user-space driver component). Reuse existing
capability/IPC/MMIO terms.

- [ ] **Step 4: Update the learning-notes index**

In `docs/learning/README.md`, add:

```markdown
- [0013 — The first user-space component: an RTC driver (ADR 0007)](0013-first-user-space-component.md)
```

- [ ] **Step 5: Final verification (run all, paste results)**

Run: `cargo test -p kernel-arch-riscv64` → expect all green.
Run: `cargo build --workspace` → expect success.
Run (PowerShell): `./tools/check-references.ps1` → expect PASS (fix any of YOUR broken references).
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0013-first-user-space-component.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: first user-space component learning note, roadmap (ADR 0007), glossary"
```

---

## Done-when checklist (maps to spec §1)

- [ ] A user-space driver served a real device over IPC — smoke patterns `ipc: 'rtc' blocks on recv` and `rtc: 0x[0-9a-f]{16}` (the server read the live goldfish clock on a client's capability-checked request).
- [ ] Capability enforcement — `ipc: 'rogue' send rejected (no capability)`.
- [ ] Host test — `parse()` returns `rtc_base = 0x10_1000` (plus RAM/timebase/UART).
- [ ] `check-references` clean; `cargo build --workspace` green; `BOOT TEST PASS`.
```
