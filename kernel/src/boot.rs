//! Boot entry: the first instructions our kernel ever runs.
//!
//! OpenSBI jumps here in S-mode with `a0` = booting hart id and
//! `a1` = device-tree blob pointer. Rust code needs a stack and a
//! zeroed .bss before it can run, so the entry is a tiny piece of
//! assembly that does exactly that and then calls `kmain` (main.rs).
//! The symbols `__stack_top`, `__bss_start`, `__bss_end` come from the
//! linker script (kernel.ld).

use core::arch::global_asm;

global_asm!(
    r#"
    .section .text.boot
    .global _start
_start:
    # a0 (hartid) and a1 (dtb) are left untouched: they become kmain's
    # two arguments under the C calling convention.
    la   sp, __stack_top

    # Zero .bss. Rust assumes statics start zeroed; nobody has done it
    # for us. The linker script 8-aligns both symbols, so 8-byte stores
    # are safe.
    la   t0, __bss_start
    la   t1, __bss_end
1:
    bgeu t0, t1, 2f
    sd   zero, 0(t0)
    addi t0, t0, 8
    j    1b
2:
    call kmain
"#
);
