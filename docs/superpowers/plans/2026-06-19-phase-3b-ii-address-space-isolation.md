# Phase 3b-ii: Per-address-space isolation — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans (inline, per the user's preference for this project) to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give each U-mode task its own page table (`satp`), clone the kernel into every address space, and swap `satp` on context switch — so two U-mode tasks cannot read each other's memory.

**Architecture:** Each U-mode task gets a private root tree built by `mem::build_user_space` = `map_kernel_sections` (kernel image, global, no free RAM) + shared `.user_text` (R-X-U) + that task's own page-aligned stack (RW-U) and data page (R-U). The scheduler writes `next.satp` right before each `switch_context`; the kernel sits at identical VAs in every tree, so `switch_context` and the trap asm stay unchanged. A `snoop` task proves cross-task isolation.

**Tech Stack:** Rust `no_std`, RISC-V (riscv64gc), QEMU virt + OpenSBI, PowerShell smoke test. Host tests: `cargo test -p kernel-arch-riscv64`. Bare build: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`; kernel binary: `cargo build -p kernel --target riscv64gc-unknown-none-elf`.

**Spec:** `docs/superpowers/specs/2026-06-19-phase-3b-ii-address-space-isolation-design.md`

**Reference — final demo:** Run queue = `ping`, `pong` (print their own data page twice via `print`, `yield` between, then `exit(0)`), `hog` (print twice then spin), `snoop` (load a pointer kept in its own page that targets `pong`'s data page → faults → contained), `idle` (kernel, `yield_now`+`wfi`). Spawn order ping, pong, hog, snoop, idle (ping runs first). Each U-mode task runs under its own `satp`; the kernel is cloned into each tree.

---

## Task 1: `paging::make_satp` (pure, host-tested)

**Files:**
- Modify: `arch/riscv64/src/mem/paging.rs` (+ its `tests` module)

- [ ] **Step 1: Write the failing test**

Add to the `tests` module in `arch/riscv64/src/mem/paging.rs`:

```rust
    #[test]
    fn make_satp_sets_mode_and_ppn() {
        // root at 0x8020_0000: ppn = 0x80200; mode field (bits 63:60) = 8.
        let satp = make_satp(0x8020_0000);
        assert_eq!(satp, (8usize << 60) | 0x80200);
        assert_eq!(satp >> 60, 8, "mode field = Sv39");
        assert_eq!(satp & 0xFFF_FFFF_FFFF, 0x80200, "ppn = root >> 12");
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 make_satp`
Expected: FAIL — `cannot find function make_satp`.

- [ ] **Step 3: Implement `make_satp` + `SATP_SV39`**

In `arch/riscv64/src/mem/paging.rs`, just below the PTE-flag constants (after `PTE_D`), add:

```rust
/// `satp` mode field (bits 63:60) selecting Sv39 translation.
pub const SATP_SV39: usize = 8 << 60;

/// Build the `satp` value that points translation at the root page table
/// physically at `root_pa` (4 KiB-aligned) in Sv39 mode: mode field |
/// root PPN (`root_pa >> 12`). Pure (host-tested); the gated callers pass
/// it to `csr::satp_write`.
pub fn make_satp(root_pa: usize) -> usize {
    SATP_SV39 | (root_pa >> 12)
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 make_satp`
Expected: PASS. Then `cargo test -p kernel-arch-riscv64` → all green.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/mem/paging.rs
git commit -m "feat(paging): pure make_satp + SATP_SV39 constant"
```

---

## Task 2: `mem` — kernel-section factor, per-task address spaces, kernel satp

**Files:**
- Modify: `arch/riscv64/src/mem/mod.rs`
- Modify: `arch/riscv64/src/csr.rs` (remove the now-unused `SATP_MODE_SV39`)

Context: `mem/mod.rs` currently has a `sym!` macro for linker symbols, the `__user_text_start/end`/`__user_data_start/end` externs, and `init()` which maps kernel sections + free RAM + the two user sections then calls `csr::satp_write(csr::SATP_MODE_SV39 | (root >> 12))`. It already does `use core::sync::atomic::{AtomicBool, Ordering};`.

- [ ] **Step 1: Add the `KERNEL_SATP` static and `AtomicUsize` import**

In `arch/riscv64/src/mem/mod.rs`, change the atomic import line:

```rust
use core::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
```

Add this static near the top of the gated section (e.g. just above `RAM_END`):

```rust
/// The master kernel `satp`, saved by [`init`] and handed to kernel
/// (S-mode) tasks via [`kernel_satp`]. Per-task user spaces clone the
/// kernel into their own trees (see [`build_user_space`]).
#[cfg(target_arch = "riscv64")]
static KERNEL_SATP: AtomicUsize = AtomicUsize::new(0);
```

- [ ] **Step 2: Factor `map_kernel_sections` and rewrite `init`**

Replace the body of `init` and add `map_kernel_sections` above it. The new code:

```rust
/// Identity-map the kernel *image* — `.text` R-X, `.rodata` R, `.data`/
/// `.bss` RW, the boot stack RW — all **global** (`PTE_G`), into `root`.
/// Reused by the master table and by every per-task user tree (the kernel
/// must be present in every address space because `PTE_G` is only a TLB
/// hint, not a substitute for the mapping existing in the walked tree).
/// Deliberately excludes free RAM and the user sections.
///
/// # Safety
/// `root` must point at a valid (zeroed/in-construction) page table; called
/// with free RAM identity-mapped (MMU off in `init`, or master `satp` when
/// building a user tree) so the page-table writes land.
#[cfg(target_arch = "riscv64")]
unsafe fn map_kernel_sections(root: *mut paging::PageTable) {
    use paging::{PTE_G, PTE_R, PTE_W, PTE_X};
    // SAFETY: forwarded; all ranges are kernel.ld symbols, page-aligned.
    unsafe {
        paging::map_range(root, sym!(__text_start), sym!(__text_end), PTE_R | PTE_X | PTE_G);
        paging::map_range(root, sym!(__rodata_start), sym!(__rodata_end), PTE_R | PTE_G);
        paging::map_range(root, sym!(__data_start), sym!(__data_end), PTE_R | PTE_W | PTE_G);
        paging::map_range(root, sym!(__stack_start), sym!(__stack_top), PTE_R | PTE_W | PTE_G);
    }
}
```

Then rewrite `init` (replace the existing one):

```rust
/// Bring memory management up: arm the frame allocator over free RAM,
/// build the master kernel page table (kernel image + free RAM, all
/// global; user sections belong to per-task trees now), enable Sv39, and
/// save the kernel `satp`.
///
/// Call exactly once, after `trap::init()` and before `timer::start()`.
#[cfg(target_arch = "riscv64")]
pub fn init() {
    use paging::{PTE_G, PTE_R, PTE_W};

    // SAFETY: sym! reads linker-symbol addresses (page-aligned by kernel.ld);
    // the MMU is still off, so writes land at the physical addresses we own.
    unsafe {
        let free_ram = (sym!(__kernel_end), RAM_END);
        frame::ALLOCATOR.with(|a| a.init(free_ram.0, free_ram.1));

        let root = frame::alloc_zeroed().expect("no frame for root page table").0
            as *mut paging::PageTable;
        map_kernel_sections(root);
        // Free RAM mapped eagerly so allocated frames are usable; the master
        // table (used by the kernel and at boot) needs it. Per-task user
        // trees deliberately do NOT map free RAM.
        paging::map_range(root, free_ram.0, free_ram.1, PTE_R | PTE_W | PTE_G);

        let satp = paging::make_satp(root as usize);
        KERNEL_SATP.store(satp, Ordering::Release);
        crate::csr::satp_write(satp);
    }
}
```

(Note: the two `paging::map_range` calls for `__user_text_*`/`__user_data_*` that `init` used to make are **removed** — user sections are now mapped per task.)

- [ ] **Step 3: Add `build_user_space` and `kernel_satp`**

Add below `init` (before or after `free_frames`/`total_frames`, your choice):

```rust
/// The master kernel `satp`, for kernel (S-mode) tasks. Valid after
/// [`init`].
#[cfg(target_arch = "riscv64")]
pub fn kernel_satp() -> usize {
    KERNEL_SATP.load(Ordering::Acquire)
}

/// Build a private address space for one U-mode task and return its `satp`.
/// The tree clones the kernel image (global), maps the shared `.user_text`
/// (R-X-U) code, and maps this task's own page-aligned `stack` (RW-U) and
/// `data` page (R-U). Other tasks' pages are absent → a cross-task access
/// faults. Both regions are half-open `(start, end)`, page-aligned.
///
/// Call at spawn time, while the master `satp` is active (the new tree's
/// frames come from free RAM, which only the master table maps).
#[cfg(target_arch = "riscv64")]
pub fn build_user_space(stack: (usize, usize), data: (usize, usize)) -> usize {
    use paging::{PTE_R, PTE_U, PTE_W, PTE_X};
    // SAFETY: a fresh zeroed root; map_kernel_sections + the user ranges are
    // valid (linker symbols / page-aligned statics); built on the master
    // satp so the page-table writes (in free RAM) land.
    unsafe {
        let root = frame::alloc_zeroed()
            .expect("no frame for user root page table").0
            as *mut paging::PageTable;
        map_kernel_sections(root);
        paging::map_range(root, sym!(__user_text_start), sym!(__user_text_end), PTE_R | PTE_X | PTE_U);
        paging::map_range(root, stack.0, stack.1, PTE_R | PTE_W | PTE_U);
        paging::map_range(root, data.0, data.1, PTE_R | PTE_U);
        paging::make_satp(root as usize)
    }
}
```

- [ ] **Step 4: Remove the unused `SATP_MODE_SV39` from `csr.rs`**

In `arch/riscv64/src/csr.rs`, delete the constant (now superseded by `paging::SATP_SV39`/`make_satp`):

```rust
/// `satp` mode field (bits 63:60) selecting Sv39 translation.
pub const SATP_MODE_SV39: usize = 8 << 60;
```

(Leave `satp_write` itself untouched — it takes the full value as a parameter.)

- [ ] **Step 5: Verify the arch crate builds and host tests pass**

Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS (no remaining references to `csr::SATP_MODE_SV39`; `make_satp` resolves).

Run: `cargo test -p kernel-arch-riscv64`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add arch/riscv64/src/mem/mod.rs arch/riscv64/src/csr.rs
git commit -m "feat(mem): per-task address spaces (build_user_space), kernel_satp; init no longer maps user sections"
```

---

## Task 3: `Task.satp` + scheduler swaps `satp` on every switch

**Files:**
- Modify: `arch/riscv64/src/task.rs`
- Modify: `arch/riscv64/src/sched.rs` (+ its `tests` module)

Context: `Task` (in `task.rs`) currently has `context, state, stack_top, name`. `sched.rs` has `spawn` (kernel task), `spawn_user` (4 args), `enter`, `yield_now`, `exit_current`, all calling `switch_context`. The `sched` tests build tasks via a `task(name, state)` helper.

- [ ] **Step 1: Add `Task.satp`**

In `arch/riscv64/src/task.rs`, add the field to `Task` (with a doc line):

```rust
#[derive(Debug)]
pub struct Task {
    pub context: Context,
    pub state: TaskState,
    /// Top of the task's static stack (informational; `context.sp` is
    /// what actually drives execution).
    pub stack_top: usize,
    pub name: &'static str,
    /// The `satp` (address space) to load when this task runs. Kernel tasks
    /// carry the master kernel `satp`; U-mode tasks carry their private one.
    pub satp: usize,
}
```

- [ ] **Step 2: Update the `sched` test helper for the new field**

In `arch/riscv64/src/sched.rs`, the `tests` module has:

```rust
    fn task(name: &'static str, state: TaskState) -> Task {
        Task { context: Context::zeroed(), state, stack_top: 0, name }
    }
```

Change it to:

```rust
    fn task(name: &'static str, state: TaskState) -> Task {
        Task { context: Context::zeroed(), state, stack_top: 0, name, satp: 0 }
    }
```

Run: `cargo test -p kernel-arch-riscv64` → expect all green (the existing `pick_next`/`Exited` tests compile again with the new field).

- [ ] **Step 3: Set `satp` in `spawn` and `spawn_user`**

In `spawn` (kernel task), set the kernel `satp` in the `Task { ... }`:

```rust
        s.tasks[slot] = Some(Task {
            context,
            state: TaskState::Ready,
            stack_top,
            name,
            satp: crate::mem::kernel_satp(),
        });
```

Change `spawn_user`'s signature to take the private `satp` and store it. New signature + tail:

```rust
#[cfg(target_arch = "riscv64")]
pub fn spawn_user(
    name: &'static str,
    entry: extern "C" fn() -> !,
    user_sp: usize,
    kstack_top: usize,
    satp: usize,
) {
    SCHED.with(|s| {
        let slot = s
            .tasks
            .iter()
            .position(Option::is_none)
            .expect("scheduler full");
        let context = crate::task::forge_user_context(
            user_trampoline as *const () as usize,
            entry as *const () as usize,
            user_sp,
            kstack_top,
            crate::task::user_sstatus(crate::csr::sstatus_read()),
        );
        s.tasks[slot] = Some(Task {
            context,
            state: TaskState::Ready,
            stack_top: kstack_top,
            name,
            satp,
        });
    });
}
```

- [ ] **Step 4: Swap `satp` before each `switch_context`**

In `enter`, capture and load the first task's `satp`. Replace the `SCHED.with(...)` block and the switch:

```rust
    let (first, first_satp) = SCHED.with(|s| {
        s.current = 0;
        let t = s.tasks[0].as_mut().expect("enter() with no spawned task");
        t.state = TaskState::Running;
        (core::ptr::addr_of!(t.context), t.satp)
    });

    let mut throwaway = Context::zeroed();
    // SAFETY: switch into the first task's address space (the kernel is
    // mapped in it), then into its context. `first` addresses a Context in
    // the 'static SCHED; the throwaway is a never-restored save target.
    unsafe {
        crate::csr::satp_write(first_satp);
        switch_context(core::ptr::addr_of_mut!(throwaway), first);
    }
    unreachable!("enter() never returns to the bootstrap context");
```

In `yield_now`, add the next task's `satp` to the returned tuple and load it before the switch. The `SCHED.with` closure's `Some(...)` and the switch block become:

```rust
    let switch = SCHED.with(|s| {
        let current = s.current;
        let next = s.pick_next();
        if next == current {
            return None;
        }
        s.tasks[current].as_mut().unwrap().state = TaskState::Ready;
        s.tasks[next].as_mut().unwrap().state = TaskState::Running;
        s.current = next;
        let old = core::ptr::addr_of_mut!(s.tasks[current].as_mut().unwrap().context);
        let new = core::ptr::addr_of!(s.tasks[next].as_ref().unwrap().context);
        let next_satp = s.tasks[next].as_ref().unwrap().satp;
        Some((old, new, next_satp))
    });

    if let Some((old, new, next_satp)) = switch {
        // SAFETY: both pointers address distinct 'static Contexts in SCHED;
        // single hart + interrupts disabled above mean no aliasing. The
        // next task's tree maps every kernel address (map_kernel_sections),
        // so switching satp before the switch is seamless. Execution resumes
        // here when this task is picked again.
        unsafe {
            crate::csr::satp_write(next_satp);
            switch_context(old, new);
        }
    }
```

In `exit_current`, add `next_satp` to the tuple and load it before the switch. The `SCHED.with` tail and the switch:

```rust
        s.current = next;
        // Save into the dying slot's own context (never resumed).
        let old = core::ptr::addr_of_mut!(s.tasks[current].as_mut().unwrap().context);
        let new = core::ptr::addr_of!(s.tasks[next].as_ref().unwrap().context);
        let next_satp = s.tasks[next].as_ref().unwrap().satp;
        (old, new, next_satp)
    });
    // SAFETY: the `assert_ne!` above guarantees `old` and `new` are distinct
    // slots, so they never alias; both are 'static contexts in SCHED. The
    // dying (old) slot is never run again. The next task's tree maps every
    // kernel address, so the satp switch is seamless. Control resumes in
    // `next`.
    unsafe {
        crate::csr::satp_write(switch.2);
        switch_context(switch.0, switch.1);
    }
    unreachable!("exit_current resumes in the next task, never here")
