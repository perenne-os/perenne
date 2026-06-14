# 0006 — User mode and system calls (Phase 3a)

How the kernel handed the CPU to code it does not trust, gave that code a
single narrow door back in (the syscall), and proved the door is the
*only* way in.

## Two privilege levels, one instruction to cross

RISC-V S-mode (the kernel) and U-mode (applications) differ in what memory
and instructions are allowed. `sret` is the move *down*: it reads
`sstatus.SPP` (0 = return to U-mode), restores the prior interrupt-enable
from `SPIE`, and jumps to `sepc`. To launch the first user task we forge
that state — `SPP = 0`, `SPIE = 1`, `sepc` = the user entry, `sp` = the
user stack — and `sret`. There was no "previous" U-mode state to restore;
we manufactured a plausible one, exactly as Phase 2c forged a task's first
context.

## The untrusted stack problem

A trap from U-mode arrives with `sp` still pointing at the *user* stack.
The kernel must not push its trap frame there — the user controls it. The
fix is `sscratch`, a scratch CSR used as a privilege-aware stack pointer:
while the user runs it holds the kernel trap-stack top; while the kernel
runs it holds 0. The first instruction of the trap entry swaps `sp` and
`sscratch`. From U-mode that lands us on the trap stack (and stashes the
user `sp`); from S-mode the swap yields 0, which we detect and undo,
running on the current kernel stack exactly as before. One instruction,
two behaviors, selected by a sentinel.

## The syscall is the whole interface

A user task reaches the kernel only by executing `ecall` (trap cause 8).
The ABI is a convention: `a7` selects the call, `a0..` carry arguments,
`a0` carries the result. `ecall` does not advance the PC, so the handler
adds 4 to `sepc` or the task would trap on the same instruction forever.
Two calls exist so far: `print` and `exit`.

## The confused deputy, and why SUM is off by default

`print` takes a pointer and a length *from the user*. If the kernel — which
can read anything — blindly dereferenced them, a user could pass a kernel
address and have the privileged kernel read it out. That is the "confused
deputy": a powerful agent tricked into misusing its power. Two defenses
compose. First, the kernel validates the range lies inside the user's own
memory before touching it. Second, the hardware bit `sstatus.SUM` is 0 by
default, so even an *accidental* kernel read of a user page faults — and
because of that, reading the (validated) user buffer requires *explicitly*
opening a SUM window for the copy and closing it immediately. The check
stops a deliberate bad pointer; the default-off SUM stops a careless one.

## Containment: a fault that kills the task, not the kernel

When the second demo task reads a kernel address, the MMU faults because
the kernel's pages have no U bit. The handler reads `sstatus.SPP`: the
fault came from U-mode, so instead of panicking it *contains* the task —
records the reason and switches back to the kernel context that launched
it. Contrast the Phase 2b W^X probe: that store fault comes from S-mode and
is deliberately skipped. Same machinery (page fault), opposite response,
chosen by where the fault came from.

## Why the user episode runs before the scheduler

Phase 2c's scheduler demo loops forever, so the user episode runs first and
is *bounded*: `enter_user` `sret`s into the task and returns to `kmain`
only when the task exits or is killed. That kept the 2c scheduler code
untouched and the new privilege machinery isolated — the boundary, not
scheduling, is the lesson here. Putting U-mode tasks into the run queue is
Phase 3b's job.
