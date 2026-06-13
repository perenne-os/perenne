# 0004 — Memory and paging (Phase 2b)

How the kernel took ownership of its own memory: handing out physical
frames, building Sv39 page tables, and turning on the MMU without
sawing off the branch it sits on.

## Physical vs. virtual: two address spaces, one RAM

Until now every address was *physical* — `0x8020_0000` named a real DRAM
location. Paging inserts a translation layer: code uses *virtual*
addresses, and the MMU walks a page table to find the physical frame
behind each 4 KiB page. We chose an **identity mapping** (VA = PA) for
the kernel, which sounds like it changes nothing — but the point isn't
*where* things live, it's the **permission bits** that come with each
mapping. Identity-mapped W^X is real protection.

## The frame allocator: a bitmap, on purpose

One bit per 4 KiB frame; 128 MiB needs a 4 KiB bitmap. The xv6-style
free-list is the classic choice (each free frame stores a pointer to the
next), but its data structure lives *inside* freed memory — impossible
to unit-test on the host and silently corrupted by a double-free. The
bitmap's core is pure integer math: it tests on the host like the trap
decoder, and a double-free hits a loud assert instead of corrupting the
list. The O(n) scan is irrelevant at 32k frames.

## Sv39 in one paragraph

A virtual address's bits 38:12 split into three 9-bit indices (VPN2/1/0),
one per table level; each table is itself exactly one 4 KiB frame holding
512 PTEs. A PTE holds the next level's (or the final page's) physical
frame number plus flag bits — Valid, Read, Write, eXecute, and friends.
`satp` holds the root table's frame number and the mode (8 = Sv39);
`sfence.vma` flushes the TLB, the cache of recent translations.

## Turning it on without crashing

The scary moment: `csrw satp` changes how the *next instruction fetch*
is addressed. Identity mapping is what makes it safe — the PC is valid
in both worlds. The order of operations matters everywhere here: trap
handler first (a paging bug should fault loudly, not hang), then build
the complete table, then fence-write-fence. Two other traps avoided:
the A/D bits are pre-set because hardware is allowed to fault instead of
setting them; and OpenSBI's region stays *unmapped* — the kernel can't
even read firmware memory now, and SBI calls still work because `ecall`
jumps to M-mode where `satp` doesn't apply.

## W^X, proven, not assumed

`.text` is executable but not writable; `.rodata` is only readable; data
is writable but not executable. The boot now *proves* it: kmain arms a
one-shot flag and deliberately stores to `.rodata`. The MMU raises a
store page fault, the handler recognizes the armed probe, skips the
store (reusing 2a's instruction-length logic), and boot continues. If
the store *succeeds*, kmain panics — broken W^X fails the smoke test.
A subtlety: the store is inline asm, because a Rust-level write through
a shared reference is UB even if it never retires.

## The guard page

Paging also paid off a 2a debt: the boot stack now has an unmapped page
below it, so an overflow store-faults instead of silently corrupting
.bss. Honest caveat: the fault itself pushes a trap frame onto the
overflowed stack, which faults again — the recursion marches `sp`
through the guard and corrupts a few hundred bytes below it before the
panic lands. Still a loud, attributable crash instead of a silent one;
the real fix is 2c's dedicated trap stack.
