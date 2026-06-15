# 0007 — U-mode tasks in the run queue (Phase 3b-i)

**One-line:** kernel and U-mode tasks now share one round-robin run queue.

## What changed
- 3a ran a U-mode task *standalone* (`enter_user` parked `kmain`, `sret`ed,
  `terminate_user` returned). 3b-i folds that into the 2c scheduler: a
  U-mode task is an ordinary slot.
- `spawn_user` forges a first-run `Context` that `switch_context` "returns"
  into via `user_trampoline` (3a's `enter_user_asm`, renamed) — the U-mode
  analogue of `task_trampoline`.
- `TaskState::Exited` + `exit_current` replace `terminate_user`:
  termination is now "mark Exited, switch to the next task," not a return
  to `kmain`. Containment is strictly stronger — the scheduler survives and
  keeps running the other tasks.
- A `yield` syscall (a7 = 3) lets U-mode tasks cooperate (they can't call
  the scheduler directly).

## The key idea
Per-task privilege rides the **saved trapframe** for free: `switch_context`
and the trap entry/exit asm are unchanged. Every task resumes through the
trap return, which restores `sepc`/`sstatus` (privilege) and re-arms
`sscratch` from the per-task kernel-stack top. The only new entry path is
the *first* run of a U-mode task.

## Proof (smoke test)
Two U-mode tasks round-robin via `yield` and exit cleanly; a bad U-mode
task is contained while the scheduler keeps running; the first timer tick
preempts a U-mode task. Next: 3b-ii (per-address-space isolation).
