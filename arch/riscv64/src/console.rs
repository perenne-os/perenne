//! Kernel console: formatted text output, switchable between the SBI
//! firmware console (early boot) and a direct ns16550 UART (once the device
//! tree has been parsed — Phase 4b).
//!
//! `Console` implements `core::fmt::Write` by dispatching each byte to the
//! active backend, so the `print!`/`println!` macros work unchanged with no
//! allocator. `use_uart` flips the backend from SBI to the UART.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::sbi;

/// Active backend: `0` = SBI firmware console; non-zero = the ns16550 MMIO
/// base address. Starts at the SBI console so the earliest boot lines print
/// before the UART is discovered.
static UART_BASE: AtomicUsize = AtomicUsize::new(0);
static UART_SHIFT: AtomicUsize = AtomicUsize::new(0);

/// Switch console output to the discovered ns16550 UART. Until this is
/// called, output goes through the SBI firmware console.
pub fn use_uart(base: usize, reg_shift: u32) {
    UART_SHIFT.store(reg_shift as usize, Ordering::Relaxed);
    UART_BASE.store(base, Ordering::Relaxed); // store last: makes the switch visible
}

/// Zero-sized writer dispatching to the active console backend.
pub struct Console;

impl Write for Console {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        let base = UART_BASE.load(Ordering::Relaxed);
        if base == 0 {
            for b in s.bytes() {
                sbi::console_putchar(b);
            }
        } else {
            let shift = UART_SHIFT.load(Ordering::Relaxed) as u32;
            for b in s.bytes() {
                // SAFETY: `base` came from the device tree via use_uart, and
                // the UART page is mapped R+W in every address space (see
                // mem::map_kernel_sections).
                unsafe { crate::uart::put(base, shift, b) };
            }
        }
        Ok(())
    }
}

/// Implementation detail of `print!`/`println!`. Not for direct use.
pub fn _print(args: fmt::Arguments) {
    // Console output cannot fail; ignore the fmt::Result.
    let _ = Console.write_fmt(args);
}

/// Prints to the kernel console.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::console::_print(core::format_args!($($arg)*)));
}

/// Prints to the kernel console, with a trailing newline.
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", core::format_args!($($arg)*)));
}