```

(The `let switch = SCHED.with(|s| { ... (old, new, next_satp) });` now returns a 3-tuple; use `switch.0/.1/.2`.)

- [ ] **Step 5: Verify the arch crate builds and host tests pass**

Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: SUCCESS. (The kernel *binary* will NOT build yet — `main.rs` still calls `spawn_user` with 4 args; fixed in Task 5. Build only the arch crate here.)

Run: `cargo test -p kernel-arch-riscv64`
Expected: all green.

- [ ] **Step 6: Commit**

```bash
git add arch/riscv64/src/task.rs arch/riscv64/src/sched.rs
git commit -m "feat(sched): per-task satp carried on Task and swapped before every switch_context"
```

---

## Task 4: update the smoke test to the 3b-ii milestones (red)

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Replace the milestone patterns**

In `tools/test-qemu.ps1`, replace the `$mustMatch = @( ... )` array with:

```powershell
$mustMatch = @(
    "hello world",
    "trap: breakpoint",
    "survived breakpoint",
    "paging: sv39 on",
    "wx: rodata write blocked",
    "frames: alloc/free ok",
    "(?s)user: ping.*user: pong.*user: ping.*user: pong",
    "sched: task 'ping' exited \(code 0\)",
    "sched: task 'pong' exited \(code 0\)",
    "sched: task 'snoop' killed by LoadPageFault",
    "sched: U-mode task preempted by timer",
    "tick: 2(?!\d)"
)
```

Also update the header comment block and the two result messages to describe the 3b-ii milestone: "the Phase 3b-ii milestone — each U-mode task runs in its own address space (its own satp), the 3b-i run-queue proofs still hold, and a snoop task that reaches into another task's memory is contained (cross-task isolation)."

- [ ] **Step 2: Run the smoke test — expect RED**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: FAIL — the kernel binary still has the 3b-i demo (and currently does not even build because `main.rs` calls the old 4-arg `spawn_user`). Either the build fails or the `snoop` line is missing. RED is the expected state for this commit.

- [ ] **Step 3: Commit (red test pinned)**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke test asserts 3b-ii address-space isolation (red)"
```

