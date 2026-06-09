/* Linker script for the QEMU riscv64 `virt` machine.
 *
 * OpenSBI (-bios default) occupies RAM from 0x80000000 and jumps to our
 * ELF entry point in S-mode; supervisor software conventionally starts
 * at 0x80200000. We link everything at that address (no virtual memory
 * yet — that is Phase 2).
 */
OUTPUT_ARCH(riscv)
ENTRY(_start)

BASE_ADDRESS = 0x80200000;

SECTIONS
{
    . = BASE_ADDRESS;

    .text : {
        *(.text.boot)            /* _start must be the first code */
        *(.text .text.*)
    }

    .rodata : {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }

    .data : {
        *(.data .data.*)
        *(.sdata .sdata.*)
    }

    /* boot.rs zeroes [__bss_start, __bss_end) in 8-byte steps. */
    .bss : ALIGN(8) {
        __bss_start = .;
        *(.bss .bss.*)
        *(.sbss .sbss.*)
        . = ALIGN(8);
        __bss_end = .;
    }

    /* Boot stack for the single hart we run in Phase 1. */
    .stack (NOLOAD) : ALIGN(16) {
        . += 64K;
        __stack_top = .;
    }

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
    }
}
