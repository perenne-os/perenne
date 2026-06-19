# 0008 — Per-address-space isolation (Phase 3b-ii)

**One-line:** each U-mode task now runs in its own page table; tasks can't
read each other's memory.

## What changed
- 3b-i shared one page table, so every task's stack/data (in `.user_data`)
  was readable by all. 3b-ii gives each U-mode task a private root tree
  built by `mem::build_user_space`: the kernel image cloned in (global),
  the shared `.user_text` code, and only THAT task's page-aligned stack and
  data page.
- `Task` carries a `satp`; the scheduler writes `next.satp` right before
  every `switch_context` (in `enter`/`yield_now`/`exit_current`).
- Per-task user memory is page-aligned (`#[repr(align(4096))]`) so a page —
  the unit of mapping — belongs to exactly one task.

## The key idea
`PTE_G` ("global") is only a TLB hint — it does NOT put the kernel into
every address space. The mapping must physically exist in each tree, so
`build_user_space` clones the kernel via `map_kernel_sections`. And swapping
`satp` mid-`switch_context` is seamless precisely because the kernel sits at
identical VAs in every tree (so the running kernel code/stack don't move).
`switch_context` and the trap asm stay unchanged.

## Invariant
Per-task trees don't map free RAM, so the kernel must not allocate while on
a user `satp` (true in steady state; allocation happens at boot on the
master `satp`).

## Proof (smoke test)
The 3b-i run-queue proofs still pass (now each under its own `satp`), and a
`snoop` task that follows a pointer into another task's data page is
contained. Next: 3b-iii (capabilities + IPC + blocking).
