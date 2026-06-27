# Phase 9 — Diagnosis-aware interactive shell (design)

**Status:** approved 2026-06-27 (user authorized completing the phase end-to-end)
**Priority served:** #2 (the self-healing organism) and principle #5 ("the OS
should explain itself"). The first interactive surface, and the first device
**input** in the project.

**Implementation revision (2026-06-27, during build):** two parts of this design
changed when they met reality, and the shipped code + learning note 0027 are
authoritative. (1) **The shell polls UART RX rather than using the RX
interrupt** — QEMU's PLIC only asserts SEIP on the rising edge of an enabled
source (note 0020), which suits one-shot completion IRQs (rng/blk) but drops the
asynchronous re-assertions of character input; polling is reliable. (2) **The
automated proof is a boot self-demo, not a live-keystroke interactive boot** —
reliable serial-input injection was not available in the CI harness on this
platform (`-serial stdio` over a piped stdin delivers only one byte; a
listening-socket chardev is blocked). A single live keystroke reaching the shell
was confirmed manually. The sections below are the original design; read them
with these two revisions in mind.

## The gap

The OS has a growing self-healing knowledge base (Phases 5–7) and a dynamic
capability system (Phase 8), but a human can only watch it print to a log. There
is no way to *ask* the organism what it knows. There is also no device **input**
at all — the UART is transmit-only and the only interrupts handled are device
*completions* (rng/blk). Phase 9 adds a console you type at that queries the
self-healing organism, introducing UART receive (an interrupt-driven input
device) along the way.

## Where the shell runs (a deliberate trade-off)

The shell is a **kernel task**, not a U-mode component. The UART is intrinsically
kernel-owned: the kernel must print from `println!` everywhere, including panic
and trap handlers (`console.rs`). A U-mode shell would contend with the kernel
for the one device it cannot give up, and would need new syscalls merely to read
the KB table. A kernel-task shell drives the existing console for echo/output and
queries `heal` directly. (A later phase could lift the whole console+shell behind
a HAL/component; out of scope here — the console is the one device that stays
kernel-coupled, and the user-space-driver model is already proven for rng/blk/
rtc.)

## Architecture & components

### Pure, host-tested logic

- **`shell::LineBuffer`** (in a new `arch/riscv64/src/shell.rs`) — a fixed
  64-byte line buffer with one method, `push(byte) -> LineEvent`: a printable
  byte is appended (→ `Echo(byte)`); backspace (0x08/0x7f) removes the last byte
  if any (→ `Backspace`/`None`); carriage-return/newline completes the line (→
  `Line`), after which `take()` yields the accumulated `&str` and resets. A full
  buffer drops further printable bytes until completion. No I/O — host-tested.

### UART receive (`arch/riscv64/src/uart.rs`)

- **`get(base, reg_shift) -> Option<u8>`** — read the RX holding register (RBR,
  offset 0) iff LSR.DR (data-ready, bit 0) is set; `None` otherwise. Reading RBR
  deasserts the device's RX interrupt.
- **`enable_rx_interrupt(base, reg_shift)`** — set IER (offset `1 << shift`) bit
  0 (received-data-available interrupt). OpenSBI already configured the line.

### Device-tree discovery (`arch/riscv64/src/dt.rs`)

- Discover the UART's IRQ (the ns16550 node's `interrupts` cell — QEMU virt = 10),
  added to `MachineInfo.uart_irq`. Same firmware-discovery ethos as Phase 4a; no
  hardcoded IRQ.

### Kernel IRQ wait (`arch/riscv64/src/sched.rs`)

- **`wait_irq_for(cap_idx)`** — a kernel-task counterpart of the `wait_irq`
  syscall (mirrors how `recv_message`/`call_message` are kernel counterparts of
  the IPC syscalls): look up the `Interrupt` cap, `plic::complete` the previous
  claim (re-arm), `park_current(WaitingIrq(irq))`. The kernel shell task blocks
  on the UART IRQ through this.