---

## Task 5: kmain — per-task address spaces + the snoop isolation proof (green)

**Files:**
- Modify: `kernel/src/main.rs`

Context: `main.rs` currently spawns ping/pong/hog/bad (U-mode) + idle, with stack statics `KS_*`/`US_*` and message statics `*_MSG`, and `sys_print`/`sys_exit`/`sys_yield` stubs. You will: page-align the user statics, give each U-mode task its own data page, replace `bad` with `snoop`, build a private address space per task, and pass each task's `satp` to the new 5-arg `spawn_user`.

- [ ] **Step 1: Replace the spawn block in `kmain`**

Replace the current spawn block (the `sched::spawn_user(...)` calls through `sched::enter()`) with:

```rust
        // Phase 3b-ii: each U-mode task runs in its OWN address space. We
        // build a private page table per task (kernel cloned in, shared
        // .user_text, plus only that task's stack + data page), then spawn
        // it with that satp. ping/pong cooperate via yield and exit; hog is
        // preempted; snoop follows a pointer (in its own page) into pong's
        // data page — unmapped in snoop's space — and is contained. idle is
        // a kernel task on the master satp. Built here on the master satp
        // (new page-table frames come from free RAM, which only it maps).
        use core::mem::size_of;
        let region = |base: usize, size: usize| (base, base + size);

        let us_ping = region(core::ptr::addr_of!(US_PING) as usize, size_of::<UStack>());
        let ud_ping = region(core::ptr::addr_of!(UD_PING) as usize, size_of::<UData>());
        let ping_satp = mem::build_user_space(us_ping, ud_ping);
        sched::spawn_user("ping", user_ping, us_ping.1,
            core::ptr::addr_of!(KS_PING) as usize + TASK_STACK, ping_satp);

        let us_pong = region(core::ptr::addr_of!(US_PONG) as usize, size_of::<UStack>());
        let ud_pong = region(core::ptr::addr_of!(UD_PONG) as usize, size_of::<UData>());
        let pong_satp = mem::build_user_space(us_pong, ud_pong);
        sched::spawn_user("pong", user_pong, us_pong.1,
            core::ptr::addr_of!(KS_PONG) as usize + TASK_STACK, pong_satp);

        let us_hog = region(core::ptr::addr_of!(US_HOG) as usize, size_of::<UStack>());
        let ud_hog = region(core::ptr::addr_of!(UD_HOG) as usize, size_of::<UData>());
        let hog_satp = mem::build_user_space(us_hog, ud_hog);
        sched::spawn_user("hog", user_hog, us_hog.1,
            core::ptr::addr_of!(KS_HOG) as usize + TASK_STACK, hog_satp);

        // snoop's data page holds a pointer to pong's data page; snoop's
        // tree maps SNOOP_TARGET but NOT pong's page, so following it faults.
        let us_snoop = region(core::ptr::addr_of!(US_SNOOP) as usize, size_of::<UStack>());
        let snoop_data = region(core::ptr::addr_of!(SNOOP_TARGET) as usize, size_of::<Snoop>());
        let snoop_satp = mem::build_user_space(us_snoop, snoop_data);
        sched::spawn_user("snoop", user_snoop, us_snoop.1,
            core::ptr::addr_of!(KS_SNOOP) as usize + TASK_STACK, snoop_satp);

        sched::spawn("idle", idle, core::ptr::addr_of!(KS_IDLE) as usize + TASK_STACK);

        timer::start();
        println!("(scheduler starting; heartbeat ~1/s; exit QEMU with Ctrl-A then X)");
        sched::enter()
```

