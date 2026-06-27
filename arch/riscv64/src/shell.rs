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
