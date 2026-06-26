# Phase 7 — Write-back & the learning organism — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the filesystem an append-only write path and let the self-healer record a newly-seen crash class to disk, so a *second boot of the same image* loads that entry and diagnoses the formerly-novel crash — the organism learns across reboots.

**Architecture:** Pure, host-tested format logic (`fs::append_plan`, `kb::serialize`) defines the write; the kernel orchestrates block writes through the existing blk call/reply server (a direction bit in the IPC badge picks read vs write); a kernel KB-writer task drains a novel-cause mailbox the crash path fills, appends the new entry, and installs it. The on-disk format is unchanged (superblock + one directory block + contiguous extents); the superblock write is the single commit point.

**Tech Stack:** Rust `no_std`/no-alloc kernel (`arch/riscv64`, `kernel`), pure shared lib `kernel-common` (`libs/common`, host-tested), host tool `mkfs`, QEMU `virt` riscv64, PowerShell boot harness.

**Spec:** `docs/superpowers/specs/2026-06-26-phase-7-write-back-learning-organism-design.md`

## Global Constraints

- **Commits:** Conventional Commits. NO Claude co-author trailer. Author identity is Kathir (`kathirpsmy@gmail.com`); signing is automated. (memory: git-commit-conventions)
- **`kernel-common` is `no_std`, no-alloc, no I/O** — pure logic only; everything in it is host-tested under `#[cfg(test)]`.
- **Host unit tests:** `cargo test -p kernel-common` and `cargo test -p mkfs` (run from repo root; these build for the host, not the target).
- **Boot/integration test:** `./tools/test-qemu.ps1` (QEMU riscv64; needs `-device virtio-rng-device` and the writable `-drive`, both already wired).
- **Kernel build check:** `./tools/build.ps1` (or `cargo build` for the target per the workspace config).
- **U-mode task code** lives in `#[link_section = ".user_text"]`, must not call into kernel `.text`/`.rodata` (use inline asm for faults/MMIO), and reports only via syscalls. New patient tasks follow the existing `flaky_task`/`transient_task` pattern exactly.
- **The crash path (`exit_current`) runs with interrupts off** — no I/O, no blocking there. All disk work happens in a task with interrupts on (like 6c's loader).
- **Do NOT commit a `knowledge-base/entries/KB-0006.md`.** The novel-ness of the crash depends on KB-0006 being absent at first boot and *written by the organism*. `mkfs` packs only committed matchable entries (KB-0005 today); KB-0006 must exist only on the post-boot-1 disk image.
- **Verify cross-references** before citing a path/KB id in docs (memory: verify-cross-references; `tools/check-references.ps1`).

---

## File Structure

- `libs/common/src/fs.rs` — **add** `SPARE_BLOCKS`, `AppendPlan`, `append_plan(...)`. Pure.
- `libs/common/src/kb.rs` — **add** `serialize(...)` (inverse of `parse`). Pure.
- `tools/mkfs/src/lib.rs` — **modify** `build_image` to pad the image with `SPARE_BLOCKS` slack.
- `arch/riscv64/src/trap.rs` — **add** `Cause::IllegalInstruction` (scause 2), decode arm, U-mode containment arm.
- `arch/riscv64/src/heal.rs` — **add** the `IllegalInstruction → "illegal-instruction"` token mapping, the novel-cause mailbox (`note_unmatched` / `take_novel`), and `max_kb_number()`.
- `arch/riscv64/src/sched.rs` — **modify** the `Killed` `None` branch to call `heal::note_unmatched(cause)`.
- `kernel/src/main.rs` — **add** the blk-server write path (badge direction bit), `fs_write_block`, `fs_append_file`, the KB-writer task, the novel patient, and boot wiring (spawn + gate release count).
- `tools/test-qemu.ps1` — **modify** into a two-boot run (refactor the run/match block into a helper invoked twice).
- `docs/learning/0025-write-back-learning-organism.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md` — **docs**.

---

## Task 1: `fs::append_plan` — pure append placement

**Files:**
- Modify: `libs/common/src/fs.rs`
- Test: `libs/common/src/fs.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: existing `Superblock`, `DirEntry`, `BLOCK_SIZE`, `DIR_BLOCK`, `DIRENT_SIZE`, `DIRENTS_PER_BLOCK`, `block_count`.
- Produces:
  - `pub const SPARE_BLOCKS: u32 = 8;`
  - `pub struct AppendPlan { pub start_block: u32, pub new_superblock: Superblock, pub dir_block: [u8; BLOCK_SIZE] }`
  - `pub fn append_plan(sb: &Superblock, dir_bytes: &[u8], name: &str, byte_len: u32) -> Option<AppendPlan>`

- [ ] **Step 1: Write the failing tests** (append to the `tests` module in `libs/common/src/fs.rs`)

```rust
    #[test]
    fn append_plan_places_a_new_file_at_end_of_volume() {
        // One file already present; directory has 1 entry; volume used 3 blocks.
        let sb = Superblock {
            magic: FS_MAGIC, version: FS_VERSION, block_size: BLOCK_SIZE as u32,
            dir_block: DIR_BLOCK, dir_entries: 1, total_blocks: 3,
        };
        let mut dir = [0u8; BLOCK_SIZE];
        DirEntry::new("KB-0005", 2, 400).encode(&mut dir[0..DIRENT_SIZE]);

        let plan = append_plan(&sb, &dir, "KB-0006", 200).expect("appends");
        // new file lands at the current end of the volume
        assert_eq!(plan.start_block, 3);
        // superblock grows by one entry and by ceil(200/512)=1 block
        assert_eq!(plan.new_superblock.dir_entries, 2);
        assert_eq!(plan.new_superblock.total_blocks, 4);
        // the new dirent is the second entry and round-trips
        let e = DirEntry::decode(&plan.dir_block[DIRENT_SIZE..2 * DIRENT_SIZE]);
        assert!(e.name_is("KB-0006"));
        assert_eq!(e.start_block, 3);
        assert_eq!(e.byte_len, 200);
        // the existing entry is preserved
        let e0 = DirEntry::decode(&plan.dir_block[0..DIRENT_SIZE]);
        assert!(e0.name_is("KB-0005"));
    }

    #[test]
    fn append_plan_refuses_when_directory_block_is_full() {
        let sb = Superblock {
            magic: FS_MAGIC, version: FS_VERSION, block_size: BLOCK_SIZE as u32,
            dir_block: DIR_BLOCK, dir_entries: DIRENTS_PER_BLOCK as u32, total_blocks: 50,
        };
        let dir = [0u8; BLOCK_SIZE];
        assert!(append_plan(&sb, &dir, "KB-0099", 10).is_none());
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p kernel-common append_plan`
Expected: FAIL — `cannot find function append_plan`.

- [ ] **Step 3: Implement `append_plan`** (add to `libs/common/src/fs.rs`, after `lookup`)

```rust
/// Spare blocks `mkfs` pads the image with past `total_blocks`, so the in-kernel
/// writer has device capacity to append entries into (free space, as a real FS
/// keeps). Capacity overflow is otherwise caught safely at write time by the
/// device status, so `append_plan` does not itself bound on capacity.
pub const SPARE_BLOCKS: u32 = 8;

/// The result of planning an append: where the new file's data goes, the
/// updated directory block (existing entries plus the new one), and the updated
/// superblock. Pure — the caller performs the actual block writes.
#[derive(Debug, Clone, Copy)]
pub struct AppendPlan {
    pub start_block: u32,
    pub new_superblock: Superblock,
    pub dir_block: [u8; BLOCK_SIZE],
}

/// Plan appending a `byte_len`-byte file named `name` to the volume described by
/// `sb` + `dir_bytes`. The file is placed at the current end of the volume; one
/// directory entry is appended. Returns `None` if the single directory block is
/// already full (the format keeps one directory block).
pub fn append_plan(sb: &Superblock, dir_bytes: &[u8], name: &str, byte_len: u32) -> Option<AppendPlan> {
    let count = sb.dir_entries as usize;
    if count >= DIRENTS_PER_BLOCK {
        return None; // one directory block only
    }
    let start_block = sb.total_blocks;
    let nblocks = block_count(byte_len);

    let mut dir_block = [0u8; BLOCK_SIZE];
    let copy = core::cmp::min(dir_bytes.len(), BLOCK_SIZE);
    dir_block[..copy].copy_from_slice(&dir_bytes[..copy]);
    let off = count * DIRENT_SIZE;
    DirEntry::new(name, start_block, byte_len).encode(&mut dir_block[off..off + DIRENT_SIZE]);

    let new_superblock = Superblock {
        dir_entries: sb.dir_entries + 1,
        total_blocks: sb.total_blocks + nblocks,
        ..*sb
    };
    Some(AppendPlan { start_block, new_superblock, dir_block })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p kernel-common append_plan`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add libs/common/src/fs.rs
git commit -m "feat(fs): append_plan — pure placement for an appended file"
```

---

## Task 2: `kb::serialize` — emit a frontmatter entry `parse` round-trips

**Files:**
- Modify: `libs/common/src/kb.rs`
- Test: `libs/common/src/kb.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: existing `parse`, `KbRecord`.
- Produces: `pub fn serialize(id: &str, title: &str, playbook: &str, match_cause: &str, out: &mut [u8]) -> Option<usize>` — writes the document into `out`, returns its byte length, or `None` if `out` is too small.

- [ ] **Step 1: Write the failing test** (add to the `tests` module in `libs/common/src/kb.rs`)

```rust
    #[test]
    fn serialize_round_trips_through_parse() {
        let mut buf = [0u8; 512];
        let n = serialize(
            "KB-0006",
            "Observed fault: illegal-instruction (auto-recorded)",
            "Restart the component, up to a bounded number of retries.",
            "illegal-instruction",
            &mut buf,
        )
        .expect("serializes within the buffer");
        let r = parse(&buf[..n]).expect("the emitted document parses");
        assert_eq!(r.id, "KB-0006");
        assert_eq!(r.title, "Observed fault: illegal-instruction (auto-recorded)");
        assert_eq!(r.playbook, "Restart the component, up to a bounded number of retries.");
        assert_eq!(r.match_cause, Some("illegal-instruction"));
    }

    #[test]
    fn serialize_reports_none_when_buffer_too_small() {
        let mut tiny = [0u8; 8];
        assert!(serialize("KB-0006", "t", "p", "illegal-instruction", &mut tiny).is_none());
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p kernel-common serialize`
Expected: FAIL — `cannot find function serialize`.

- [ ] **Step 3: Implement `serialize`** (add to `libs/common/src/kb.rs`, after `parse`)

```rust
/// Emit a KB entry document that `parse` round-trips, into `out`. Returns the
/// byte length written, or `None` if `out` is too small. The inverse of
/// `parse` for the runtime fields — so what the self-healer writes to disk is
/// provably what a later boot reads back. Strings are emitted double-quoted
/// (matching the schema and what `parse`'s `clean` strips); callers pass
/// already-sane ASCII values (no embedded quotes/newlines).
pub fn serialize(id: &str, title: &str, playbook: &str, match_cause: &str, out: &mut [u8]) -> Option<usize> {
    let mut n = 0usize;
    let mut put = |s: &str| -> Option<()> {
        let b = s.as_bytes();
        if n + b.len() > out.len() {
            return None;
        }
        out[n..n + b.len()].copy_from_slice(b);
        n += b.len();
        Some(())
    };
    put("---\n")?;
    put("id: ")?; put(id)?; put("\n")?;
    put("title: \"")?; put(title)?; put("\"\n")?;
    put("match-cause: ")?; put(match_cause)?; put("\n")?;
    put("playbook:\n")?;
    put("  - \"")?; put(playbook)?; put("\"\n")?;
    put("---\n")?;
    Some(n)
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p kernel-common serialize`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add libs/common/src/kb.rs
git commit -m "feat(kb): serialize — inverse of parse, for organism write-back"
```

---

## Task 3: `Cause::IllegalInstruction` — decode, containment, token

**Files:**
- Modify: `arch/riscv64/src/trap.rs` (enum, `decode`, the U-mode containment match)
- Modify: `arch/riscv64/src/heal.rs` (`cause_token`)
- Test: `arch/riscv64/src/trap.rs` and `arch/riscv64/src/heal.rs` test modules

**Interfaces:**
- Consumes: existing `Cause`, `decode`, `from_user`, `exit_current`, `heal::cause_token`.
- Produces: `Cause::IllegalInstruction` variant; `cause_token(Cause::IllegalInstruction) == Some("illegal-instruction")`.

- [ ] **Step 1: Write the failing tests**

In `arch/riscv64/src/trap.rs` tests (near the existing `decode` tests, e.g. after line 152):

```rust
    #[test]
    fn decodes_illegal_instruction() {
        assert_eq!(decode(2), Cause::IllegalInstruction);
    }
```

In `arch/riscv64/src/heal.rs` tests (extend `page_faults_map_to_the_page_fault_token` or add):

```rust
    #[test]
    fn illegal_instruction_maps_to_its_token() {
        assert_eq!(cause_token(Cause::IllegalInstruction), Some("illegal-instruction"));
    }
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p riscv64 decodes_illegal_instruction illegal_instruction_maps`
Expected: FAIL — no `IllegalInstruction` variant.
(If the crate name differs, use the arch crate's package name from `arch/riscv64/Cargo.toml`.)

- [ ] **Step 3: Implement**

In `arch/riscv64/src/trap.rs`, add the variant to `enum Cause` (after `Breakpoint`, before `InstructionPageFault`):

```rust
    /// Illegal instruction executed (exception code 2) — e.g. a U-mode task
    /// that ran `unimp`. A distinct fault class from a page fault.
    IllegalInstruction,
```

Add the decode arm in `decode` (with the other `(false, _)` arms):

```rust
        (false, 2) => Cause::IllegalInstruction,
```

Add U-mode containment. Put this arm alongside the existing `InstructionPageFault | LoadPageFault if from_user(frame)` arm (around line 428):

```rust
        Cause::IllegalInstruction if from_user(frame) => {
            // A U-mode task executed an illegal instruction: contain it, just
            // like a page fault — a different symptom class for the organism.
            crate::sched::exit_current(crate::task::ExitReason::Killed(cause));
        }
        Cause::IllegalInstruction => fatal("illegal instruction", frame),
```

In `arch/riscv64/src/heal.rs`, extend `cause_token`:

```rust
fn cause_token(cause: Cause) -> Option<&'static str> {
    match cause {
        Cause::LoadPageFault | Cause::StorePageFault | Cause::InstructionPageFault => {
            Some("page-fault")
        }
        Cause::IllegalInstruction => Some("illegal-instruction"),
        _ => None,
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p riscv64 decodes_illegal_instruction illegal_instruction_maps`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/trap.rs arch/riscv64/src/heal.rs
git commit -m "feat(trap): IllegalInstruction cause — decode, U-mode containment, token"
```

---

## Task 4: The novel-cause mailbox + `max_kb_number` in `heal`

**Files:**
- Modify: `arch/riscv64/src/heal.rs`
- Modify: `arch/riscv64/src/sched.rs` (the `Killed` `None` branch)
- Test: `arch/riscv64/src/heal.rs` test module

**Interfaces:**
- Produces:
  - `pub fn note_unmatched(cause: Cause)` — if `cause` has a token but no installed entry, latch the token into a single-slot mailbox.
  - `pub fn take_novel() -> Option<&'static str>` — drain the mailbox.
  - `pub fn max_kb_number() -> u32` — the largest `KB-NNNN` number installed (0 if none).
- Consumes (Task 5): the writer calls `take_novel`, `max_kb_number`, `install`.

- [ ] **Step 1: Write the failing test** (in `arch/riscv64/src/heal.rs` tests)

```rust
    #[test]
    fn max_kb_number_reads_the_largest_installed_id() {
        // install() mutates process-global state; this test runs in isolation.
        assert_eq!(max_kb_number(), 0);
        assert!(install("KB-0005", "t", "p", Some("page-fault")));
        assert!(install("KB-0003", "t", "p", Some("x")));
        assert_eq!(max_kb_number(), 5);
    }
```

(Note: `note_unmatched`/`take_novel` touch the same `static mut` table and a mailbox; they are exercised end-to-end by the boot test in Task 8 rather than unit-tested, to avoid cross-test global-state coupling. `max_kb_number` is the pure-enough piece worth a unit test.)

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p riscv64 max_kb_number`
Expected: FAIL — `cannot find function max_kb_number`.

- [ ] **Step 3: Implement** (in `arch/riscv64/src/heal.rs`)

Add the mailbox statics near `KB_TABLE`:

```rust
/// A single pending novel cause token, latched by the crash path
/// (`note_unmatched`) and drained by the KB-writer task (`take_novel`). One
/// slot is enough: a token is recorded at most once (once installed it matches,
/// so `note_unmatched` never re-latches it).
static mut NOVEL_TOKEN: Option<&'static str> = None;
```

Add the functions:

```rust
/// Called from the crash path when `diagnose` found no match. If the kernel can
/// still *name* the cause (it has a token) but no entry is installed for it, the
/// crash is novel-but-recognizable: latch the token for the KB-writer to record.
/// Pure aside from the single-slot latch; safe in the interrupts-off crash path
/// (no I/O, no blocking).
pub fn note_unmatched(cause: Cause) {
    if let Some(token) = cause_token(cause) {
        // SAFETY: single hart; crash path is not re-entrant. Don't clobber an
        // un-drained token.
        unsafe {
            if core::ptr::read(core::ptr::addr_of!(NOVEL_TOKEN)).is_none() {
                core::ptr::write(core::ptr::addr_of_mut!(NOVEL_TOKEN), Some(token));
            }
        }
    }
}

/// Drain the pending novel token, if any. Called by the KB-writer task.
pub fn take_novel() -> Option<&'static str> {
    // SAFETY: single hart; the writer task is the only drainer.
    unsafe {
        let t = core::ptr::read(core::ptr::addr_of!(NOVEL_TOKEN));
        core::ptr::write(core::ptr::addr_of_mut!(NOVEL_TOKEN), None);
        t
    }
}

/// The largest `KB-NNNN` number installed in the runtime table (0 if none) — so
/// the writer can mint the next id deterministically.
pub fn max_kb_number() -> u32 {
    // SAFETY: single hart; read of the boot-populated table.
    let table = unsafe { &*core::ptr::addr_of!(KB_TABLE) };
    let mut max = 0u32;
    for issue in table.iter().flatten() {
        let id = issue.id();
        if let Some(num) = id.strip_prefix("KB-").and_then(|d| d.parse::<u32>().ok()) {
            if num > max {
                max = num;
            }
        }
    }
    max
}
```

In `arch/riscv64/src/sched.rs`, change the `None` arm of the diagnosis match (around line 426) from:

```rust
                    None => crate::println!("heal: no known issue for {cause:?} (recorded for triage)"),
```

to:

```rust
                    None => {
                        crate::println!("heal: no known issue for {cause:?} (recording for write-back)");
                        // Phase 7: latch the cause for the KB-writer to record
                        // to disk, so a later boot recognizes it.
                        crate::heal::note_unmatched(cause);
                    }
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p riscv64 max_kb_number`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/heal.rs arch/riscv64/src/sched.rs
git commit -m "feat(heal): novel-cause mailbox + next-id, latched from the crash path"
```

---

## Task 5: `mkfs` spare-capacity padding

**Files:**
- Modify: `tools/mkfs/src/lib.rs`
- Test: `tools/mkfs/src/lib.rs` (`#[cfg(test)] mod tests`)

**Interfaces:**
- Consumes: `kernel_common::fs::SPARE_BLOCKS` (Task 1).
- Produces: the image is `(total_blocks + SPARE_BLOCKS) * BLOCK_SIZE` bytes; the superblock's `total_blocks` still records only the *used* blocks.

- [ ] **Step 1: Write the failing test** (add to `tools/mkfs/src/lib.rs` tests)

```rust
    #[test]
    fn image_carries_spare_capacity_past_used_blocks() {
        use kernel_common::fs::SPARE_BLOCKS;
        let img = build_image(&[("alpha", b"hi")]);
        let sb = Superblock::decode(&img[0..BLOCK_SIZE]).expect("valid superblock");
        // used: superblock + dir + ceil(2/512)=1 = 3 blocks
        assert_eq!(sb.total_blocks, 3);
        // but the file is padded with spare blocks for in-kernel appends
        assert_eq!(img.len(), (sb.total_blocks + SPARE_BLOCKS) as usize * BLOCK_SIZE);
    }
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p mkfs spare_capacity`
Expected: FAIL — image length equals `total_blocks * BLOCK_SIZE` (no slack).

- [ ] **Step 3: Implement** (in `tools/mkfs/src/lib.rs`)

Add `SPARE_BLOCKS` to the import list from `kernel_common::fs`, then change the image-sizing line in `build_image`:

```rust
    let total_blocks = DATA_START_BLOCK + data_blocks;
    // Pad the file with spare capacity past the used blocks so the in-kernel
    // writer has device room to append entries (the organism's write-back).
    let device_blocks = total_blocks + SPARE_BLOCKS;
    let mut img = vec![0u8; device_blocks as usize * BLOCK_SIZE];
```

(The superblock still encodes `total_blocks` = used blocks — unchanged.)

- [ ] **Step 4: Run the test to verify it passes**

Run: `cargo test -p mkfs`
Expected: PASS (new test + the existing `build_then_parse_round_trips_two_files`, which checks `sb.total_blocks`, not `img.len()`).

- [ ] **Step 5: Commit**

```bash
git add tools/mkfs/src/lib.rs
git commit -m "feat(mkfs): pad image with spare capacity for in-kernel appends"
```

---

## Task 6: Kernel write path — blk server direction bit, `fs_write_block`, `fs_append_file`

**Files:**
- Modify: `kernel/src/main.rs` (`blk_component`, plus new `fs_write_block` / `fs_append_file` near `fs_read_block`)
- Test: covered by the boot test (Task 8); no host test (kernel-target code).

**Interfaces:**
- Consumes: `blk_req` (already supports `write: bool`), `sched::call_message`, `BLK_DMA_PA`, `virtio::BLK_DATA_OFF`, `BLK_EP_CAP`, `fs::{Superblock, append_plan, BLOCK_SIZE}`.
- Produces:
  - a `BLK_WRITE_FLAG` badge bit understood by `blk_component`;
  - `fn fs_write_block(n: u32, bytes: &[u8]) -> bool` — write up to one block;
  - `fn fs_append_file(name: &str, contents: &[u8]) -> bool` — append a file, superblock-committed last.

- [ ] **Step 1: Add the write-direction constant and extend the blk server**

Near the blk cap-slot constants (around line 331) add:

```rust
    /// Badge bit that asks the blk server to WRITE block N from the DMA data
    /// page (otherwise it reads into it). Block numbers are small, so the high
    /// bit is free.
    const BLK_WRITE_FLAG: usize = 1 << 31;
```

Change the `blk_component` server loop (around line 1094) from:

```rust
            loop {
                let block = sys_recv(BLK_EP_CAP, BLK_REPLY_SLOT); // badge = block #
                let status = blk_req(mmio, dma, false, avail_idx, block as u64);
                avail_idx = avail_idx.wrapping_add(1);
                sys_reply(BLK_REPLY_SLOT, status as usize);
            }
```

to:

```rust
            loop {
                // badge = block #, with BLK_WRITE_FLAG set for a write.
                let badge = sys_recv(BLK_EP_CAP, BLK_REPLY_SLOT);
                let write = badge & BLK_WRITE_FLAG != 0;
                let block = (badge & !BLK_WRITE_FLAG) as u64;
                let status = blk_req(mmio, dma, write, avail_idx, block);
                avail_idx = avail_idx.wrapping_add(1);
                sys_reply(BLK_REPLY_SLOT, status as usize);
            }
```

- [ ] **Step 2: Add `fs_write_block` and `fs_append_file`** (after `fs_read_block`, around line 1123)

```rust
    /// Write `bytes` (≤ one block; zero-padded) into block `n` via the blk
    /// server. Fills the shared DMA data page, then asks the server to write it.
    /// Returns false on a device error. Invalidates the one-block read cache,
    /// since the DMA page now holds `n`'s outgoing data.
    fn fs_write_block(n: u32, bytes: &[u8]) -> bool {
        // SAFETY: BLK_DMA_PA is the kernel-allocated, identity-mapped DMA frame;
        // single hart owns the page for the duration of this write.
        unsafe {
            let data = (BLK_DMA_PA + virtio::BLK_DATA_OFF) as *mut u8;
            let take = core::cmp::min(bytes.len(), virtio::BLK_SECTOR_SIZE);
            core::ptr::copy_nonoverlapping(bytes.as_ptr(), data, take);
            for i in take..virtio::BLK_SECTOR_SIZE {
                core::ptr::write(data.add(i), 0);
            }
            FS_CACHED_BLOCK = -1; // DMA page no longer holds a cached read
            let status = sched::call_message(BLK_EP_CAP, (n as usize) | BLK_WRITE_FLAG);
            status == 0
        }
    }

    /// Append a file named `name` with `contents` to the on-disk volume. Reads
    /// the superblock + directory, plans the append (`fs::append_plan`), then
    /// writes data block(s) → directory → superblock LAST. The superblock write
    /// is the single commit point: a crash before it leaves the new blocks
    /// unreferenced (invisible), so existing data is never corrupted. Returns
    /// false if the plan is refused or any write errors (aborting before the
    /// commit). `contents` must fit in one block (a KB entry's frontmatter does).
    fn fs_append_file(name: &str, contents: &[u8]) -> bool {
        use kernel_common::fs;
        if contents.len() > fs::BLOCK_SIZE {
            return false;
        }
        // Copy the superblock and directory out of the reused DMA page before
        // any write overwrites it.
        let mut sb_buf = [0u8; fs::BLOCK_SIZE];
        match fs_read_block(0) {
            Some(b) => sb_buf.copy_from_slice(b),
            None => return false,
        }
        let sb = match fs::Superblock::decode(&sb_buf) {
            Some(sb) => sb,
            None => return false,
        };
        let mut dir_buf = [0u8; fs::BLOCK_SIZE];
        match fs_read_block(sb.dir_block) {
            Some(b) => dir_buf.copy_from_slice(b),
            None => return false,
        }
        let plan = match fs::append_plan(&sb, &dir_buf, name, contents.len() as u32) {
            Some(p) => p,
            None => return false,
        };
        // 1. data, 2. directory, 3. superblock (commit) — in that order.
        if !fs_write_block(plan.start_block, contents) {
            return false;
        }
        if !fs_write_block(sb.dir_block, &plan.dir_block) {
            return false;
        }
        let mut new_sb = [0u8; fs::BLOCK_SIZE];
        plan.new_superblock.encode(&mut new_sb);
        fs_write_block(0, &new_sb)
    }
```

- [ ] **Step 3: Build to verify it compiles**

Run: `./tools/build.ps1`
Expected: builds clean (functions are unused until Task 7 wires the writer; add `#[allow(dead_code)]` only if the build treats the warning as an error — otherwise leave it, Task 7 consumes them).

- [ ] **Step 4: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat(fs): kernel write path — blk write badge, fs_write_block, fs_append_file"
```

---

## Task 7: The KB-writer task + the novel patient + boot wiring

**Files:**
- Modify: `kernel/src/main.rs` (new `kb_writer_task`, new `novel_task`, boot spawns, gate-release count)
- Test: boot test (Task 8).

**Interfaces:**
- Consumes: `heal::{take_novel, max_kb_number, install}`, `kb::serialize`, `fs_append_file`, the existing gate (`GATE_*`), patient spawn pattern, `sched::{spawn, spawn_user, grant_cap, call_message, yield_now}`.
- Produces: the runtime behavior the boot test asserts (`heal: recorded KB-0006 ...`).

- [ ] **Step 1: Add the KB-writer task** (near `kb_loader_task`, around line 1220)

```rust
    /// Default playbook the organism records for any newly-seen contained crash
    /// — the same caged, bounded restart KB-0005 prescribes.
    const DEFAULT_PLAYBOOK: &str = "Restart the component, up to a bounded number of retries.";

    /// The KB-writer task (Phase 7): drains the novel-cause mailbox the crash
    /// path fills, mints a KB entry for the unrecognized token, appends it to
    /// disk (`fs_append_file`), and installs it into the runtime table — so a
    /// later boot of the same image diagnoses the formerly-novel crash. Runs
    /// with interrupts on (I/O is forbidden in the crash path). Polls in a
    /// yield loop, like `idle`/the loader.
    extern "C" fn kb_writer_task() -> ! {
        use kernel_common::kb;
        loop {
            if let Some(token) = heal::take_novel() {
                // Mint the next id: "KB-NNNN" after the largest installed.
                let num = heal::max_kb_number() + 1;
                let mut id = [0u8; 8]; // "KB-NNNN"
                id[0] = b'K'; id[1] = b'B'; id[2] = b'-';
                id[3] = b'0' + ((num / 1000) % 10) as u8;
                id[4] = b'0' + ((num / 100) % 10) as u8;
                id[5] = b'0' + ((num / 10) % 10) as u8;
                id[6] = b'0' + (num % 10) as u8;
                let id = core::str::from_utf8(&id[..7]).unwrap_or("KB-0000");

                // Title: "Observed fault: <token> (auto-recorded)".
                let mut title = [0u8; 96];
                let mut tn = 0usize;
                for s in ["Observed fault: ", token, " (auto-recorded)"] {
                    let b = s.as_bytes();
                    let take = core::cmp::min(b.len(), title.len() - tn);
                    title[tn..tn + take].copy_from_slice(&b[..take]);
                    tn += take;
                }
                let title = core::str::from_utf8(&title[..tn]).unwrap_or("Observed fault");

                let mut doc = [0u8; kernel_common::fs::BLOCK_SIZE];
                if let Some(len) = kb::serialize(id, title, DEFAULT_PLAYBOOK, token, &mut doc) {
                    if fs_append_file(id, &doc[..len]) {
                        // Install it now too, so a re-crash this boot matches.
                        heal::install(id, title, DEFAULT_PLAYBOOK, Some(token));
                        println!("heal: recorded {id} ({token}) to disk");
                    } else {
                        println!("heal: could not record {id} ({token}) to disk");
                    }
                }
            }
            sched::yield_now();
            // SAFETY: wait for the next interrupt between polls.
            unsafe { core::arch::asm!("wfi") };
        }
    }

    /// The novel patient (Phase 7): a U-mode component that executes an illegal
    /// instruction (`unimp`) — a contained crash whose token has no KB entry at
    /// first boot. Gated on the KB-ready gate (first run) like the other
    /// patients, so it crashes only after the loader has built the table.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn novel_task() -> ! {
        let generation: usize;
        // SAFETY: read the launch generation the kernel placed in a0.
        unsafe {
            core::arch::asm!("mv {g}, a0", g = out(reg) generation, options(nomem, nostack, preserves_flags));
        }
        if generation == 0 {
            // SAFETY: we hold Endpoint(GATE_EP) at GATE_CAP; recv then reply.
            unsafe {
                let _ = sys_recv(GATE_CAP, GATE_REPLY_SLOT);
                sys_reply(GATE_REPLY_SLOT, 0);
            }
        }
        // SAFETY: `unimp` is the canonical illegal instruction; the U-mode trap
        // (scause 2) is contained by the kernel. Control never returns.
        unsafe {
            core::arch::asm!("unimp", options(nostack, noreturn));
        }
    }
```

- [ ] **Step 2: Spawn the writer and the novel patient in `kmain`**

After the flaky patient block (around line 129), add the novel patient:

```rust
        // Phase 7 — the novel patient: an illegal-instruction crash with no KB
        // entry at first boot. Gated on the KB-ready gate like the others. No
        // Restart cap: it just needs to be contained and diagnosed (None on
        // boot 1 -> recorded; matched on boot 2).
        let xu = ustack(core::ptr::addr_of!(US_NOVEL) as usize);
        let novel = sched::spawn_user("novel", novel_task, xu.1,
            core::ptr::addr_of!(KS_NOVEL) as usize + TASK_STACK,
            mem::build_user_space(xu, NO_DEVICE));
        sched::grant_cap(novel, GATE_CAP, Capability::Endpoint(GATE_EP));
```

After the blk block where the loader is granted `BLK_EP_CAP` (around line 235), spawn the writer (it needs blk access; only meaningful when a disk backs the FS):

```rust
            // Phase 7 — the KB-writer: records a novel contained crash to disk.
            let kb_writer = sched::spawn("kbw", kb_writer_task,
                core::ptr::addr_of!(KS_KBW) as usize + TASK_STACK);
            sched::grant_cap(kb_writer, BLK_EP_CAP, Capability::Endpoint(BLK_EP));
```

- [ ] **Step 3: Add the three gate releases** (the loader releases each gated patient once)

In `kb_loader_task`, change the two releases (around line 1213) to three (transient, flaky, novel):

```rust
        // Release the gated patients now that the table is built.
        sched::call_message(GATE_LOADER_CAP, 0);
        sched::call_message(GATE_LOADER_CAP, 0);
        sched::call_message(GATE_LOADER_CAP, 0);
```

- [ ] **Step 4: Declare the new per-task stacks `US_NOVEL`, `KS_NOVEL`, `KS_KBW`**

Find the block declaring the other task stacks (search for `US_FLAKY` / `KS_FLAKY`). Add, following the exact existing pattern and type (match the surrounding `static mut ... : [u8; TASK_STACK]` declarations; `US_*` are user stacks, `KS_*` kernel/trap stacks):

```rust
    static mut US_NOVEL: [u8; TASK_STACK] = [0; TASK_STACK];
    static mut KS_NOVEL: [u8; TASK_STACK] = [0; TASK_STACK];
    static mut KS_KBW: [u8; TASK_STACK] = [0; TASK_STACK];
```

(Match whatever exact size/type the neighboring `US_FLAKY`/`KS_FLAKY` use — copy their declaration and rename. `kbw` is a kernel task, so it needs only a `KS_` stack, like `kb`/`idle`.)

- [ ] **Step 5: Build and smoke-run**

Run: `./tools/build.ps1`
Expected: clean build (the dead-code warnings from Task 6 are now resolved — the functions are used).

- [ ] **Step 6: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat(heal): KB-writer task + novel illegal-instruction patient + wiring"
```

---

## Task 8: Two-boot cross-boot test in `test-qemu.ps1`

**Files:**
- Modify: `tools/test-qemu.ps1`
- Test: this IS the integration test.

**Interfaces:**
- Consumes: the markers `heal: recorded KB-0006 (illegal-instruction) to disk` (boot 1), `heal: loaded 2 KB entries from disk` and `heal: diagnosed KB-0006 ...` (boot 2).

- [ ] **Step 1: Refactor the single QEMU run into a reusable helper**

Wrap the existing `Start-Process qemu-system-riscv64 ... ; wait-loop ; Stop-Process` block into a function that takes the must-match list and a serial-log path, runs one boot, and returns the list of still-missing patterns. Keep the existing QEMU args verbatim (same `-drive file=$diskImg,...`, `-device virtio-rng-device`, etc.) so both boots share `$diskImg`.

```powershell
function Invoke-Boot([string[]]$mustMatch, [string]$serialLog) {
    if (Test-Path $serialLog) { Remove-Item $serialLog -Force }
    $qemu = Start-Process qemu-system-riscv64 -PassThru -NoNewWindow -ArgumentList @(
        "-machine", "virt",
        "-m", "192M",
        "-global", "virtio-mmio.force-legacy=false",
        "-device", "virtio-rng-device",
        "-drive", "file=$diskImg,if=none,format=raw,id=blk0",
        "-device", "virtio-blk-device,drive=blk0",
        "-display", "none",
        "-serial", "file:$serialLog",
        "-bios", "default",
        "-kernel", $kernelElf
    )
    $missing = $mustMatch
    try {
        $deadline = (Get-Date).AddSeconds(30)
        while ((Get-Date) -lt $deadline) {
            Start-Sleep -Milliseconds 500
            $text = Read-LogText $serialLog
            $missing = @($mustMatch | Where-Object { $text -notmatch $_ })
            if ($missing.Count -eq 0) { break }
        }
    }
    finally {
        if (-not $qemu.HasExited) { Stop-Process -Id $qemu.Id -Force }
    }
    return $missing
}
```

- [ ] **Step 2: Build the fresh image once, then run boot 1**

After the existing `& $mkfs $diskImg` (which builds the fresh boot-1 image — KB-0005 only + spare), keep the existing `$mustMatch` list as the **boot-1** list and ADD this line to it:

```powershell
    "heal: recorded KB-0006 \(illegal-instruction\) to disk",
```

Then call the helper (boot 1 uses the existing `$serialLog`):

```powershell
$missing1 = Invoke-Boot $mustMatch $serialLog
```

- [ ] **Step 3: Run boot 2 against the SAME image (no rebuild)**

After boot 1, do NOT rebuild the image. Define the boot-2 expectations and run a second boot into a separate log:

```powershell
$serialLog2 = Join-Path ([System.IO.Path]::GetTempPath()) "kernel-qemu-serial-2.log"
$mustMatch2 = @(
    "heal: loaded 2 KB entries from disk",
    "heal: diagnosed KB-0006 \(Observed fault: illegal-instruction \(auto-recorded\)\) -> playbook: Restart the component, up to a bounded number of retries"
)
$missing2 = Invoke-Boot $mustMatch2 $serialLog2
```

- [ ] **Step 4: Combine the pass/fail reporting**

Replace the single `if ($missing.Count -eq 0)` block so the test passes only if BOTH boots matched. On failure, print which boot's patterns are missing. Update the PASS banner to append the Phase 7 clause, e.g.:

```powershell
if ($missing1.Count -eq 0 -and $missing2.Count -eq 0) {
    Write-Host "BOOT TEST PASS: ... (existing banner) ...; and Phase 7 write-back: on first boot the self-healer meets a novel illegal-instruction crash with no KB entry and RECORDS a new entry (KB-0006) to the writable disk; on a second boot of the same image it loads 2 entries and DIAGNOSES KB-0006 - the organism learned across reboots." -ForegroundColor Green
    exit 0
} else {
    if ($missing1.Count -ne 0) { Write-Host "BOOT 1 missing:" -ForegroundColor Red; $missing1 | ForEach-Object { Write-Host "  $_" } }
    if ($missing2.Count -ne 0) { Write-Host "BOOT 2 missing:" -ForegroundColor Red; $missing2 | ForEach-Object { Write-Host "  $_" } }
    exit 1
}
```

(Preserve the existing failure-path details already in the file; adapt them to the two-list form. Keep the existing boot-1 banner text and append the Phase 7 clause rather than rewriting it.)

- [ ] **Step 5: Run the full boot test**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS: ...; and Phase 7 write-back: ...`. If boot 2 reports KB-0006 missing, check that the `-drive` is not in snapshot mode and that the image was NOT rebuilt between boots.

- [ ] **Step 6: Commit**

```bash
git add tools/test-qemu.ps1
git commit -m "test: two-boot cross-boot proof of KB write-back (Phase 7)"
```

---

## Task 9: Documentation — learning note, roadmap, glossary

**Files:**
- Create: `docs/learning/0025-write-back-learning-organism.md`
- Modify: `docs/roadmap/roadmap.md` (mark Phase 7 done; reshape the old "Phase 7+ — Breadth" into "Phase 8+")
- Modify: `docs/glossary.md` (add: write-back, append-only FS, novel cause / auto-recorded entry, commit point)
- Modify: `docs/learning/README.md` (index the new note, if it maintains an index)

- [ ] **Step 1: Write the learning note** `docs/learning/0025-write-back-learning-organism.md`

Cover (short, per memory: learning-notes-minimal — summary not tutorial): what changed (writable append-only FS; the organism records a novel crash and reads it back next boot); the idea worth keeping (the kernel can *name* a symptom it hasn't *catalogued* — recording = writing new knowledge keyed by that token; diagnosis is unchanged); the constraint (write ordering data→dir→superblock-last is the commit point; capacity overflow fails safe at the device); the proof (boot 1 `heal: recorded KB-0006`, boot 2 `heal: diagnosed KB-0006`); what's next (in-place updates / counters, deletion/compaction, free-block allocator). Follow the structure of `0024-living-knowledge-base.md`.

- [ ] **Step 2: Update the roadmap**

Replace the placeholder `## Phase 7+ — Breadth` section: add a completed `## Phase 7 — Write-back & the learning organism (done — 2026-06-26)` section (goal / you-learn / done-when, matching the style of the Phase 6 sections and citing learning note 0025), and renumber the breadth placeholder to `## Phase 8+ — Breadth`.

- [ ] **Step 3: Update the glossary** with the new terms (write-back, append-only filesystem, commit point / superblock-last ordering, novel cause / auto-recorded KB entry).

- [ ] **Step 4: Run the cross-reference check**

Run: `./tools/check-references.ps1`
Expected: passes (every cited path/KB id exists). Fix any dangling reference.

- [ ] **Step 5: Commit**

```bash
git add docs/learning/0025-write-back-learning-organism.md docs/roadmap/roadmap.md docs/glossary.md docs/learning/README.md
git commit -m "docs: Phase 7 write-back — learning note 0025, roadmap, glossary"
```

---

## Self-Review (completed during planning)

- **Spec coverage:** append-only FS → Task 1; `kb::serialize` round-trip → Task 2; `IllegalInstruction`/token/containment → Task 3; novel mailbox + deferred writer + idempotency + next-id → Tasks 4 & 7; mkfs spare capacity → Task 5; blk write path + write ordering/commit point + error abort → Task 6; novel patient + gate → Task 7; two-boot cross-boot proof → Task 8; host tests for the pure units → Tasks 1–5; docs → Task 9. All spec sections map to a task.
- **Type consistency:** `append_plan(&Superblock, &[u8], &str, u32) -> Option<AppendPlan>` produced in Task 1 and consumed identically in Task 6; `AppendPlan { start_block, new_superblock, dir_block }` field names match across Tasks 1 and 6; `serialize(id,title,playbook,match_cause,out) -> Option<usize>` consistent across Tasks 2 and 7; `heal::{note_unmatched, take_novel, max_kb_number, install}` signatures consistent across Tasks 4 and 7; `BLK_WRITE_FLAG` defined and used in Task 6 and reused by `fs_write_block`.
- **Open verification during execution:** confirm the arch crate's package name for `cargo test -p <name>` (Task 3 assumes `riscv64`); confirm the exact `static mut US_*/KS_*` stack declaration form when adding `US_NOVEL/KS_NOVEL/KS_KBW` (Task 7 Step 4) by reading the neighbors first.
