# 0003 — Traps and the timer heartbeat (Phase 2a)

How the kernel got its reflexes: catching exceptions, surviving them,
and waking up once a second.

## Traps: one mechanism, two flavors

A **trap** is the hart's reaction to an exceptional event: stop what
you're doing, jump to the address in `stvec`, and let the kernel sort
it out. Two flavors share that one mechanism:

- **Exceptions** are *synchronous* — caused by the instruction itself
  (our deliberate `ebreak`, an illegal instruction, later a page fault).
- **Interrupts** are *asynchronous* — they arrive from outside, like the
  timer saying "your deadline passed".

`scause` tells them apart: the top bit is 1 for interrupts, and the rest
is a cause code (3 = breakpoint exception, 5 = supervisor timer interrupt).

What clicked: this is the same `ecall`-shaped machinery from Phase 1,
pointed the other way. In Phase 1 we *made* traps into OpenSBI below us;
now we *receive* traps from our own code (and later, from user programs
above us — that's what a syscall is).

## Why the entry is assembly again

The handler interrupts code mid-thought. Rust would freely overwrite
registers the interrupted code still needs, so the first thing that runs
is assembly that saves **all 31** general-purpose registers plus
`sepc`/`sstatus`/`scause`/`stval` into a `TrapFrame` on the stack, and
the last thing is assembly restoring them and executing `sret`. We save
the *full* set (not just caller-saved) because a saved-everything frame
is precisely a suspended task — Phase 2c's context switch will reuse it.

Subtleties that bit during review: the original `sp` has to be
reconstructed (`sp + frame size`) because the entry already moved it,
and it must be restored *last* — the moment `sp` changes, the frame is
conceptually freed.

## Recovering from a breakpoint

`ebreak` does not advance the PC: `sepc` points *at* the breakpoint, and
a bare `sret` would re-execute it forever. The handler advances `sepc`
past it first — by 4 bytes normally, but riscv64**gc** includes the
compressed extension, so `c.ebreak` is only 2. The encoding rule: a
16-bit parcel ending in `0b11` starts a 4-byte instruction; anything
else is compressed. The "survived breakpoint" line in the boot output
exists purely to prove the resume worked.

## The timer: one-shot by design

SBI timers aren't periodic. `sbi::set_timer(deadline)` fires *once*; the
handler re-arms the next deadline (`time` CSR + interval). The `time`
CSR ticks at the platform **timebase** — 10 MHz on QEMU virt, hardcoded
as a documented constant (`TIMEBASE_HZ`) until Phase 4 reads it from the
device tree. Each tick prints `tick: N` where N is the monotonic count
since boot.

Enabling order matters and is one-way: install `stvec` first, *then*
set `sie.STIE` and `sstatus.SIE`. Enable interrupts with no handler and
the first tick is a wild jump.

The pleasing part: `kmain` still ends in the same `wfi` park loop from
Phase 1 — but now the hart genuinely sleeps, wakes once a second,
handles the tick, and sleeps again. The kernel has a pulse.

## A known limitation: no guard against handler recursion

Interrupts stay masked while the trap handler runs, so a second timer
interrupt cannot arrive mid-handler. But a *fault* inside the handler
or its panic path (say, a bad pointer dereference) would re-enter
`__trap_entry` and recurse on the same 64 KiB boot stack with no guard
page. It does not "double-fault to OpenSBI" the way x86 hardware would
— it just silently overflows. This is a known 2a limitation, accepted
because the handler is tiny and the stack large relative to what it
uses. It will be revisited before 2c enables preemption and makes
nested entry a realistic risk.

Printing from trap context is safe for a different reason: `console_putchar`
issues an `ecall` that traps to M-mode (OpenSBI), not back to our S-mode
vector — so there is no re-entry on the print path.

## Crash diagnostics for free

Any trap we don't recognize prints the decoded cause, `sepc`, `stval`,
and the full register dump, then panics. This is the project's first
real diagnostic surface — every future fault lands here until it gets
its own handler.

## Day-to-day commands

Unchanged: `./tools/run-qemu.ps1` to watch it live (you'll see the `tick: N`
lines), `./tools/test-qemu.ps1` for the automated check.
