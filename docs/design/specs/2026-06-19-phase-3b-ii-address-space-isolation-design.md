# Kernel — Phase 3b-ii Design: Per-address-space isolation

- **Date:** 2026-06-19
- **Status:** Draft — awaiting review
- **Scope of this document:** Phase 3b-ii only — giving each U-mode task
  its own page table (`satp`), mapping the kernel into every address space,
  and swapping `satp` on context switch, so two U-mode tasks can no longer
  read each other's memory.

---

## 0. Where 3b-ii sits

Phase 3b ("capabilities & IPC") was decomposed (2026-06-14) into **3b-i**
(U-mode tasks in the run queue — done 2026-06-15) → **3b-ii** (this doc) →
**3b-iii** (capabilities + synchronous IPC + blocking). 3b-i made kernel
and U-mode tasks share one round-robin run queue but kept the single 2b/3a
page table: every U-mode task's stack and data live in the shared
`.user_data` (mapped RW-U once), so any task can read any other's memory.
3b-ii closes that gap — the *isolation* that "two isolated components"
(the 3b goal) requires before they communicate only through IPC (3b-iii).

Predecessors: the [3b-i spec](2026-06-14-phase-3b-i-user-scheduling-design.md)
(§6 filed per-address-space isolation as the next open question) and the
[2b memory spec](2026-06-10-phase-2b-memory-design.md) (Sv39 paging,
`map_range`, the global-bit kernel mappings, `satp_write`).

## 1. Goal

Each U-mode task runs in its **own address space**: its own root page
table, selected by its own `satp` value, swapped in on context switch. The
kernel is mapped into every address space (so a trap from U-mode runs the
handler without a page table switch), but each task's **private** user
memory (its stack, its message data) is mapped **only** in its own tree.
A task that reaches for another task's memory faults and is contained.

**You learn (kept brief):** that `PTE_G` ("global") is only a TLB hint —
the kernel mappings must still *physically exist* in every per-task root
tree to be walked — so "kernel in every address space" means cloning the
kernel mappings into each tree; and that swapping `satp` mid-`switch_context`
is seamless precisely because the kernel occupies identical VAs in every
tree.

**Done when** `./tools/test-qemu.ps1` observes, in one boot, *in addition
to all prior milestones* (greeting, breakpoint recovery, paging line, W^X
block, frame round-trip, ≥ 2 ticks):

1. **The 3b-i run-queue proofs still hold, now under per-task `satp`** —
   two U-mode tasks round-robin via the `yield` syscall and exit cleanly,
   and a non-yielding hog is preempted by the timer. Because every context
   switch now also swaps `satp`, these passing is itself proof the swap is
   correct.
2. **Cross-task isolation** — a `snoop` U-mode task loads a known address
   that holds *another* task's data (an address that was readable in 3b-i's
   shared `.user_data`) and is **contained** (killed by a load page fault)
   while the scheduler keeps running the other tasks.

## 2. Non-goals (deferred)

- **ASIDs** — `satp.ASID` stays 0; every switch does a full TLB flush
  (the `sfence.vma` sandwich already in `satp_write`). Tagged TLBs to
  avoid the flush are a later optimization.
- **Trampoline / hiding kernel text from user spaces** — the kernel
  (`.text` included) is mapped, non-`U`, into every address space
  (Approach A). The xv6-style trampoline that keeps kernel text out of
  user spaces (Approach C) is deferred.
- **Mapping free RAM into user spaces** — per-task trees map the kernel
  *image* (text/rodata/data/bss + kernel stacks) but **not** the free-RAM
  region. The kernel must not allocate frames or otherwise touch free RAM
  while running on a user `satp`; this holds in the current steady state
  (see §3.6). The master table still maps free RAM (the allocator and
  page-table construction run on it).
- **Address-space / page-table reclamation** — an exited task's root tree
  and frames are not reclaimed (stack reaping was already deferred; AS
  reaping joins it).
- **Megapages** — still 4 KiB leaves only (2b deferral unchanged); cloning
  the kernel image per task is cheap enough without them (§3.3).
- **Capabilities, IPC, blocking** — 3b-iii.
- **Dynamic user memory** — no `mmap`/growth/COW; each task's regions are
  fixed at spawn, as in 3b-i.

## 3. Design

### 3.1 Components

Following the arch-crate pattern (pure logic ungated and host-testable;
hardware/frame access gated to `target_arch = "riscv64"`):