- [ ] **Step 2: Replace the stack/message statics with page-aligned ones**

Replace the existing `TASK_STACK`/`KStack`/`KS_*`/`USER_STACK_SIZE`/`UStack`/`US_*`/`*_MSG` block with:

```rust
    /// Per-task kernel stack size (also the trap stack a U-mode task's
    /// traps land on). 16 KiB; per-task guard pages stay deferred.
    const TASK_STACK: usize = 16 * 1024;
    type KStack = [u8; TASK_STACK];

    /// One kernel/trap stack per U-mode task, plus one for the idle task.
    /// In .bss → mapped global by map_kernel_sections, so a task traps onto
    /// its own kernel stack in its own address space.
    static mut KS_PING: KStack = [0; TASK_STACK];
    static mut KS_PONG: KStack = [0; TASK_STACK];
    static mut KS_HOG: KStack = [0; TASK_STACK];
    static mut KS_SNOOP: KStack = [0; TASK_STACK];
    static mut KS_IDLE: KStack = [0; TASK_STACK];

    /// A page-aligned U-mode stack (2 pages). Page alignment is required so
    /// each task's stack occupies its OWN pages — the unit of isolation.
    const USER_STACK_SIZE: usize = 8 * 1024;
    #[repr(C, align(4096))]
    struct UStack([u8; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_PING: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_PONG: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_HOG: UStack = UStack([0; USER_STACK_SIZE]);
    #[link_section = ".user_data"]
    static mut US_SNOOP: UStack = UStack([0; USER_STACK_SIZE]);

    /// A page-aligned 4 KiB U-mode data page. Each task gets its own so its
    /// data is page-isolated from every other task. The message bytes sit
    /// at the start; the rest is zero.
    #[repr(C, align(4096))]
    struct UData([u8; 4096]);

    /// Fill a 4 KiB page with `prefix` at the front, zero after (const so
    /// the page is a link-time constant in .user_data, not a runtime write).
    const fn page_with(prefix: &[u8]) -> [u8; 4096] {
        let mut p = [0u8; 4096];
        let mut i = 0;
        while i < prefix.len() {
            p[i] = prefix[i];
            i += 1;
        }
        p
    }

    #[link_section = ".user_data"]
    static UD_PING: UData = UData(page_with(b"user: ping\n"));
    #[link_section = ".user_data"]
    static UD_PONG: UData = UData(page_with(b"user: pong\n"));
    #[link_section = ".user_data"]
    static UD_HOG: UData = UData(page_with(b"user: hog\n"));

    /// Length of each task's message (bytes to print): "user: ping\n" and
    /// "user: pong\n" are 11; "user: hog\n" is 10.
    const PING_LEN: usize = 11;
    const PONG_LEN: usize = 11;
    const HOG_LEN: usize = 10;

    /// A page-aligned page holding a pointer into pong's data page. Mapped
    /// into snoop's address space (R-U); pong's page is NOT — so snoop can
    /// load the pointer from its own page but faults when it dereferences.
    #[repr(C, align(4096))]
    struct Snoop(&'static u8);
    #[link_section = ".user_data"]
    static SNOOP_TARGET: Snoop = Snoop(&UD_PONG.0[0]);
```