### Organism query accessors (`arch/riscv64/src/heal.rs`)

- **`entry(i) -> Option<(&'static str, &'static str)>`** — the `i`-th loaded KB
  entry's `(id, title)`, for the `kb` command to enumerate the runtime table.
- **`last_diagnosis() -> Option<(&'static str, &'static str)>`** — the `(id,
  playbook)` of the most recent contained-crash diagnosis, for the `diag`
  command. A `static LAST_DIAGNOSIS` set in `exit_current`'s `Some(issue)` arm
  (the existing diagnosis log site).

### The shell task (`arch/riscv64/src/shell.rs` + `kernel/src/main.rs`)

- A kernel task that: enables UART RX, prints `shell: ready` and a `> ` prompt,
  then loops `wait_irq_for(UART_IRQ_CAP)` → drain `uart::get` while data-ready →
  feed each byte to its `LineBuffer` (echoing via the console) → on a completed
  line, dispatch the command, print the result and a fresh prompt. Holds an
  `Interrupt(uart_irq)` capability (granted at boot like the entropy driver); the
  UART IRQ is routed through the PLIC (priority/enable) in `kmain`.

## Commands

| Command | Output |
|---|---|
| `help` | the command list |
| `kb` | each loaded KB entry: `KB-0005  User-space component terminated by a fatal fault` (from `heal::entry`) |
| `diag` | the last diagnosis: `last: KB-0005 -> Restart the component, up to a bounded number of retries.` (from `heal::last_diagnosis`), or `none yet` |
| (empty line) | just a fresh prompt |
| anything else | `unknown command (try 'help')` |

## Data flow

Keystroke → the ns16550 asserts its IRQ → PLIC → `Cause::SupervisorExternal`
claims it and `wake_irq` wakes the shell → the shell reads RBR (deasserting the
device), echoes, and buffers; on Enter it parses the command, calls into `heal`,
prints the answer, and loops back to `wait_irq_for` (which `plic::complete`s the
claim, re-arming). The same claim-masks-in-service / complete-re-arms discipline
the entropy and blk drivers use.

## Error handling

| Situation | Behavior |
|---|---|
| RX interrupt with no data-ready | `uart::get` returns `None`; the drain loop exits — harmless. |
| Line longer than 64 bytes | further printable bytes dropped until Enter; no overflow. |
| Unknown command | `unknown command (try 'help')`; prompt redrawn. |
| `diag` before any crash | `none yet`. |

## Testing

**Host unit tests:** `LineBuffer` — append + echo, backspace (including on an
empty buffer), completion on CR and on LF, the 64-byte cap dropping excess, and
`take()` reset between lines.

**Boot test (`tools/test-qemu.ps1`) — a third, interactive boot.** The existing
two file-based boots (storage + delegation proofs) are unchanged. The new boot
runs QEMU with `-serial stdio` driven by a .NET `Process`: capture stdout via an
`OutputDataReceived` event; after the `shell: ready` marker (and the existing
`heal: diagnosed KB-0005` line, so the KB table and last-diagnosis are
populated), write `kb\r` and assert `KB-0005` appears in the response, then write
`diag\r` and assert `last: KB-0005 -> Restart the component`. This drives real
keystrokes through the interrupt-driven RX path. (Mechanism validated by a spike:
stdio pipes + event capture + write-after-marker capture output before and after
the write with no hang and no listening sockets.)

## Scope / YAGNI

One combined spec. Kernel-task shell; fixed command set; a 64-byte line with
echo + backspace + Enter only — no history, arrow keys, tab-completion, or
multi-line editing. The line discipline is pure and host-tested; the device/IRQ
glue and commands are proven by the interactive boot.

## What this proves / what's next

The organism becomes interrogable: a human can ask what it knows and what it has
diagnosed, over an interrupt-driven console — principle #5 realized. Deferred: a
richer command set (restart a component, query capabilities), command history/
editing, and lifting the console behind a HAL so the shell can become a U-mode
component.