| Component | Location | Responsibility |
|-----------|----------|----------------|
| `make_satp(root_pa)` | `arch/riscv64/src/mem/mod.rs` (or `paging.rs`) | Pure: `SATP_MODE_SV39 \| (root_pa >> 12)`. Host-tested. |
| `map_kernel_sections(root)` | `arch/riscv64/src/mem/mod.rs` | Factored out of `init`: identity-map the kernel image — `.text` R-X-G, `.rodata` R-G, `.data`/`.bss` RW-G, stack RW-G — into `root`. Does **not** map free RAM or user sections. Reused by the master table and every per-task tree. |
| `build_user_space(regions)` | `arch/riscv64/src/mem/mod.rs` | Alloc a root frame, `map_kernel_sections` into it, map shared `.user_text` R-X-U, then map each caller-supplied private region. Returns the `satp` value (`make_satp(root)`). |
| `kernel_satp()` | `arch/riscv64/src/mem/mod.rs` | The master `satp`, saved during `init`; used by kernel (S-mode) tasks. |
| `Task.satp` | `arch/riscv64/src/task.rs` | New field: the `satp` to load when this task runs. |
| satp swap in the switch | `arch/riscv64/src/sched.rs` | `spawn` sets `satp = kernel_satp()`; `spawn_user` sets the task's private `satp`. `enter`/`yield_now`/`exit_current` write `next.satp` immediately before `switch_context`. |
| Demo + isolation proof | `kernel/src/main.rs` | Build each U-mode task with only its own stack+messages mapped; add a `snoop` task proving cross-task isolation. |

### 3.2 Why `PTE_G` is not enough, and what "kernel in every space" means

Sv39 translation always walks the tree rooted at `satp.PPN`. `PTE_G` marks
a leaf's TLB entry as global (untagged by ASID, so it survives an `satp`
change without a flush) — but the entry must still be *present in the tree
being walked*. So a per-task root tree that lacked the kernel's PTEs would
fault on the first kernel instruction after the switch.

Therefore every per-task tree must **contain** the kernel mappings.
Approach A clones them: `map_kernel_sections` is run once per tree. Under
identity mapping the kernel and user pages share the same 1 GiB root entry
(both are physical-RAM addresses), so the kernel's root entries cannot
simply be shared between trees — hence cloning rather than root-entry
sharing (that elegant split is the deferred Approach B). Cloning is cheap:
the kernel *image* is small (well under a few MiB → a handful of leaf
tables), and free RAM (the large region) is deliberately excluded (§2).

### 3.3 Building a per-task address space

`build_user_space(regions: &[(start, end, flags)]) -> usize`:

```
root = alloc_zeroed()                      // a fresh root page table
map_kernel_sections(root)                  // kernel image, global, no U
map_range(root, user_text_start, user_text_end, R | X | U)  // shared code
for (start, end, flags) in regions:        // this task's PRIVATE pages
    map_range(root, start, end, flags)     // stack RW-U, messages R-U
return make_satp(root)
```

The shared `.user_text` is mapped R-X-U in every user tree (the same
physical code via separate PTEs — sharing read-only code is not a
confidentiality leak; per-function code isolation is out of scope). Each
task's **private** regions — its kernel-stack-adjacent *user* stack
(`US_*`, RW-U) and its message blob (`*_MSG`, R-U) — are mapped only in its
own tree. Another task's `US_*`/`*_MSG` are simply absent → a load faults.

`build_user_space` runs at spawn time (boot), while `satp` is still the
master table, so the frame allocator and the page-table writes it performs
all see free RAM mapped.

### 3.4 The master table and kernel tasks

`init` now: `map_kernel_sections(master)`, then additionally map free RAM
(RW-G, for the allocator) — the user sections are **no longer** mapped in
the master table (they belong to per-task spaces; `kmain` and kernel tasks
never touch user pages). `init` saves `make_satp(master)` as the kernel
`satp` and `satp_write`s it, exactly as before.

Kernel (S-mode) tasks — e.g. `idle` — carry `satp = kernel_satp()`. They
touch only kernel global memory, which the master table maps in full
(including free RAM), so they are unrestricted.

### 3.5 Swapping `satp` on context switch

`Task` gains `satp: usize`. The swap happens in the scheduler, immediately
before each `switch_context`, in `enter`, `yield_now`, and `exit_current`:

```
satp_write(next.satp)   // sfence sandwich; full TLB flush
switch_context(old, new)
```

This is sound because the kernel occupies identical VAs (global) in the
old and new trees: the `satp_write` itself, the following `switch_context`
code, the kernel stacks, and `SCHED` (in `.bss`) are all mapped the same in
both. When `switch_context` resumes the next task — whether into a parked
continuation (→ trap-return → `sret`) or `user_trampoline` (→ `sret`) —
the hart is already on the next task's `satp`, so its private user pages
are visible exactly when U-mode resumes. The trap entry/exit asm and
`switch_context` are **unchanged**.

`satp_write` always flushes (no ASIDs, §2). Writing the same `satp` value
when staying within one space (e.g. switching between two kernel tasks)
would flush needlessly; with the current cast (one kernel `idle` + U-mode
tasks) this is negligible, and a "skip if unchanged" guard is a noted
future optimization, not required.