- [ ] **Step 3: Keep `sys_print`/`sys_exit`/`sys_yield`; replace the task functions**

Keep the three syscall stubs unchanged. Remove `user_good`/`user_bad` (whichever 3b-i names remain — the current file has `user_ping`, `user_pong`, `user_hog`, `user_bad`) and replace the U-mode task fns with these (note `user_bad` → `user_snoop`):

```rust
    /// Cooperating U-mode task: print its own data page twice, yielding to
    /// its peer between each, then exit cleanly. In its own address space.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_ping() -> ! {
        // SAFETY: UD_PING is mapped R-U in this task's space; the kernel
        // validates the pointer (it lies in .user_data) and reads it.
        unsafe {
            sys_print(core::ptr::addr_of!(UD_PING) as *const u8, PING_LEN);
            sys_yield();
            sys_print(core::ptr::addr_of!(UD_PING) as *const u8, PING_LEN);
            sys_yield();
            sys_exit(0)
        }
    }

    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_pong() -> ! {
        // SAFETY: see `user_ping`.
        unsafe {
            sys_print(core::ptr::addr_of!(UD_PONG) as *const u8, PONG_LEN);
            sys_yield();
            sys_print(core::ptr::addr_of!(UD_PONG) as *const u8, PONG_LEN);
            sys_yield();
            sys_exit(0)
        }
    }

    /// The hog: two cooperative rounds, then spin forever without yielding.
    /// The timer preempts it (proving a U-mode task is preemptible, now
    /// across an address-space switch).
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_hog() -> ! {
        // SAFETY: see `user_ping`.
        unsafe {
            sys_print(core::ptr::addr_of!(UD_HOG) as *const u8, HOG_LEN);
            sys_yield();
            sys_print(core::ptr::addr_of!(UD_HOG) as *const u8, HOG_LEN);
            sys_yield();
        }
        loop {
            core::hint::spin_loop();
        }
    }

    /// The cross-task snooper: follow a pointer (kept in our OWN mapped
    /// page) into another task's data page. That page is not mapped in our
    /// address space, so the load faults in U-mode and the kernel contains
    /// us while the others keep running — the isolation proof.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn user_snoop() -> ! {
        let target: *const u8 = SNOOP_TARGET.0;
        let _v: u8;
        // SAFETY: deliberate cross-task isolation probe. `lb` (not
        // read_volatile, which a debug build may turn into a jalr into
        // kernel text) faults in U-mode before completing; control never
        // returns here.
        unsafe {
            core::arch::asm!("lb {v}, 0({p})", v = out(reg) _v, p = in(reg) target, options(nostack));
            sys_exit(0) // unreachable: the load above faults first
        }
    }

    /// The idle kernel (S-mode) task: cooperatively yield when other tasks
    /// are ready; `wfi`-sleep when alone. Runs on the master kernel satp;
    /// never exits, so it is always a valid `exit_current` successor.
    extern "C" fn idle() -> ! {
        loop {
            sched::yield_now();
            // SAFETY: wfi just waits for the next interrupt (the timer).
            unsafe { core::arch::asm!("wfi") };
        }
    }
```

