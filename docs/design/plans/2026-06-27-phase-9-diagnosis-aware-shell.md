# Phase 9 — Diagnosis-aware interactive shell — Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** A kernel-task console you type at — interrupt-driven UART input + a minimal line discipline + commands that query the self-healing organism (`kb`, `diag`).

**Architecture:** New UART receive path (RBR + IER) whose IRQ (discovered from the device tree, routed through the PLIC) wakes a kernel shell task blocking on a kernel-side `wait_irq` counterpart. The shell echoes via the existing console, buffers a line, and dispatches commands against `heal`'s runtime KB table. Pure `LineBuffer` is host-tested; the device/IRQ glue is proven by an interactive boot.

**Tech Stack:** Rust `no_std` kernel (`arch/riscv64`, `kernel`), QEMU riscv64, PowerShell boot harness driving `-serial stdio` via .NET `Process`.

**Spec:** `docs/design/specs/2026-06-27-phase-9-diagnosis-aware-shell-design.md`

## Global Constraints

- **Commits:** Conventional Commits, NO AI co-author trailer; author Kathir (signing automated).
- **Pure logic host-tested:** `cargo test -p kernel-arch-riscv64`. **Kernel build:** `./tools/build.ps1`. **Boot test:** `./tools/test-qemu.ps1`.
- The shell is a **kernel task** (the UART is kernel-owned). It must not run with interrupts off during I/O — it blocks on the UART IRQ between keystrokes.
- **PLIC masking discipline** (from the PLIC phase): the external handler claims (masks in-service) but does not complete; `wait_irq`/`wait_irq_for` completes (re-arms). The source stays enabled. The harness sends input only after `shell: ready`, so no keystroke predates enablement (avoids the QEMU rising-edge-SEIP gotcha).

---

## File Structure

- `arch/riscv64/src/shell.rs` — **new**: pure `LineBuffer` (+ tests) and the `shell_task` loop.
- `arch/riscv64/src/lib.rs` — **add** `pub mod shell;`.
- `arch/riscv64/src/uart.rs` — **add** `get` (RBR read) and `enable_rx_interrupt` (IER).
- `arch/riscv64/src/dt.rs` — **add** `MachineInfo.uart_irq` discovery.
- `arch/riscv64/src/sched.rs` — **add** `wait_irq_for` (kernel IRQ-wait counterpart).
- `arch/riscv64/src/heal.rs` — **add** `entry(i)`, `last_diagnosis()`, `note_diagnosis`; set last-diagnosis from the crash path.
- `kernel/src/main.rs` — **boot wiring**: `shell::init`, route the UART IRQ through the PLIC, grant the shell its Interrupt cap, spawn the shell; bump `MAX_TASKS` if needed.
- `tools/test-qemu.ps1` — **add** a third interactive boot.
- `docs/learning/0027-interactive-shell.md`, `docs/learning/README.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md` — docs.

---

## Task 1: `shell::LineBuffer` — the pure line discipline

**Files:**
- Create: `arch/riscv64/src/shell.rs`
- Modify: `arch/riscv64/src/lib.rs` (`pub mod shell;`)

**Interfaces:**
- Produces:
  - `pub enum LineEvent { None, Echo(u8), Backspace, Line }`
  - `pub struct LineBuffer { … }` with `pub const fn new()`, `pub fn push(&mut self, byte: u8) -> LineEvent`, `pub fn take(&mut self) -> &str`.

- [ ] **Step 1: Register the module** — add to `arch/riscv64/src/lib.rs` after `pub mod uart;`:

```rust
pub mod shell;
```