### 3.6 The free-RAM invariant

Per-task trees do not map free RAM. So while a U-mode task (or its trap
handler) runs on a user `satp`, the kernel must not dereference free-RAM
addresses — in practice, must not allocate frames or build page tables.
The 3b-i/3b-ii steady state honors this: the trap handler does syscall
dispatch, `yield_now`, and `exit_current`, none of which allocate. Frame
allocation happens only during `init` and `build_user_space`, both on the
master `satp`. A comment (and, where cheap, a debug assert) documents the
invariant so a later phase that wants to allocate in trap context switches
to the kernel `satp` first.

### 3.7 The isolation proof

The demo keeps the 3b-i cast so the run-queue proofs still run (now each
U-mode task under its own `satp`): `ping`/`pong` (print + `yield` ×2 then
`exit(0)`), `hog` (cooperative rounds then spin → preempted), and the
kernel `idle`. It adds:

- **`snoop`** — a U-mode task that loads a *fixed address known to hold
  another task's data* (e.g. `pong`'s message blob / stack, whose address
  is a kernel symbol the demo passes as a literal). In 3b-i that address
  lived in the shared `.user_data` and was readable by anyone; in 3b-ii
  `snoop`'s tree does not map it, so the load faults and `snoop` is
  contained (`Killed(LoadPageFault)`) while every other task keeps running.

`user_bad` (the 3a/3b-i kernel-address probe) is dropped: kernel-boundary
containment is already proven by earlier phases, and `snoop` is the
stronger, on-topic proof for 3b-ii (cross-task, not just kernel,
isolation). As in 3b-i, the probe uses inline-asm `lb` (not
`read_volatile`, which a debug build may turn into a `jalr` into kernel
text → the wrong fault).

### 3.8 Error handling summary

| Failure | Behavior |
|---------|----------|
| U-mode access to an unmapped (e.g. another task's) page | `Load`/`StorePageFault` from U-mode → `exit_current(Killed(cause))` (3b-i path, unchanged); scheduler survives. |
| `exit_current` finds no successor | Unchanged: idle is always `Ready`; the assert documents it. |
| Kernel touches free RAM on a user `satp` | Would fault as a kernel-side page fault → `fatal`. Prevented by the §3.6 invariant (no alloc in trap context); documented. |
| `build_user_space` runs out of frames | `expect`/panic — a static configuration bug (as `map_page` already does). |
| S-mode W^X probe (2b) | Unchanged: skip-and-resume. |

## 4. Testing

Test-first, per house discipline:

- **Host unit tests** (pure cores):
  - `make_satp`: `make_satp(root_pa)` == `SATP_MODE_SV39 | (root_pa >> 12)`
    for a representative aligned `root_pa`; the PPN field is placed
    correctly and mode bits are set.
  - Existing `vpn`/`pte_for`/`pte_to_pa` tests stay green; `Task.satp` is
    threaded through the `sched` test helpers (constructed as 0) so the
    `pick_next`/`Exited` tests stay green.
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): keeps every prior pattern, adds the **`snoop` containment**
  line, and keeps the 3b-i run-queue proofs (which now also prove `satp`
  swapping works on every switch).

## 5. Deliverables

1. `mem`: `make_satp`, `map_kernel_sections` (factored from `init`),
   `build_user_space`, `kernel_satp`; `init` updated (master table = kernel
   sections + free RAM, no user sections; saves the kernel `satp`).
2. `task.rs`: `Task.satp` field.
3. `sched.rs`: `spawn`/`spawn_user` set `satp`; `enter`/`yield_now`/
   `exit_current` write `next.satp` before `switch_context`.
4. `kmain`: per-task address spaces for the U-mode tasks; the `snoop`
   isolation proof; `idle` on the kernel `satp`.
5. Extended QEMU smoke test + host unit tests, all green.
6. Short learning note `docs/learning/0008-address-space-isolation.md`.
7. Roadmap: 3b-ii marked done with date.
8. Glossary: `satp` (if not already), address space, the global bit's TLB
   meaning — only genuinely new terms.

## 6. Open questions (for later sub-phases)

- **3b-iii (capabilities + IPC):** with per-address-space isolation in
  place, IPC must copy (or grant) across address spaces — the kernel,
  mapped in both, is the natural intermediary; how do capability tokens
  name endpoints, and where do blocked/waiting task states live?
- **ASIDs:** tagging each space to avoid the full-flush-per-switch cost.
- **Approach B/C later:** sharing kernel root entries (needs a user VA
  region distinct from the kernel's) and/or a trampoline to keep kernel
  text out of user spaces.
- **AS reclamation:** what frees an exited task's root tree and frames
  without racing a switch (joins the deferred stack-reaping question).
- **Allocating in trap context:** if a later phase needs it, switch to the
  kernel `satp` first (per the §3.6 invariant).
