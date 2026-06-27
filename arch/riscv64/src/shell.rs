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

    /// The completed line as a `&str`, and reset for the next line. The bytes
    /// remain valid until overwritten by the next `push`, so the caller
    /// dispatches the command before reading more input.
    pub fn take(&mut self) -> &str {
        let s = core::str::from_utf8(&self.buf[..self.len]).unwrap_or("");
        self.len = 0;
        s
    }
}

impl Default for LineBuffer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(target_arch = "riscv64")]
use core::sync::atomic::{AtomicUsize, Ordering};

/// The discovered UART MMIO base / register shift, stored at boot for the shell
/// task (which is spawned with no arguments).
#[cfg(target_arch = "riscv64")]
static UART_BASE: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_arch = "riscv64")]
static UART_SHIFT: AtomicUsize = AtomicUsize::new(0);

/// Record the UART location for the shell task. Called from `kmain`.
#[cfg(target_arch = "riscv64")]
pub fn init(base: usize, reg_shift: u32) {
    UART_BASE.store(base, Ordering::Relaxed);
    UART_SHIFT.store(reg_shift as usize, Ordering::Relaxed);
}

#[cfg(target_arch = "riscv64")]
fn prompt() {
    crate::print!("> ");
}

/// Run one completed command, printing its result.
#[cfg(target_arch = "riscv64")]
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

/// Run each command in `["help", "kb", "diag"]` through the real `LineBuffer`
/// and `dispatch`, as if typed — a deterministic boot-time demonstration that
/// the command pipeline assembles a line and answers it against the organism.
/// (Reliable serial-input injection is not available in the CI harness on this
/// platform; live keystrokes over the poll loop are verified manually. This
/// exercises everything but the `uart::get` hardware read.)
#[cfg(target_arch = "riscv64")]
fn self_demo() {
    crate::println!("shell: demo");
    for cmd in ["help", "kb", "diag"] {
        crate::println!("> {cmd}");
        let mut lb = LineBuffer::new();
        for &b in cmd.as_bytes() {
            let _ = lb.push(b);
        }
        let _ = lb.push(b'\r');
        dispatch(lb.take());
    }
    crate::println!("shell: demo done");
}

/// The shell task: configure UART RX, announce readiness, then poll the receive
/// register, feeding each byte through the line discipline and dispatching each
/// completed command against the organism. It yields between polls so other
/// tasks run.
///
/// Why polling, not the UART RX interrupt: QEMU's PLIC only asserts SEIP on the
/// *rising edge* of an enabled source (see learning note 0020). That delivers
/// cleanly for the one-shot *completion* interrupts of the rng/blk drivers, but
/// not for the repeated, asynchronous re-assertions of character input, which
/// drops keystrokes. Polling the data-ready bit is the reliable choice for a
/// console; the completion-driven devices remain interrupt-driven.
#[cfg(target_arch = "riscv64")]
pub extern "C" fn shell_task() -> ! {
    let base = UART_BASE.load(Ordering::Relaxed);
    let shift = UART_SHIFT.load(Ordering::Relaxed) as u32;
    // SAFETY: `base` is the kernel-owned ns16550, mapped in every address space.
    // Configure a 1-byte RX FIFO trigger and clear the FIFOs before we read.
    unsafe { crate::uart::enable_rx_interrupt(base, shift) };
    let mut line = LineBuffer::new();
    crate::println!("shell: ready (type 'help')");
    prompt();
    // Run the boot demonstration once the organism has diagnosed a crash, so
    // `kb`/`diag` show real, populated data deterministically.
    let mut demoed = false;
    loop {
        if !demoed && crate::heal::last_diagnosis().is_some() {
            crate::println!();
            self_demo();
            prompt();
            demoed = true;
        }
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
        // Nothing pending: let other tasks run, then poll again.
        crate::sched::yield_now();
    }
}

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
        for &b in b"one\r" {
            let _ = lb.push(b);
        }
        assert_eq!(lb.take(), "one");
        for &b in b"two\r" {
            let _ = lb.push(b);
        }
        assert_eq!(lb.take(), "two");
    }

    #[test]
    fn caps_at_capacity_then_still_completes() {
        let mut lb = LineBuffer::new();
        for _ in 0..200 {
            let _ = lb.push(b'z');
        }
        assert!(matches!(lb.push(b'\r'), LineEvent::Line));
        assert_eq!(lb.take().len(), CAP);
    }
}