- [ ] **Step 2: Write the failing tests** — create `arch/riscv64/src/shell.rs` with the tests first:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn line_of(bytes: &[u8]) -> String {
        let mut lb = LineBuffer::new();
        let mut completed = false;
        for &b in bytes {
            if matches!(lb.push(b), LineEvent::Line) {
                completed = true;
                break;
            }
        }
        assert!(completed, "expected a completed line");
        lb.take().to_string()
    }

    #[test]
    fn appends_printable_and_completes_on_cr() {
        assert_eq!(line_of(b"kb\r"), "kb");
    }

    #[test]
    fn completes_on_lf_too() {
        assert_eq!(line_of(b"diag\n"), "diag");
    }

    #[test]
    fn backspace_removes_last_byte() {
        assert_eq!(line_of(b"kbx\x08\r"), "kb");
    }

    #[test]
    fn backspace_on_empty_is_a_noop() {
        let mut lb = LineBuffer::new();
        assert!(matches!(lb.push(0x08), LineEvent::None));
        assert_eq!(line_of(b"hi\r"), "hi");
    }

    #[test]
    fn push_reports_echo_and_backspace_events() {
        let mut lb = LineBuffer::new();
        assert!(matches!(lb.push(b'a'), LineEvent::Echo(b'a')));
        assert!(matches!(lb.push(0x7f), LineEvent::Backspace));
        assert!(matches!(lb.push(b'\r'), LineEvent::Line));
    }

    #[test]
    fn take_resets_for_the_next_line() {
        let mut lb = LineBuffer::new();
        for &b in b"one\r" { let _ = lb.push(b); }
        assert_eq!(lb.take(), "one");
        for &b in b"two\r" { let _ = lb.push(b); }
        assert_eq!(lb.take(), "two");
    }

    #[test]
    fn caps_at_capacity_then_still_completes() {
        let mut lb = LineBuffer::new();
        for _ in 0..200 { let _ = lb.push(b'z'); }
        assert!(matches!(lb.push(b'\r'), LineEvent::Line));
        assert_eq!(lb.take().len(), CAP);
    }
}
```

- [ ] **Step 3: Run to verify they fail**

Run: `cargo test -p kernel-arch-riscv64 --lib shell`
Expected: FAIL — `LineBuffer` not found.

- [ ] **Step 4: Implement** — at the top of `arch/riscv64/src/shell.rs`:

```rust
//! The diagnosis-aware shell (Phase 9): interrupt-driven UART input feeds a
//! line discipline whose completed commands query the self-healing organism.
//! `LineBuffer` is pure (host-tested); the device/IRQ loop is `shell_task`.

/// Maximum bytes held in one input line (excess printable bytes are dropped
/// until the line completes).
pub const CAP: usize = 64;

/// What a pushed byte did, so the caller can echo appropriately.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEvent {
    /// Consumed without visible change (e.g. backspace on an empty line, or a
    /// printable byte past the capacity).
    None,
    /// A printable byte was appended; echo it.
    Echo(u8),
    /// The last byte was removed; echo a backspace-erase.
    Backspace,
    /// Enter was pressed; the line is complete — call `take`.
    Line,
}

/// A fixed-capacity line buffer with echo + backspace + Enter handling.
pub struct LineBuffer {
    buf: [u8; CAP],
    len: usize,
}

impl LineBuffer {
    pub const fn new() -> Self {
        LineBuffer { buf: [0; CAP], len: 0 }
    }

    /// Feed one received byte; returns what happened so the caller can echo.
    pub fn push(&mut self, byte: u8) -> LineEvent {
        match byte {
            b'\r' | b'\n' => LineEvent::Line,
            0x08 | 0x7f => {
                if self.len > 0 {
                    self.len -= 1;
                    LineEvent::Backspace
                } else {
                    LineEvent::None
                }
            }
            b' '..=b'~' => {
                if self.len < CAP {
                    self.buf[self.len] = byte;
                    self.len += 1;
                    LineEvent::Echo(byte)
                } else {
                    LineEvent::None
                }
            }
            _ => LineEvent::None, // ignore other control bytes
        }
    }

    /// The completed line as a `&str`, and reset for the next line.
    pub fn take(&mut self) -> &str {
        let s = core::str::from_utf8(&self.buf[..self.len]).unwrap_or("");
        // NB: caller must use the returned &str before the next push; we reset
        // len so the borrow covers the current bytes.
        self.len = 0;
        s
    }
}