(If the current file's `idle` is already identical, leave it; only the U-mode task fns and the `bad`→`snoop` swap need changing.)

- [ ] **Step 4: Build the kernel binary**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: SUCCESS. If you hit `static_mut_refs` it means a `&` slipped in where `addr_of!` is needed — use `core::ptr::addr_of!`. If a `page_with`/const error appears, confirm `page_with` is a `const fn` and message literals are ≤ 4096 bytes.

- [ ] **Step 5: Run the smoke test — expect GREEN**

Run (PowerShell): `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS` — all 12 patterns within 30 s, including the ping/pong interleave (each printed twice), both clean exits, the `snoop` containment, and the U-mode preemption line. If it FAILS, read the dumped serial output and diagnose. Likely culprits: a region not page-aligned (a `map_page: unaligned` panic → check `#[repr(C, align(4096))]`), the snoop reading the wrong thing (should fault as LoadPageFault), or a message length mismatch. Do NOT weaken the smoke-test patterns — fix the demo.

- [ ] **Step 6: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat: Phase 3b-ii live - per-task address spaces and cross-task isolation (snoop contained)"
```

---

## Task 6: docs — short learning note, roadmap, glossary; final verification

**Files:**
- Create: `docs/learning/0008-address-space-isolation.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`
- Modify: `docs/learning/README.md` (if it indexes notes)

- [ ] **Step 1: Write a short learning note**

Create `docs/learning/0008-address-space-isolation.md` (brief summary, per project preference):

```markdown
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
```

- [ ] **Step 2: Update the roadmap**

In `docs/roadmap/roadmap.md`, mark 3b-ii done. Find the `#### Phase 3b-ii — Per-address-space isolation` block and replace it with:

```markdown
#### Phase 3b-ii — Per-address-space isolation  *(done — 2026-06-19)*

- **Goal:** each component gets its own `satp`; the kernel is cloned into
  every address space; `satp` swaps on context switch.
- **You learn:** that `PTE_G` is only a TLB hint, building per-task page
  tables, swapping `satp` in the context switch (see
  [learning note 0008](../learning/0008-address-space-isolation.md)).
- **Done when:** `./tools/test-qemu.ps1` observes the 3b-i run-queue proofs
  (now each under its own `satp`) plus a `snoop` task that reaches into
  another task's memory being contained — all in one boot.
```

- [ ] **Step 3: Add glossary entries for the genuinely new terms**

In `docs/glossary.md`, add entries (in the file's format/order) for: **address space** (the set of virtual→physical mappings a task sees, selected by its `satp`; each U-mode task has its own), **`satp`** (the CSR naming the active root page table + mode; written on every context switch), and **the global bit (`PTE_G`)** (a TLB hint that a mapping is the same in every address space — it still must physically exist in each tree). Reuse existing paging/Sv39 entries; don't duplicate.

- [ ] **Step 4: Final verification (run all, paste results)**

Run: `cargo test -p kernel-arch-riscv64` → expect all green.
Run: `cargo build --workspace` → expect success (host stub + libs).
Run (PowerShell): `./tools/check-references.ps1` → expect PASS (new learning-note + roadmap links resolve; fix any of YOUR broken references).
Run (PowerShell): `./tools/test-qemu.ps1` → expect `BOOT TEST PASS`.

- [ ] **Step 5: Commit**

```bash
git add docs/learning/0008-address-space-isolation.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: Phase 3b-ii learning note, roadmap, glossary terms"
```

---

## Done-when checklist (maps to spec §1)

- [ ] The 3b-i run-queue proofs hold under per-task `satp` — interleave `(?s)user: ping.*user: pong.*user: ping.*user: pong` + two `exited (code 0)` lines + `U-mode task preempted by timer`.
- [ ] Cross-task isolation — `sched: task 'snoop' killed by LoadPageFault`, with the other tasks/milestones still printing (scheduler survives).
- [ ] `make_satp` host-tested; all host tests green; `check-references` clean; `BOOT TEST PASS`.
```
