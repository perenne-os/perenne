/* Linker script for the QEMU riscv64 `virt` machine.
 *
 * OpenSBI (-bios default) occupies RAM from 0x80000000 and jumps to our
 * ELF entry point in S-mode; supervisor software conventionally starts
 * at 0x80200000. We link everything at that address; Phase 2b identity-maps
 * exactly these sections, so their boundaries must be 4 KiB-aligned.
 */
OUTPUT_ARCH(riscv)
ENTRY(_start)

BASE_ADDRESS = 0x80200000;

SECTIONS
{
    . = BASE_ADDRESS;

    /* Section start/end symbols feed mem::init()'s W^X mapping:
     * .text R-X, .rodata R--, .data+.bss RW-, stack RW-. Each region
     * is 4 KiB-aligned because permissions are per-page. */
    __text_start = .;
    .text : {
        *(.text.boot)            /* _start must be the first code */
        *(.text .text.*)
    }
    . = ALIGN(4K);
    __text_end = .;

    __rodata_start = .;
    .rodata : {
        *(.rodata .rodata.*)
        *(.srodata .srodata.*)
    }
    . = ALIGN(4K);
    __rodata_end = .;

    __data_start = .;
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
    . = ALIGN(4K);
    __data_end = .;              /* .data and .bss map as one RW region */

    /* 4 KiB guard page below the stack. mem::init() leaves it unmapped,
     * so a stack overflow store-faults instead of silently corrupting
     * .bss (2a's documented limitation). */
    . += 4K;

    /* Boot stack for the single hart we run. */
    .stack (NOLOAD) : {
        __stack_start = .;
        . += 64K;
        __stack_top = .;
    }

    /* Phase 3a: the embedded U-mode "program". mem::init() maps these with
     * the PTE U bit (text R-X-U, data RW-U); everything above stays non-U,
     * so a U-mode access to the kernel faults. 4 KiB-aligned: permissions
     * are per-page. */
    . = ALIGN(4K);
    __user_text_start = .;
    .user_text : { *(.user_text) }
    . = ALIGN(4K);
    __user_text_end = .;

    __user_data_start = .;
    .user_data : { *(.user_data) }
    . = ALIGN(4K);
    __user_data_end = .;

    __kernel_end = .;            /* frame allocator manages from here */

    /DISCARD/ : {
        *(.eh_frame .eh_frame_hdr)
    }
}