impl Default for LineBuffer {
    fn default() -> Self {
        Self::new()
    }
}
```

Note: `take` returns a `&str` borrowing `self.buf` while setting `self.len = 0`; the bytes remain valid until overwritten by the next `push`, so the caller dispatches the command before reading more input. (The kernel `shell_task` copies/dispatches immediately.)

- [ ] **Step 5: Run to verify they pass**

Run: `cargo test -p kernel-arch-riscv64 --lib shell`
Expected: PASS (7 tests).

- [ ] **Step 6: Commit**

```bash
git add arch/riscv64/src/shell.rs arch/riscv64/src/lib.rs
git commit -m "feat(shell): pure LineBuffer line discipline (Phase 9)"
```

---

## Task 2: UART receive — `get` + `enable_rx_interrupt`

**Files:**
- Modify: `arch/riscv64/src/uart.rs`

**Interfaces:**
- Produces: `pub unsafe fn get(base, reg_shift) -> Option<u8>`; `pub unsafe fn enable_rx_interrupt(base, reg_shift)`.

- [ ] **Step 1: Implement** (append to `arch/riscv64/src/uart.rs`)

```rust
/// LSR "data ready" bit — a received byte is waiting in the RBR.
const LSR_DR: u8 = 0x01;
/// IER "received-data-available interrupt enable" bit.
const IER_RDA: u8 = 0x01;

/// Read one received byte from the ns16550 at `base` (registers spaced by
/// `reg_shift`) iff the line status register reports data ready; `None`
/// otherwise. Reading the receive holding register (RBR, offset 0) deasserts
/// the device's RX interrupt.
///
/// # Safety
/// `base` must be the MMIO base of an ns16550 UART mapped readable/writable in
/// the current address space.
#[cfg(target_arch = "riscv64")]
pub unsafe fn get(base: usize, reg_shift: u32) -> Option<u8> {
    let lsr = (base + (5usize << reg_shift)) as *const u8;
    let rbr = base as *const u8;
    // SAFETY: caller guarantees a mapped ns16550 register window.
    unsafe {
        if core::ptr::read_volatile(lsr) & LSR_DR == 0 {
            return None;
        }
        Some(core::ptr::read_volatile(rbr))
    }
}

/// Enable the received-data-available interrupt (IER bit 0, offset `1 << shift`).
/// OpenSBI already configured the line; we only turn on RX interrupts.
///
/// # Safety
/// As [`get`]: `base` must be a mapped ns16550 register window.
#[cfg(target_arch = "riscv64")]
pub unsafe fn enable_rx_interrupt(base: usize, reg_shift: u32) {
    let ier = (base + (1usize << reg_shift)) as *mut u8;
    // SAFETY: caller guarantees a mapped ns16550 register window.
    unsafe {
        core::ptr::write_volatile(ier, IER_RDA);
    }
}
```

- [ ] **Step 2: Build**

Run: `./tools/build.ps1`
Expected: clean build (unused until wired by Task 6; `pub` items are not dead-code-flagged).

- [ ] **Step 3: Commit**

```bash
git add arch/riscv64/src/uart.rs
git commit -m "feat(uart): receive path — get (RBR) + enable_rx_interrupt (IER)"
```

---

## Task 3: Device-tree discovery of the UART IRQ

**Files:**
- Modify: `arch/riscv64/src/dt.rs` (struct field, capture, test)

**Interfaces:**
- Produces: `MachineInfo.uart_irq: u32`.

- [ ] **Step 1: Write the failing test** — extend the existing DTB-parsing test (the one that asserts `uart_base`/`plic_base`, around line 245) with:

```rust
        assert_eq!(mi.uart_irq, 10, "uart irq (QEMU virt ns16550 = 10)");
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 --lib dt`
Expected: FAIL — no `uart_irq` field.

- [ ] **Step 3: Implement**

Add the field to `struct MachineInfo` (near `uart_base`):

```rust
    pub uart_irq: u32,
