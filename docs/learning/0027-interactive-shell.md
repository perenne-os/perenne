# 0027 — A diagnosis-aware shell (and why it polls)

**One-line:** the OS gets its first interactive surface — a console whose `kb`
and `diag` commands let a human *interrogate the self-healing organism* (list the
loaded knowledge base, show the last diagnosis). Principle #5 ("the OS should
explain itself") made concrete.

## What changed
- A UART **receive** path (`uart::get` reads the RBR when LSR.DR is set; a 1-byte
  FIFO trigger so every keystroke is visible) — the project's first device
  *input*, where before the UART was transmit-only.
- A pure, host-tested **`LineBuffer`** line discipline: printable bytes append,
  backspace removes, CR/LF completes a line. Fixed 64-byte buffer.
- A kernel **shell task** that drives the existing console for echo/output,
  assembles a line, and dispatches `help` / `kb` / `diag` against `heal`. New
  `heal::entry` (list the runtime KB table) and `heal::last_diagnosis` (the
  organism's most recent contained-crash diagnosis, recorded from the crash
  path).

## The shell is a kernel task (a deliberate trade-off)
The UART is the one device the kernel cannot give up: it prints from `println!`
everywhere, including panic and trap handlers. A user-space shell would contend
for that device and need new syscalls just to read the KB. So the shell is a
kernel task — it drives the console directly and reads `heal` directly. The
user-space-driver model (ADR 0007) is already proven for rng/blk/rtc; the console
is intrinsically kernel-coupled, and a later phase could lift the whole
console+shell behind a HAL.

## The idea worth keeping: input doesn't suit QEMU's edge-delivered PLIC
The plan was interrupt-driven RX. It didn't work reliably, and *why* is the
lesson. QEMU's PLIC only asserts the external-interrupt line (SEIP) on the
**rising edge of an enabled source** (learning note 0020). That fits a *one-shot
completion* interrupt perfectly — rng/blk assert once per operation, get claimed,
acked, and completed. But **character input is a stream of asynchronous
re-assertions**: bytes that arrive during the brief window between draining the
FIFO and re-arming the claim never produce a new rising edge, so keystrokes are
dropped. Polling the data-ready bit sidesteps the whole edge problem and is the
reliable choice for a console. The general lesson: *interrupts model "a thing
finished"; polling models "is there a thing yet?" — pick by which question the
device answers.* The completion-driven devices stay interrupt-driven; the
console polls.

## Proving it without reliable input injection
Driving real keystrokes into QEMU from the test harness turned out to be the hard
part on this platform: `-serial stdio` over a piped stdin delivers only the first
byte, and a listening-socket chardev is blocked by the environment. A single
keystroke *was* confirmed to reach the shell and echo (manual check). For a
deterministic automated proof, the shell runs a **boot self-demo** once the
organism has diagnosed a crash: it feeds `help`/`kb`/`diag` through the *real*
`LineBuffer` and `dispatch`, so the smoke test asserts the organism answering
with live data —
`KB-0005  User-space component terminated by a fatal fault` and
`last: KB-0005 -> Restart the component, up to a bounded number of retries.`
This covers the whole pipeline except the `uart::get` hardware read, which is
simple and manually verified.

## What's next
Reliable serial-input injection in CI (a named-pipe or socket chardev once the
environment allows it) to assert live typing; a richer command set (restart a
component, inspect capabilities); line editing/history; and lifting the console
behind a HAL so the shell can become a user-space component.
