# 0005 — Scheduling and context switching (Phase 2c)

How the kernel learned to run more than one thing: parking a task's
registers, handing the CPU to another, and letting the timer take the CPU
back from a task that won't give it up.

## A task is just a saved set of registers

A "task" sounds heavy, but in a kernel it is mostly a stack plus a place
to store registers when it is not running. Our `Context` is only the
*callee-saved* set — `ra`, `sp`, and `s0..s11`. That is a deliberate
contrast with the trap frame, which saves all 31 GPRs.

## Why two switch paths save different amounts

The insight of the phase: it depends on whether the switch is *voluntary*.
`yield_now()` is reached by a normal function call, and the RISC-V calling
convention says the caller already preserved any caller-saved register it
still needs. So the switch only has to save the callee-saved set — 14
registers. Preemption is involuntary: a timer interrupt freezes the task
mid-instruction, having volunteered nothing, so the *full* 31-GPR state
must be saved — and it already is, by the trap entry assembly from Phase
2a. So preemption reuses the very same `switch_context`; the trap frame
does the heavy lifting underneath it.

## The first run of a task that has never run

`switch_context` restores registers and `ret`s — but a brand-new task has
no saved registers to restore. We forge a `Context`: `ra` points at a
small trampoline, `sp` at the top of the task's stack, and `s0` holds the
real entry function. The first switch "returns" into the trampoline, which
jumps to the entry. There is nothing to restore the first time — we
manufacture a plausible past.

## Interrupts: the subtle part

The bug that does not announce itself: if you enable interrupts at the
wrong moment, a timer tick lands in the middle of a context switch and
re-enters the scheduler. Two rules keep it correct. First, the
pick-the-next-task-and-switch critical section runs with interrupts
disabled. Second — less obvious — `yield_now` restores the interrupt state
the caller *had on entry*, rather than blindly enabling. A task that
yielded with interrupts on resumes with them on; a task preempted from
inside the trap handler (interrupts already masked) resumes with them
masked and lets `sret` re-enable them atomically. New tasks are the
exception that proves the rule: they never went through a `yield_now`, so
the trampoline enables interrupts explicitly.

## Proving preemption, not just claiming it

Cooperative scheduling is easy to fake-pass: three tasks taking turns
could just be three function calls. So the boot proves the real thing.
Task C runs a tight loop that never yields. Without preemption the kernel
would freeze on it. The timer tick descheduled it, and tasks A and B —
which only run again if something *takes* the CPU from C — print a line
that can only appear under preemption. If that line shows up, preemption
works; the smoke test greps for exactly it.

## Honest caveat carried forward

Only the boot stack has a guard page; the three task stacks do not, so a
task that overflowed its 16 KiB stack would corrupt memory silently. The
demo tasks are nowhere near that, and the real fix (per-task guard pages
and a dedicated trap stack) belongs with the Phase 3 work that introduces
user space and privilege transitions.