```

In `parse`, capture it. Add a local alongside `uart` (near line 88):

```rust
    let mut uart_irq: u32 = 0;
```

In the `FDT_END_NODE` arm's UART branch (around line 120), record the node's
`interrupts` value (already captured generically into `node_virtio_irq`):

```rust
                if node_is_uart && uart.is_none() {
                    if let Some(b) = node_reg {
                        uart = Some((b, node_shift));
                        uart_irq = node_virtio_irq.unwrap_or(0);
                    }
                }
```

Add it to the returned `MachineInfo` (near `uart_reg_shift`):

```rust
        uart_irq,
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 --lib dt`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/dt.rs
git commit -m "feat(dt): discover the UART IRQ from the device tree"
```

---

## Task 4: `heal` query accessors + last-diagnosis

**Files:**
- Modify: `arch/riscv64/src/heal.rs` (accessors, `LAST_DIAGNOSIS`, `note_diagnosis`)
- Modify: `arch/riscv64/src/sched.rs` (record the diagnosis in the crash path)
- Test: `arch/riscv64/src/heal.rs` tests.

**Interfaces:**
- Produces:
  - `pub fn entry(i: usize) -> Option<(&'static str, &'static str)>` — `(id, title)`
  - `pub fn last_diagnosis() -> Option<(&'static str, &'static str)>` — `(id, playbook)`
  - `pub fn note_diagnosis(issue: &KnownIssue)`

- [ ] **Step 1: Write the failing test** (in `heal.rs` tests)

```rust
    #[test]
    fn entry_lists_installed_id_and_title() {
        assert!(entry(0).is_none(), "empty table");
        assert!(install("KB-0005", "fatal fault", "Restart", Some("page-fault")));
        assert_eq!(entry(0), Some(("KB-0005", "fatal fault")));
        assert!(entry(1).is_none());
    }
```

