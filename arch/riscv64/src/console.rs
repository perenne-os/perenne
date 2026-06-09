//! Kernel console: formatted text output over the SBI firmware console.
//!
//! `SbiConsole` implements `core::fmt::Write` by sending every byte
//! through [`crate::sbi::console_putchar`], which lets us reuse Rust's
//! normal formatting machinery (`write!`, `format_args!`) with no
//! allocator. The `print!`/`println!` macros mirror std's.

use core::fmt::{self, Write};

use crate::sbi;

/// Zero-sized writer that sends bytes to the SBI console.
pub struct SbiConsole;

impl Write for SbiConsole {
    fn write_str(&mut self, s: &str) -> fmt::Result {
        for b in s.bytes() {
            sbi::console_putchar(b);
        }
        Ok(())
    }
}

/// Implementation detail of `print!`/`println!`. Not for direct use.
pub fn _print(args: fmt::Arguments) {
    // Writing to the SBI console cannot fail; ignore the fmt::Result.
    let _ = SbiConsole.write_fmt(args);
}

/// Prints to the SBI console.
#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::console::_print(core::format_args!($($arg)*)));
}

/// Prints to the SBI console, with a trailing newline.
#[macro_export]
macro_rules! println {
    () => ($crate::print!("\n"));
    ($($arg:tt)*) => ($crate::print!("{}\n", core::format_args!($($arg)*)));
}