(`last_diagnosis`/`note_diagnosis` touch a separate `static` and are exercised by the boot test, to avoid cross-test global coupling like `max_kb_number`'s test note.)

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 --lib entry_lists`
Expected: FAIL — no `entry` function.

- [ ] **Step 3: Implement** (in `heal.rs`)

Add a `LAST_DIAGNOSIS` static near `KB_TABLE`:

```rust
/// The most recent contained-crash diagnosis, for the shell's `diag` command.
/// Set from the crash path (single hart).
static mut LAST_DIAGNOSIS: Option<KnownIssue> = None;
```

Add the accessors (near `loaded_count`):

```rust
/// The `i`-th installed KB entry's `(id, title)`, for the shell's `kb` command.
pub fn entry(i: usize) -> Option<(&'static str, &'static str)> {
    // SAFETY: single hart; the table is boot-populated then read-only.
    let table = unsafe { &*core::ptr::addr_of!(KB_TABLE) };
    table.get(i).and_then(|slot| slot.as_ref()).map(|issue| (issue.id(), issue.title()))
}

/// Record the most recent diagnosis (called from the crash path).
pub fn note_diagnosis(issue: &KnownIssue) {
    // SAFETY: single hart; crash path is not re-entrant.
    unsafe { core::ptr::write(core::ptr::addr_of_mut!(LAST_DIAGNOSIS), Some(*issue)); }
}

/// The most recent diagnosis as `(id, playbook)`, for the shell's `diag` command.
pub fn last_diagnosis() -> Option<(&'static str, &'static str)> {
    // SAFETY: single hart; read of a boot/crash-populated cell.
    let last = unsafe { &*core::ptr::addr_of!(LAST_DIAGNOSIS) };
    last.as_ref().map(|issue| (issue.id(), issue.playbook()))
}
```

Note: `KnownIssue` is `Copy` (it derives `Clone, Copy`), so `*issue` stores a copy; `id()/title()/playbook()` borrow from the static, giving `&'static`.

In `arch/riscv64/src/sched.rs`, in `exit_current`'s `Some(issue)` diagnosis arm
(where it prints `heal: diagnosed …`), record it — add after the println:

```rust
                    Some(issue) => {
                        crate::println!(
                            "heal: diagnosed {} ({}) -> playbook: {}",
                            issue.id(), issue.title(), issue.playbook()
                        );
                        crate::heal::note_diagnosis(issue);
                    }
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 --lib entry_lists`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/heal.rs arch/riscv64/src/sched.rs
git commit -m "feat(heal): kb-list + last-diagnosis accessors for the shell"
```

---

## Task 5: `sched::wait_irq_for` — kernel-task IRQ wait

**Files:**
- Modify: `arch/riscv64/src/sched.rs`

**Interfaces:**
- Produces: `pub fn wait_irq_for(cap_idx: usize) -> bool` — block on the IRQ named by an `Interrupt` cap; `false` if the cap is missing.

- [ ] **Step 1: Implement** (in `sched.rs`, near `wait_irq` / `recv_message`)

```rust
/// Block a **kernel** (S-mode) task on the device interrupt named by the
/// `Interrupt` capability at `cap_idx` — the kernel-task counterpart of the
/// `wait_irq` syscall (cf. `recv_message`/`call_message`). Completes the
/// previous PLIC claim (re-arming the source), parks `WaitingIrq`, and returns
/// when the external-interrupt handler wakes the task. `false` if the caller
/// lacks the capability.
#[cfg(target_arch = "riscv64")]
pub fn wait_irq_for(cap_idx: usize) -> bool {
    let irq = SCHED.with(|s| {
        crate::cap::interrupt_irq(&s.tasks[s.current].as_ref().unwrap().caps, cap_idx)
    });
    match irq {
        None => false,
        Some(irq) => {
            crate::plic::complete(irq);
            park_current(TaskState::WaitingIrq(irq));
            true
        }
    }
}
```

- [ ] **Step 2: Build**

Run: `./tools/build.ps1`
Expected: clean build (`pub`, used by Task 6).

- [ ] **Step 3: Commit**

```bash
git add arch/riscv64/src/sched.rs
git commit -m "feat(sched): wait_irq_for — kernel-task IRQ wait counterpart"
```

---

## Task 6: The shell task + boot wiring

**Files:**
- Modify: `arch/riscv64/src/shell.rs` (the `shell_task` loop + `init`)
- Modify: `kernel/src/main.rs` (PLIC route + grant + spawn; `MAX_TASKS` if needed)
- Test: interactive boot (Task 7).

**Interfaces:**
- Consumes: `uart::{get, enable_rx_interrupt}`, `sched::wait_irq_for`, `heal::{entry, last_diagnosis}`, `plic`, the discovered `uart_irq`.
- Produces: `pub fn init(base, reg_shift)`; `pub extern "C" fn shell_task() -> !`.

- [ ] **Step 1: Add `init` + `shell_task` to `arch/riscv64/src/shell.rs`**

```rust
use core::sync::atomic::{AtomicUsize, Ordering};

/// The discovered UART MMIO base / register shift, stored at boot for the shell
/// task (which is spawned with no arguments).
static UART_BASE: AtomicUsize = AtomicUsize::new(0);
static UART_SHIFT: AtomicUsize = AtomicUsize::new(0);

/// The cap slot the shell holds its `Interrupt(uart_irq)` capability in.
const SHELL_IRQ_CAP: usize = 0;

/// Record the UART location for the shell task. Called from `kmain`.
pub fn init(base: usize, reg_shift: u32) {
    UART_BASE.store(base, Ordering::Relaxed);
    UART_SHIFT.store(reg_shift as usize, Ordering::Relaxed);
}

fn prompt() {
    crate::print!("> ");
}

/// Run one completed command, printing its result.
fn dispatch(cmd: &str) {
    match cmd {
        "" => {}
        "help" => crate::println!("commands: help, kb, diag"),
        "kb" => {
            let mut i = 0;
            while let Some((id, title)) = crate::heal::entry(i) {
                crate::println!("{id}  {title}");
                i += 1;
            }
            if i == 0 {
                crate::println!("(knowledge base empty)");
            }
        }
        "diag" => match crate::heal::last_diagnosis() {
            Some((id, playbook)) => crate::println!("last: {id} -> {playbook}"),
            None => crate::println!("none yet"),
        },
        other => crate::println!("unknown command '{other}' (try 'help')"),
    }
}

/// The shell task: enable UART RX, announce readiness, then loop blocking on the
/// UART IRQ, draining received bytes through the line discipline, and
/// dispatching each completed command against the organism.
pub extern "C" fn shell_task() -> ! {
    let base = UART_BASE.load(Ordering::Relaxed);
    let shift = UART_SHIFT.load(Ordering::Relaxed) as u32;
    // SAFETY: `base` is the kernel-owned ns16550, mapped in every address space.
    unsafe { crate::uart::enable_rx_interrupt(base, shift) };
    let mut line = LineBuffer::new();
    crate::println!("shell: ready (type 'help')");
    prompt();
    loop {
        // Block until a keystroke's IRQ wakes us (re-arms the PLIC source).
        if !crate::sched::wait_irq_for(SHELL_IRQ_CAP) {
            // No IRQ cap — should not happen; avoid a busy loop.
            crate::sched::yield_now();
            continue;
        }
        // Drain everything the UART has buffered.
        // SAFETY: kernel-owned ns16550 register window.
        while let Some(byte) = unsafe { crate::uart::get(base, shift) } {
            match line.push(byte) {
                LineEvent::Echo(b) => crate::print!("{}", b as char),
                LineEvent::Backspace => crate::print!("\x08 \x08"),
                LineEvent::Line => {
                    crate::println!();
                    let cmd = line.take();
                    dispatch(cmd);
                    prompt();
                }
                LineEvent::None => {}
            }
        }
    }
}
```

(Confirm `print!` is exported by the crate's macros as `println!` is — if only `println!` exists, add a `print!` macro or use `println!` without newline equivalents. Check `console.rs`/`lib.rs` macro exports before relying on `print!`.)

- [ ] **Step 2: Wire the boot** in `kmain` (`kernel/src/main.rs`). After the console is switched to the UART and the PLIC is initialized (the blk/entropy block already calls `plic::init`), add the shell wiring. Place it near where `blk`/`entropy` IRQs are routed, using the discovered `machine.uart_irq`:

```rust
        // Phase 9 — the diagnosis-aware shell: a kernel task driven by UART RX
        // interrupts that queries the self-healing organism.
        shell::init(machine.uart_base, machine.uart_reg_shift);
        let shell = sched::spawn("shell", shell::shell_task,
            core::ptr::addr_of!(KS_SHELL) as usize + TASK_STACK);
        plic::init(machine.plic_base); // idempotent
        plic::set_priority(machine.uart_irq, 1);
        plic::enable(machine.uart_irq);
        // SAFETY: the trap handler + PLIC service external interrupts.
        unsafe { csr::sie_enable_external() };
        sched::grant_cap(shell, 0, Capability::Interrupt(machine.uart_irq));
```

(`shell::shell_task` is `extern "C" fn() -> !`, matching `spawn`'s signature. The grant slot `0` matches `SHELL_IRQ_CAP`.)

- [ ] **Step 3: Declare the shell's kernel stack** (with the other `KS_*` declarations)

```rust
    static mut KS_SHELL: KStack = [0; TASK_STACK];
```

- [ ] **Step 4: Build**

Run: `./tools/build.ps1`
Expected: clean build. If `print!` is undefined, define it next to `println!` in `console.rs` (mirror `println!` without the trailing newline) and rebuild.

- [ ] **Step 5: Smoke-run the existing test (non-interactive)** to confirm the shell spawns without breaking the existing boots and announces readiness.

Run: `./tools/test-qemu.ps1`
Expected: still PASS (the shell just blocks on RX; existing assertions unaffected). If it panics `scheduler full`, raise `MAX_TASKS` in `sched.rs` by 1 (and add one `None` to the `Scheduler::new` initializer), as prior phases did, then re-run.

- [ ] **Step 6: Commit**

```bash
git add arch/riscv64/src/shell.rs kernel/src/main.rs arch/riscv64/src/sched.rs
git commit -m "feat(shell): interrupt-driven kernel shell task + boot wiring"
```

---

## Task 7: Interactive boot in `test-qemu.ps1`

**Files:**
- Modify: `tools/test-qemu.ps1`

**Interfaces:**
- Consumes the markers: `shell: ready`, and the `kb`/`diag` responses.

- [ ] **Step 1: Add an interactive-boot helper** (after `Invoke-Boot`, near the top of the run section). It drives `-serial stdio` via .NET `Process`, captures stdout with an `OutputDataReceived` event, waits for `shell: ready`, sends commands, and returns the captured transcript. (Mechanism validated by spike.)

```powershell
function Invoke-ShellBoot([string]$diskImg, [string]$kernelElf) {
    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = "qemu-system-riscv64"
    $psi.Arguments = "-machine virt -m 192M -global virtio-mmio.force-legacy=false " +
        "-device virtio-rng-device -drive file=$diskImg,if=none,format=raw,id=blk0 " +
        "-device virtio-blk-device,drive=blk0 -display none -serial stdio -bios default -kernel $kernelElf"
    $psi.RedirectStandardInput = $true
    $psi.RedirectStandardOutput = $true
    $psi.UseShellExecute = $false
    $p = New-Object System.Diagnostics.Process
    $p.StartInfo = $psi
    $global:shellCap = New-Object System.Text.StringBuilder
    $sub = Register-ObjectEvent -InputObject $p -EventName OutputDataReceived -Action {
        if ($EventArgs.Data) { [void]$global:shellCap.AppendLine($EventArgs.Data) }
    }
    try {
        [void]$p.Start(); $p.BeginOutputReadLine()
        # Wait until the shell is ready AND a crash has been diagnosed (so kb/diag
        # are populated), then send the commands.
        $ready = $false
        $deadline = (Get-Date).AddSeconds(40)
        while ((Get-Date) -lt $deadline) {
            Start-Sleep -Milliseconds 300
            $t = $global:shellCap.ToString()
            if (-not $ready -and $t -match "shell: ready" -and $t -match "heal: diagnosed KB-0005") {
                $p.StandardInput.Write("kb`r"); $p.StandardInput.Flush()
                Start-Sleep -Milliseconds 1500
                $p.StandardInput.Write("diag`r"); $p.StandardInput.Flush()
                $ready = $true
                $sendAt = Get-Date
            }
            if ($ready -and ((Get-Date) - $sendAt).TotalSeconds -gt 5) { break }
        }
    }
    finally {
        if (-not $p.HasExited) { $p.Kill() }
        Start-Sleep -Milliseconds 300
        Unregister-Event -SubscriptionId $sub.Id
    }
    return $global:shellCap.ToString()
}
```

- [ ] **Step 2: Run the interactive boot and assert** the shell responded to the typed commands. Add after the boot-2 block (before the combined pass/fail):

```powershell
# Boot 3: interactive — drive the shell over -serial stdio, type `kb` and
# `diag`, and assert the organism answered. Uses the boot-1 image (rebuilt
# fresh below so KB-0005 is present); does not need to persist.
& $mkfs $diskImg | Out-Null
$shellOut = Invoke-ShellBoot $diskImg $kernelElf
$shellMissing = @(
    "shell: ready",
    "KB-0005  User-space component terminated by a fatal fault",
    "last: KB-0005 -> Restart the component, up to a bounded number of retries"
) | Where-Object { $shellOut -notmatch $_ }
```

- [ ] **Step 3: Fold boot 3 into the pass/fail** — extend the final condition to require `$shellMissing.Count -eq 0`, and on failure print the missing shell patterns plus (a tail of) `$shellOut`. Append a Phase 9 clause to the PASS banner: `; and Phase 9 the diagnosis-aware shell: typing 'kb' and 'diag' over the interrupt-driven UART console lists the loaded knowledge base and the last diagnosis - the organism is interrogable.`

- [ ] **Step 4: Run the full boot test**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS: …; and Phase 9 the diagnosis-aware shell: …`.

Debugging aids if the shell doesn't respond:
- If `shell: ready` never appears → the shell task isn't running (check spawn / `MAX_TASKS`).
- If `shell: ready` appears but `kb`/`diag` produce nothing → the RX interrupt path: verify `plic::enable(uart_irq)` ran, `enable_rx_interrupt` was called, and the keystroke arrives after `shell: ready` (it does — we gate on the marker). Confirm `uart_irq` discovered as 10 (the dt test asserts this).
- If echo appears but no command output → the line isn't completing; `\r` vs `\n` (the buffer accepts both).

- [ ] **Step 5: Commit**

```bash
git add tools/test-qemu.ps1
git commit -m "test: interactive boot drives the shell (kb/diag) over UART RX (Phase 9)"
```

---

## Task 8: Documentation

**Files:**
- Create: `docs/learning/0027-interactive-shell.md`
- Modify: `docs/learning/README.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md`

- [ ] **Step 1: Learning note** `docs/learning/0027-interactive-shell.md` — short (memory: learning-notes-minimal). Cover: what changed (UART RX + a kernel shell task that queries the organism); the idea worth keeping (interrupt-driven input mirrors the device-completion IRQ pattern — claim-masks/complete-re-arms — but the "ack" is reading RBR; and the shell makes the self-healing KB *interrogable*, principle #5); the kernel-task trade-off (the console is the one device that stays kernel-owned); the test (real keystrokes over `-serial stdio`, gated on `shell: ready`). Follow `0020-plic-interrupts.md` / `0026` in style.

- [ ] **Step 2: Index** it in `docs/learning/README.md` (add the `0027` line).

- [ ] **Step 3: Roadmap** — replace `## Phase 9+ — Breadth` with a completed `## Phase 9 — Diagnosis-aware interactive shell (done — 2026-06-27)` (goal / you-learn / done-when citing learning note 0027), and re-add a `## Phase 10+ — Breadth` placeholder (revisable KB, more device drivers/HAL, physical board boot 4c, capability/IPC extensions).

- [ ] **Step 4: Glossary** — add **Shell**, **UART RX / receive interrupt**, and **line discipline** near the console/UART terms.

- [ ] **Step 5: Cross-reference check**

Run: `./tools/check-references.ps1`
Expected: passes.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0027-interactive-shell.md docs/learning/README.md docs/roadmap/roadmap.md docs/glossary.md
git commit -m "docs: Phase 9 interactive shell — learning note 0027, roadmap, glossary"
```

---

## Self-Review (completed during planning)

- **Spec coverage:** LineBuffer → Task 1; UART RX → Task 2; UART IRQ discovery → Task 3; heal accessors + last-diagnosis → Task 4; kernel IRQ wait → Task 5; shell task + PLIC/cap wiring → Task 6; interactive proof → Task 7; docs → Task 8. All spec sections map to a task.
- **Type consistency:** `LineBuffer::push -> LineEvent` and `take -> &str` consistent across Tasks 1 and 6; `uart::get -> Option<u8>` / `enable_rx_interrupt` consistent across Tasks 2 and 6; `MachineInfo.uart_irq: u32` consistent across Tasks 3 and 6; `heal::entry -> Option<(&str,&str)>` / `last_diagnosis -> Option<(&str,&str)>` consistent across Tasks 4 and 6; `wait_irq_for(cap_idx) -> bool` consistent across Tasks 5 and 6; `SHELL_IRQ_CAP = 0` matches the boot grant slot 0.
- **Open verification during execution:** confirm a `print!` macro exists (Task 6 Step 1/4) — define it if only `println!` is exported; confirm `MAX_TASKS` headroom (Task 6 Step 5) — bump only if `scheduler full`; confirm the interactive harness mechanism end-to-end (Task 7 — the stdio-pipe capture was spiked; the RX-delivery half is proven here).
```
