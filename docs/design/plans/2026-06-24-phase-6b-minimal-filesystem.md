# Phase 6b — Minimal Filesystem Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Give the kernel a minimal, read-only filesystem over the Phase 6a virtio-blk driver so it can locate a file **by name** in a boot-time disk image and read its contents off the disk.

**Architecture:** A shared `no_std` on-disk format (`kernel-common::fs`: superblock + a flat directory of fixed-size entries + contiguous file extents) is consumed by two sides: a host `mkfs` tool that builds the disk image, and an in-kernel reader. The Phase 6a `blk` component stops self-testing and becomes a `call`-driven server ("read block N" → virtio read into its identity-mapped DMA page → reply). The kernel filesystem is the **client** (new kernel-side `call_message`), reads sector bytes straight from the identity-mapped DMA page through a single `read_block(n)` boundary (one-block cache), walks the layout, and prints the file.

**Tech Stack:** Rust (`no_std` kernel + arch crate; `std` host tool), QEMU riscv64 `virt`, virtio-mmio + PLIC, PowerShell smoke test.

**Grounding facts (verified against the current tree):**
- `kernel-common` is the package name (crate path `libs/common`, imported as `kernel_common`). It is `#![cfg_attr(not(test), no_std)]`. Add `pub mod fs;` to `libs/common/src/lib.rs`.
- Workspace `members` are listed in the root `Cargo.toml`; there is **no** `[build] target`, so `cargo build -p mkfs` builds for the host (output `target/debug/mkfs.exe` on Windows).
- `MAX_TASKS = 16` (`arch/riscv64/src/sched.rs:28`); the smoke cast is 15 tasks. Phase 6b **replaces** the kernel `blk` consumer with the kernel `fs` task — count unchanged, **no `MAX_TASKS` bump**.
- IPC the plan reuses (all in `arch/riscv64/src/sched.rs`): `Message { badge, data: [usize;3] }`, `EndpointId`, `IpcRole`, `TaskState::{AwaitingReply, Ready, ...}`, `cap_lookup`, private `find_blocked`, `park_current`, `block_current`, public `recv_message`. The U-mode `call`/`recv`/`reply` syscalls (`ipc_call`/`ipc_recv`/`ipc_reply`) already implement the one-shot reply-cap rendezvous; a recv minting a reply cap and a queued `Call` sender are both handled (proven by the `deferrer` demo).
- Kernel-side `blk` helpers in `kernel/src/main.rs` `bare` module: `blk_req(mmio, dma, write, avail_idx) -> u8` (`main.rs:640`), `virtio_queue_init`, `sys_recv(cap, reply_slot)`, `sys_reply(reply_slot, badge)`, `dma_*`/`mmio_*`, `sys_wait_irq`. virtio/blk constants live in `arch/riscv64/src/virtio.rs` (`BLK_DATA_OFF=512`, `BLK_SECTOR_SIZE=512`, etc.).
- The `blk` component currently holds: cap slot 0 = `Endpoint(BLK_EP)` (was `BLK_REPORT_CAP`), cap slot 1 = `Interrupt(blk_irq)` (`BLK_IRQ_CAP`). `CAP_SLOTS ≥ 3` (the `deferrer` recvs into slots 1 and 2), so a reply cap at slot 2 is valid.
- The smoke test (`tools/test-qemu.ps1`) already passes `-drive file=$diskImg,...` + `-device virtio-blk-device`; only the image-build and the assertions change.

---

## File Structure

| File | Create/Modify | Responsibility |
|------|---------------|----------------|
| `libs/common/src/fs.rs` | Create | On-disk format: constants, `Superblock`, `DirEntry`, encode/decode, `lookup`, `block_count`. Pure, `no_std`, host-tested. |
| `libs/common/src/lib.rs` | Modify | `pub mod fs;` |
| `tools/mkfs/Cargo.toml` | Create | Host bin+lib crate depending on `kernel-common`. |
| `tools/mkfs/src/lib.rs` | Create | `build_image(files) -> Vec<u8>` using `kernel_common::fs`; host-tested. |
| `tools/mkfs/src/main.rs` | Create | CLI: build the Phase 6b demo image and write it to the path in `argv[1]`. |
| `Cargo.toml` (root) | Modify | Add `"tools/mkfs"` to `members`. |
| `tools/test-qemu.ps1` | Modify | Build the image with `mkfs`; assert the `fs:` read line + tail marker; drop the retired `blk: sector round-trip ok`. |
| `arch/riscv64/src/sched.rs` | Modify | Add `call_message(cap_idx, badge) -> reply_badge` (kernel-side IPC client). |
| `kernel/src/main.rs` | Modify | `blk_req` gains a `sector` param; `blk_component` becomes a serve loop; add `fs_task` + `read_block`/`read_file` + statics; rewire `kmain` (remove `blk_consumer`). |
| `docs/learning/0023-minimal-filesystem.md` | Create | Short learning note. |
| `docs/roadmap/roadmap.md` | Modify | Mark Phase 6b done. |
| `docs/glossary.md` | Modify | Add filesystem / superblock / directory entry / extent / block cache. |

---

## Task 1: The on-disk format module (`kernel-common::fs`)

**Files:**
- Create: `libs/common/src/fs.rs`
- Modify: `libs/common/src/lib.rs`
- Test: `libs/common/src/fs.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Write the failing tests**

Create `libs/common/src/fs.rs` with ONLY the test module first (the items it references come in Step 3):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn superblock_round_trips() {
        let sb = Superblock {
            magic: FS_MAGIC, version: FS_VERSION, block_size: BLOCK_SIZE as u32,
            dir_block: DIR_BLOCK, dir_entries: 2, total_blocks: 5,
        };
        let mut block = [0u8; BLOCK_SIZE];
        sb.encode(&mut block);
        assert_eq!(Superblock::decode(&block), Some(sb));
    }

    #[test]
    fn superblock_decode_rejects_bad_magic_or_version() {
        let mut block = [0u8; BLOCK_SIZE];
        Superblock { magic: 0xdead, version: FS_VERSION, block_size: 512,
            dir_block: 1, dir_entries: 0, total_blocks: 2 }.encode(&mut block);
        assert_eq!(Superblock::decode(&block), None);
        let mut block2 = [0u8; BLOCK_SIZE];
        Superblock { magic: FS_MAGIC, version: 99, block_size: 512,
            dir_block: 1, dir_entries: 0, total_blocks: 2 }.encode(&mut block2);
        assert_eq!(Superblock::decode(&block2), None);
    }

    #[test]
    fn dirent_round_trips_and_name_matches() {
        let e = DirEntry::new("KB-0005", 2, 574);
        let mut buf = [0u8; DIRENT_SIZE];
        e.encode(&mut buf);
        let d = DirEntry::decode(&buf);
        assert_eq!(d.start_block, 2);
        assert_eq!(d.byte_len, 574);
        assert!(d.name_is("KB-0005"));
        assert!(!d.name_is("KB-0004"));
        assert!(!d.name_is("KB-0005x"));
    }

    #[test]
    fn block_count_rounds_up() {
        assert_eq!(block_count(0), 0);
        assert_eq!(block_count(1), 1);
        assert_eq!(block_count(512), 1);
        assert_eq!(block_count(513), 2);
        assert_eq!(block_count(1024), 2);
    }

    #[test]
    fn lookup_finds_present_and_rejects_absent() {
        let mut dir = [0u8; BLOCK_SIZE];
        DirEntry::new("alpha", 2, 10).encode(&mut dir[0..DIRENT_SIZE]);
        DirEntry::new("beta", 3, 20).encode(&mut dir[DIRENT_SIZE..2 * DIRENT_SIZE]);
        assert_eq!(lookup(&dir, 2, "alpha").unwrap().start_block, 2);
        assert_eq!(lookup(&dir, 2, "beta").unwrap().byte_len, 20);
        assert!(lookup(&dir, 2, "gamma").is_none());
        assert!(lookup(&dir, 0, "alpha").is_none()); // count gates the scan
    }
}
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p kernel-common`
Expected: FAIL — `cannot find type Superblock` / `cannot find function lookup` (items not defined yet).

- [ ] **Step 3: Write the module (above the test module)**

Prepend this to `libs/common/src/fs.rs`:

```rust
//! A minimal, read-only on-disk filesystem format shared by the in-kernel
//! reader (Phase 6b) and the host `mkfs` tool: a superblock, a flat directory
//! of fixed-size entries, and contiguous file extents. 512-byte blocks.
//!
//! Pure layout logic — no I/O, no arch code — so it is host-tested and used
//! identically on both sides (define the format once).

/// Bytes per block; equals the virtio-blk sector size, so one FS block is one
/// device request (no block/sector translation).
pub const BLOCK_SIZE: usize = 512;
/// Superblock magic — the ASCII bytes "6BFS" little-endian.
pub const FS_MAGIC: u32 = 0x5346_4236;
/// On-disk format version.
pub const FS_VERSION: u32 = 1;
/// The directory occupies block 1 (block 0 is the superblock).
pub const DIR_BLOCK: u32 = 1;
/// First block available for file data.
pub const DATA_START_BLOCK: u32 = 2;
/// Bytes reserved for a NUL-padded name in a directory entry.
pub const NAME_LEN: usize = 48;
/// On-disk size of one directory entry.
pub const DIRENT_SIZE: usize = 64;
/// Directory entries that fit in one block.
pub const DIRENTS_PER_BLOCK: usize = BLOCK_SIZE / DIRENT_SIZE;

/// The first block of the image: locates the directory and bounds the volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Superblock {
    pub magic: u32,
    pub version: u32,
    pub block_size: u32,
    pub dir_block: u32,
    pub dir_entries: u32,
    pub total_blocks: u32,
}

impl Superblock {
    /// Encode into the start of a block buffer (first 24 bytes; rest untouched).
    pub fn encode(&self, block: &mut [u8]) {
        block[0..4].copy_from_slice(&self.magic.to_le_bytes());
        block[4..8].copy_from_slice(&self.version.to_le_bytes());
        block[8..12].copy_from_slice(&self.block_size.to_le_bytes());
        block[12..16].copy_from_slice(&self.dir_block.to_le_bytes());
        block[16..20].copy_from_slice(&self.dir_entries.to_le_bytes());
        block[20..24].copy_from_slice(&self.total_blocks.to_le_bytes());
    }

    /// Decode and validate. Returns `None` if magic or version mismatch.
    pub fn decode(block: &[u8]) -> Option<Superblock> {
        if block.len() < 24 {
            return None;
        }
        let rd = |o: usize| u32::from_le_bytes([block[o], block[o + 1], block[o + 2], block[o + 3]]);
        let sb = Superblock {
            magic: rd(0),
            version: rd(4),
            block_size: rd(8),
            dir_block: rd(12),
            dir_entries: rd(16),
            total_blocks: rd(20),
        };
        if sb.magic != FS_MAGIC || sb.version != FS_VERSION {
            return None;
        }
        Some(sb)
    }
}

/// One directory entry: maps a name to a contiguous file extent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DirEntry {
    pub name: [u8; NAME_LEN],
    pub start_block: u32,
    pub byte_len: u32,
}

impl DirEntry {
    /// Build from a name (truncated to `NAME_LEN`, NUL-padded).
    pub fn new(name: &str, start_block: u32, byte_len: u32) -> DirEntry {
        let mut n = [0u8; NAME_LEN];
        let b = name.as_bytes();
        let k = core::cmp::min(b.len(), NAME_LEN);
        n[..k].copy_from_slice(&b[..k]);
        DirEntry { name: n, start_block, byte_len }
    }

    /// Encode into a `DIRENT_SIZE`-byte slice (trailing 8 bytes reserved/zero).
    pub fn encode(&self, e: &mut [u8]) {
        e[..NAME_LEN].copy_from_slice(&self.name);
        e[NAME_LEN..NAME_LEN + 4].copy_from_slice(&self.start_block.to_le_bytes());
        e[NAME_LEN + 4..NAME_LEN + 8].copy_from_slice(&self.byte_len.to_le_bytes());
    }

    /// Decode from a `DIRENT_SIZE`-byte slice.
    pub fn decode(e: &[u8]) -> DirEntry {
        let mut name = [0u8; NAME_LEN];
        name.copy_from_slice(&e[..NAME_LEN]);
        let start_block = u32::from_le_bytes([e[NAME_LEN], e[NAME_LEN + 1], e[NAME_LEN + 2], e[NAME_LEN + 3]]);
        let byte_len = u32::from_le_bytes([e[NAME_LEN + 4], e[NAME_LEN + 5], e[NAME_LEN + 6], e[NAME_LEN + 7]]);
        DirEntry { name, start_block, byte_len }
    }

    /// Does this entry's (NUL-padded) name equal `want`?
    pub fn name_is(&self, want: &str) -> bool {
        let w = want.as_bytes();
        if w.len() > NAME_LEN {
            return false;
        }
        let end = self.name.iter().position(|&c| c == 0).unwrap_or(NAME_LEN);
        &self.name[..end] == w
    }
}

/// Number of blocks a file of `byte_len` bytes occupies (ceil division).
pub fn block_count(byte_len: u32) -> u32 {
    (byte_len + BLOCK_SIZE as u32 - 1) / BLOCK_SIZE as u32
}

/// Find the entry named `name` among the first `count` entries packed in one
/// directory block `dir_bytes`. `None` if absent.
pub fn lookup(dir_bytes: &[u8], count: u32, name: &str) -> Option<DirEntry> {
    let n = core::cmp::min(count as usize, DIRENTS_PER_BLOCK);
    for i in 0..n {
        let off = i * DIRENT_SIZE;
        if off + DIRENT_SIZE > dir_bytes.len() {
            break;
        }
        let e = DirEntry::decode(&dir_bytes[off..off + DIRENT_SIZE]);
        if e.name_is(name) {
            return Some(e);
        }
    }
    None
}
```

- [ ] **Step 4: Register the module**

In `libs/common/src/lib.rs`, add after the doc comment / `PROJECT_NAME` block (top-level, before the `#[cfg(test)]` tests module):

```rust
pub mod fs;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p kernel-common`
Expected: PASS (all 5 `fs` tests plus the existing `project_name_is_set`).

- [ ] **Step 6: Commit**

```bash
git add libs/common/src/fs.rs libs/common/src/lib.rs
git commit -m "feat(fs): on-disk format (superblock + directory + extents), host-tested"
```

---

## Task 2: The `mkfs` host tool

**Files:**
- Create: `tools/mkfs/Cargo.toml`
- Create: `tools/mkfs/src/lib.rs`
- Create: `tools/mkfs/src/main.rs`
- Modify: `Cargo.toml` (root — add member)
- Test: `tools/mkfs/src/lib.rs` (`#[cfg(test)] mod tests`)

- [ ] **Step 1: Create the crate manifest**

Create `tools/mkfs/Cargo.toml`:

```toml
[package]
name = "mkfs"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
kernel-common = { path = "../../libs/common" }
```

- [ ] **Step 2: Add the crate to the workspace**

In the root `Cargo.toml`, add `"tools/mkfs"` to `members`:

```toml
members = [
    "kernel",
    "hal",
    "arch/riscv64",
    "libs/common",
    "libs/crypto",
    "tools/mkfs",
]
```

- [ ] **Step 3: Write the failing test**

Create `tools/mkfs/src/lib.rs`:

```rust
//! Build a Phase 6b filesystem image (see `kernel_common::fs`) from a set of
//! named files. Pure host code; the in-kernel reader parses what this writes.

use kernel_common::fs::{
    block_count, DirEntry, Superblock, BLOCK_SIZE, DATA_START_BLOCK, DIRENTS_PER_BLOCK,
    DIR_BLOCK, FS_MAGIC, FS_VERSION,
};

/// Build a raw disk image: block 0 superblock, block 1 directory, block 2+
/// contiguous file data. Panics if more than one directory block is needed
/// (6b keeps a single directory block).
pub fn build_image(files: &[(&str, &[u8])]) -> Vec<u8> {
    assert!(files.len() <= DIRENTS_PER_BLOCK, "too many files for one directory block");
    let mut entries: Vec<(DirEntry, &[u8])> = Vec::new();
    let mut next = DATA_START_BLOCK;
    let mut data_blocks = 0u32;
    for (name, bytes) in files {
        let nb = block_count(bytes.len() as u32);
        entries.push((DirEntry::new(name, next, bytes.len() as u32), *bytes));
        next += nb;
        data_blocks += nb;
    }
    let total_blocks = DATA_START_BLOCK + data_blocks;
    let mut img = vec![0u8; total_blocks as usize * BLOCK_SIZE];

    let sb = Superblock {
        magic: FS_MAGIC,
        version: FS_VERSION,
        block_size: BLOCK_SIZE as u32,
        dir_block: DIR_BLOCK,
        dir_entries: files.len() as u32,
        total_blocks,
    };
    sb.encode(&mut img[0..BLOCK_SIZE]);

    let dir_off = DIR_BLOCK as usize * BLOCK_SIZE;
    for (i, (ent, _)) in entries.iter().enumerate() {
        let off = dir_off + i * kernel_common::fs::DIRENT_SIZE;
        ent.encode(&mut img[off..off + kernel_common::fs::DIRENT_SIZE]);
    }
    for (ent, bytes) in &entries {
        let off = ent.start_block as usize * BLOCK_SIZE;
        img[off..off + bytes.len()].copy_from_slice(bytes);
    }
    img
}

#[cfg(test)]
mod tests {
    use super::*;
    use kernel_common::fs::{lookup, Superblock};

    #[test]
    fn build_then_parse_round_trips_two_files() {
        let small = b"hi".to_vec();
        let big = vec![b'Z'; 600]; // spans two blocks
        let img = build_image(&[("alpha", &small), ("big", &big)]);

        let sb = Superblock::decode(&img[0..BLOCK_SIZE]).expect("valid superblock");
        assert_eq!(sb.dir_entries, 2);
        // total: superblock + dir + ceil(2/512)=1 + ceil(600/512)=2 = 5 blocks
        assert_eq!(sb.total_blocks, 5);

        let dir = &img[BLOCK_SIZE..2 * BLOCK_SIZE];
        let a = lookup(dir, sb.dir_entries, "alpha").unwrap();
        let b = lookup(dir, sb.dir_entries, "big").unwrap();
        assert_eq!(a.byte_len, 2);
        assert_eq!(b.byte_len, 600);
        // file bytes land at their start_block and read back intact
        let a_off = a.start_block as usize * BLOCK_SIZE;
        assert_eq!(&img[a_off..a_off + 2], b"hi");
        let b_off = b.start_block as usize * BLOCK_SIZE;
        assert_eq!(&img[b_off..b_off + 600], &big[..]);
    }
}
```

- [ ] **Step 4: Run the test to verify it fails**

Run: `cargo test -p mkfs`
Expected: FAIL — `tools/mkfs/src/main.rs` does not exist yet, so the bin target fails to build (or, if Cargo only builds the lib for the test, the test should already compile — in that case it PASSES and you proceed; the missing `main.rs` is added next regardless).

- [ ] **Step 5: Write the CLI entry point**

Create `tools/mkfs/src/main.rs`:

```rust
//! Build the Phase 6b demo filesystem image and write it to the path given as
//! the first argument. The demo file is named "KB-0005" and spans two blocks,
//! with a marker line near the end (in block 2) so the smoke test can prove a
//! multi-block extent read. (6c will generalize this to pack real KB files.)

use std::fs;

const DEMO_NAME: &str = "KB-0005";

fn demo_content() -> Vec<u8> {
    let mut v = Vec::new();
    v.extend_from_slice(b"PHASE 6B FILESYSTEM TEST FILE\n");
    while v.len() < 560 {
        v.extend_from_slice(b"....filler line....\n");
    }
    v.extend_from_slice(b"FS-6B-TAIL-OK\n");
    v
}

fn main() {
    let out = std::env::args().nth(1).expect("usage: mkfs <image-path>");
    let content = demo_content();
    let img = mkfs::build_image(&[(DEMO_NAME, &content)]);
    fs::write(&out, &img).expect("write image");
    eprintln!(
        "mkfs: wrote {} ({} bytes); file '{}' = {} bytes",
        out,
        img.len(),
        DEMO_NAME,
        content.len()
    );
}
```

- [ ] **Step 6: Run the test + build the bin to verify both pass**

Run: `cargo test -p mkfs && cargo build -p mkfs`
Expected: test PASS; build produces `target/debug/mkfs.exe`.

- [ ] **Step 7: Commit**

```bash
git add Cargo.toml tools/mkfs
git commit -m "feat(mkfs): host tool that builds a Phase 6b filesystem image"
```

---

## Task 3: Update the smoke test (the red integration test)

**Files:**
- Modify: `tools/test-qemu.ps1`

This makes the smoke assert the new behavior **before** the kernel implements it — a deliberately failing (red) integration test, committed as such (matching the Phase 6a workflow).

- [ ] **Step 1: Replace the scratch-image creation with an `mkfs`-built image**

In `tools/test-qemu.ps1`, replace these lines (currently ~59–62):

```powershell
# A scratch raw disk image (64 KiB of zeros) for the virtio-blk driver to
# write a sector to and read it back.
$diskImg = Join-Path ([System.IO.Path]::GetTempPath()) "kernel-qemu-disk.img"
[System.IO.File]::WriteAllBytes($diskImg, (New-Object byte[] 65536))
```

with:

```powershell
# A filesystem disk image (Phase 6b) for the kernel to read a named file from,
# built by the host `mkfs` tool from the shared on-disk format.
cargo build --manifest-path "$repo/Cargo.toml" -p mkfs
$mkfs = Join-Path $repo "target/debug/mkfs.exe"
if (-not (Test-Path $mkfs)) {
    Write-Host "BOOT TEST FAIL: mkfs not produced at $mkfs" -ForegroundColor Red
    exit 1
}
$diskImg = Join-Path ([System.IO.Path]::GetTempPath()) "kernel-qemu-disk.img"
& $mkfs $diskImg
if ($LASTEXITCODE -ne 0) {
    Write-Host "BOOT TEST FAIL: mkfs failed to build the image" -ForegroundColor Red
    exit 1
}
```

- [ ] **Step 2: Swap the blk assertion for the filesystem assertions**

In the `$mustMatch` array, remove this line:

```powershell
    "blk: sector round-trip ok",
```

and add, in its place:

```powershell
    "fs: read 'KB-0005'",
    "FS-6B-TAIL-OK",
```

- [ ] **Step 3: Run the smoke test to verify it fails (red)**

Run: `./tools/test-qemu.ps1`
Expected: FAIL — `missing within 30s: fs: read 'KB-0005', FS-6B-TAIL-OK` (the kernel does not yet read the filesystem). The image build itself should succeed.

- [ ] **Step 4: Commit the red test**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke asserts the kernel reads a named file off disk (red)"
```

---

## Task 4: Kernel-side `call_message` (the IPC client half)

**Files:**
- Modify: `arch/riscv64/src/sched.rs`

A kernel (S-mode) task cannot use the U-mode `ecall` `call` path, so add the S-mode counterpart to `recv_message`. Verified by the smoke test (the project's kernel IPC is integration-tested, like `recv_message`); there is no host unit test for this riscv-gated path.

- [ ] **Step 1: Add `call_message` next to `recv_message`**

In `arch/riscv64/src/sched.rs`, immediately after the `recv_message` function (which ends near line 640), add:

```rust
/// Make a synchronous `call` (send a request, then block for the reply) from a
/// **kernel** (S-mode) task — the client counterpart to [`recv_message`]. The
/// `badge` crosses to the server; the return value is the server's reply badge.
/// Reuses the same call/reply rendezvous and one-shot reply-cap machinery as
/// the U-mode `call` syscall ([`ipc_call`]). Panics if the caller lacks the
/// capability (a kernel-config bug).
#[cfg(target_arch = "riscv64")]
pub fn call_message(cap_idx: usize, badge: usize) -> usize {
    enum K {
        AwaitReply,
        Queue(EndpointId),
    }
    let msg = Message { badge, data: [0, 0, 0] };
    let step = SCHED.with(|s| {
        let cur = s.current;
        let ep = cap_lookup(&s.tasks[cur].as_ref().unwrap().caps, cap_idx)
            .expect("call_message: caller lacks the endpoint capability");
        match find_blocked(s, ep, IpcRole::Recv) {
            Some(ri) => {
                // A server is recv-blocked: hand it the request, bind us as its
                // caller (it mints the reply cap on wake), and ready it.
                s.tasks[ri].as_mut().unwrap().message = msg;
                s.tasks[ri].as_mut().unwrap().caller = Some(cur);
                s.tasks[ri].as_mut().unwrap().state = TaskState::Ready;
                K::AwaitReply
            }
            None => {
                // No server yet: queue our request; a server's recv binds us
                // and moves us to AwaitingReply.
                s.tasks[cur].as_mut().unwrap().message = msg;
                K::Queue(ep)
            }
        }
    });
    match step {
        K::AwaitReply => park_current(TaskState::AwaitingReply),
        K::Queue(ep) => block_current(ep, IpcRole::Call),
    }
    SCHED.with(|s| s.tasks[s.current].as_ref().unwrap().message.badge)
}
```

- [ ] **Step 2: Verify it compiles for the bare target**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: builds (it will warn `call_message` is unused until Task 6 wires it; that is fine).

- [ ] **Step 3: Commit**

```bash
git add arch/riscv64/src/sched.rs
git commit -m "feat(sched): call_message - kernel-side synchronous IPC client"
```

---

## Task 5: The `blk` server (parameterize `blk_req`, replace the self-test)

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Give `blk_req` a `sector` parameter**

In `kernel/src/main.rs`, change the `blk_req` signature (currently line ~640) and the header sector write. Replace:

```rust
    unsafe fn blk_req(mmio: usize, dma: usize, write: bool, avail_idx: u16) -> u8 {
```

with:

```rust
    unsafe fn blk_req(mmio: usize, dma: usize, write: bool, avail_idx: u16, sector: u64) -> u8 {
```

and replace the header sector line:

```rust
        dma_w64(hdr + 8, 0); // sector 0
```

with:

```rust
        dma_w64(hdr + 8, sector);
```

Also update the doc comment's first line above `blk_req` from "Issue one virtio-blk request for sector 0" to "Issue one virtio-blk request for `sector`".

- [ ] **Step 2: Add the cap-slot / reply-slot constants and rename the endpoint cap**

In `kernel/src/main.rs`, replace the block of blk constants (currently ~308–312):

```rust
    /// The endpoint the blk component reports its self-test result on; the cap
    /// slot it (and the consumer) hold it in; and the slot of its Interrupt cap.
    const BLK_EP: usize = 4;
    const BLK_REPORT_CAP: usize = 0;
    const BLK_IRQ_CAP: usize = 1;
```

with:

```rust
    /// The blk service endpoint (the kernel FS calls it; the blk server recvs).
    const BLK_EP: usize = 4;
    /// blk cap slots: 0 = the service endpoint, 1 = its Interrupt cap, 2 = the
    /// one-shot Reply cap the server's recv mints per call.
    const BLK_EP_CAP: usize = 0;
    const BLK_IRQ_CAP: usize = 1;
    const BLK_REPLY_SLOT: usize = 2;
```

- [ ] **Step 3: Replace `blk_component`'s self-test with a serve loop**

In `kernel/src/main.rs`, replace the entire `blk_component` function body (the part after the `mmio`/`dma` launch-arg read, currently ~1028–1058) so the function reads:

```rust
    /// The blk component (user-space virtio-blk driver), now a call/reply
    /// **server**. The kernel maps the device's MMIO + a zeroed DMA frame into
    /// it (a1 = mmio, a2 = dma) and grants an Interrupt cap for `wait_irq`. It
    /// loops: receive a block number (the call badge), read that sector into the
    /// identity-mapped DMA data page, and reply with the device status byte
    /// (0 = OK). The kernel FS reads the data straight from the DMA page.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn blk_component() -> ! {
        let mmio: usize;
        let dma: usize;
        // SAFETY: read the launch args the kernel placed in a1/a2.
        unsafe {
            core::arch::asm!("mv {m}, a1", "mv {d}, a2",
                m = out(reg) mmio, d = out(reg) dma,
                options(nomem, nostack, preserves_flags));
        }
        // SAFETY: mmio + dma are mapped RW-U into this task; the sequence is the
        // spike-verified virtio-blk bring-up, then a recv/read/reply loop.
        unsafe {
            virtio_queue_init(mmio, dma);
            let mut avail_idx: u16 = 0;
            loop {
                let block = sys_recv(BLK_EP_CAP, BLK_REPLY_SLOT); // badge = block #
                let status = blk_req(mmio, dma, false, avail_idx, block as u64);
                avail_idx = avail_idx.wrapping_add(1);
                sys_reply(BLK_REPLY_SLOT, status as usize);
            }
        }
    }
```

(The write path in `blk_req` is retained but unexercised in 6b; 6c uses it.)

- [ ] **Step 4: Verify it compiles for the bare target**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: FAIL — `blk_consumer` still references the removed `BLK_REPORT_CAP`, and `sys_send4`/`sys_exit` in the old `blk_component` are gone. That is expected; Task 6 removes `blk_consumer` and rewires `kmain`. (If you prefer a green checkpoint, do Steps 1–3 here and Task 6 before building.)

- [ ] **Step 5: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat(blk): driver becomes a call/reply read-block server (sector param)"
```

---

## Task 6: The kernel filesystem + `kmain` rewiring (turns the smoke green)

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Add FS statics and rename the blk kernel-task stack**

In `kernel/src/main.rs`, find the `KS_BLKC` stack static (line ~330):

```rust
    static mut KS_BLK: KStack = [0; TASK_STACK];
    static mut KS_BLKC: KStack = [0; TASK_STACK];
```

replace with:

```rust
    static mut KS_BLK: KStack = [0; TASK_STACK];
    static mut KS_FS: KStack = [0; TASK_STACK];
```

Then add the FS runtime statics. Put them right after the `BLK_REPLY_SLOT` const added in Task 5 Step 2:

```rust
    /// Physical address of the blk DMA frame (identity-mapped); the FS reads
    /// sector bytes from `BLK_DMA_PA + BLK_DATA_OFF`. Set by `kmain`.
    static mut BLK_DMA_PA: usize = 0;
    /// The block currently resident in the DMA data page (the one-block cache);
    /// `-1` = none. Re-reading the same block skips the IPC round-trip.
    static mut FS_CACHED_BLOCK: i64 = -1;
    /// Kernel buffer a read file is copied into (out of the reused DMA page).
    const FS_FILEBUF_LEN: usize = 4096;
    static mut FS_FILEBUF: [u8; FS_FILEBUF_LEN] = [0; FS_FILEBUF_LEN];
```

- [ ] **Step 2: Add `read_block`, `read_file`, and `fs_task`; remove `blk_consumer`**

In `kernel/src/main.rs`, delete the entire `blk_consumer` function (currently ~1061–1075) and replace it with:

```rust
    /// The FS↔device boundary: read block `n` via the blk server into the
    /// identity-mapped DMA data page and return a view of it. `None` on a device
    /// I/O error. A trivial one-block cache skips the IPC if `n` is resident.
    fn fs_read_block(n: u32) -> Option<&'static [u8]> {
        // SAFETY: BLK_DMA_PA is the kernel-allocated, identity-mapped DMA frame;
        // the slice addresses real RAM. Single hart; the cache state is ours.
        unsafe {
            if FS_CACHED_BLOCK != n as i64 {
                let status = sched::call_message(BLK_EP_CAP, n as usize);
                if status != 0 {
                    FS_CACHED_BLOCK = -1;
                    return None;
                }
                FS_CACHED_BLOCK = n as i64;
            }
            Some(core::slice::from_raw_parts(
                (BLK_DMA_PA + virtio::BLK_DATA_OFF) as *const u8,
                virtio::BLK_SECTOR_SIZE,
            ))
        }
    }

    /// Locate `name` in the filesystem and copy its contents into `FS_FILEBUF`.
    /// Returns the file's contents on success, or `None` (with a logged reason).
    fn fs_read_file(name: &str) -> Option<&'static [u8]> {
        use kernel_common::fs;
        let sb = match fs::Superblock::decode(fs_read_block(0)?) {
            Some(sb) => sb,
            None => {
                println!("fs: bad superblock");
                return None;
            }
        };
        // Copy the directory block out before later reads clobber the DMA page.
        let mut dir = [0u8; fs::BLOCK_SIZE];
        dir.copy_from_slice(fs_read_block(sb.dir_block)?);
        let ent = match fs::lookup(&dir, sb.dir_entries, name) {
            Some(e) => e,
            None => {
                println!("fs: file '{}' not found", name);
                return None;
            }
        };
        let len = ent.byte_len as usize;
        if len > FS_FILEBUF_LEN {
            println!("fs: file '{}' too large ({} bytes)", name, len);
            return None;
        }
        let nblocks = fs::block_count(ent.byte_len) as usize;
        // SAFETY: single hart; FS_FILEBUF is ours for the duration of this read.
        unsafe {
            for i in 0..nblocks {
                let blk = match fs_read_block(ent.start_block + i as u32) {
                    Some(b) => b,
                    None => {
                        println!("fs: block read error");
                        return None;
                    }
                };
                let off = i * fs::BLOCK_SIZE;
                let take = core::cmp::min(fs::BLOCK_SIZE, len - off);
                FS_FILEBUF[off..off + take].copy_from_slice(&blk[..take]);
            }
            Some(&FS_FILEBUF[..len])
        }
    }

    /// The kernel filesystem task: reads a known file by name off the disk
    /// (through the blk server) and prints its length and contents, then idles.
    extern "C" fn fs_task() -> ! {
        let name = "KB-0005";
        match fs_read_file(name) {
            Some(bytes) => {
                println!("fs: read '{}' ({} bytes)", name, bytes.len());
                if let Ok(text) = core::str::from_utf8(bytes) {
                    println!("{}", text);
                }
            }
            None => println!("fs: read of '{}' failed", name),
        }
        loop {
            sched::yield_now();
            // SAFETY: wait for the next interrupt between yields.
            unsafe { core::arch::asm!("wfi") };
        }
    }
```

- [ ] **Step 3: Rewire `kmain`'s blk block (server first, then the FS task)**

In `kernel/src/main.rs`, replace the entire `if let Some(blk) = blk_base { ... } else { ... }` block (currently ~196–221) with:

```rust
        // Phase 6b — a minimal filesystem. The blk driver is now a call/reply
        // server (recv block N -> virtio read into its identity-mapped DMA page
        // -> reply). The kernel `fs` task is the client: it calls the server to
        // read blocks, finds a file by name, and prints its contents off disk.
        // Spawn the server first (lower slot) so it recv-blocks before fs calls.
        if let Some(blk) = blk_base {
            let dma_pa = mem::frame::alloc_zeroed().expect("no DMA frame for blk").0;
            // SAFETY: set once at boot before the fs task runs; single hart.
            unsafe { BLK_DMA_PA = dma_pa; }

            let bu = ustack(core::ptr::addr_of!(US_BLK) as usize);
            let blkdev = sched::spawn_user("blk", blk_component, bu.1,
                core::ptr::addr_of!(KS_BLK) as usize + TASK_STACK,
                mem::build_virtio_space(bu, (blk, blk + 0x1000), (dma_pa, dma_pa + 0x1000)));
            sched::grant_cap(blkdev, BLK_EP_CAP, Capability::Endpoint(BLK_EP));

            let n = machine.virtio_mmio_count;
            let blk_irq = virtio::irq_for_base(&machine.virtio_mmio[..n], &machine.virtio_mmio_irq[..n], blk)
                .expect("blk has no IRQ in the device tree");
            sched::grant_cap(blkdev, BLK_IRQ_CAP, Capability::Interrupt(blk_irq));
            sched::set_launch_args(blkdev, blk, dma_pa);
            // PLIC setup (idempotent if the rng path already did init + sie).
            plic::init(machine.plic_base);
            plic::set_priority(blk_irq, 1);
            plic::enable(blk_irq);
            // SAFETY: the trap handler and the PLIC are set up to service it.
            unsafe { csr::sie_enable_external() };

            // The kernel filesystem client, holding the service endpoint cap.
            let fs = sched::spawn("fs", fs_task,
                core::ptr::addr_of!(KS_FS) as usize + TASK_STACK);
            sched::grant_cap(fs, BLK_EP_CAP, Capability::Endpoint(BLK_EP));
        } else {
            println!("blk: no virtio-blk device found");
        }
```

- [ ] **Step 4: Build for the bare target**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: PASS (no references to `blk_consumer`, `BLK_REPORT_CAP`, or `KS_BLKC` remain).

- [ ] **Step 5: Run the full smoke test (green)**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS` — including `fs: read 'KB-0005' (... bytes)` and `FS-6B-TAIL-OK` in the serial log, with every prior milestone still present (the retired `blk: sector round-trip ok` is gone).

- [ ] **Step 6: Run the host test suite (nothing regressed)**

Run: `cargo test`
Expected: PASS (workspace host tests, including `kernel-common` and `mkfs`).

- [ ] **Step 7: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat(fs): kernel filesystem reads a named file off disk via the blk server"
```

---

## Task 7: Documentation (learning note, roadmap, glossary)

**Files:**
- Create: `docs/learning/0023-minimal-filesystem.md`
- Modify: `docs/roadmap/roadmap.md`
- Modify: `docs/glossary.md`

- [ ] **Step 1: Write the learning note**

Create `docs/learning/0023-minimal-filesystem.md` (keep it short — summary, not tutorial, per the project's learning-note style):

```markdown
# 0023 — A minimal filesystem

**One-line:** the kernel can read a file **by name** off the disk — a tiny
read-only filesystem over the Phase 6a block driver, the layer between raw
sectors and the on-disk knowledge base.

## What changed
- A shared `no_std` format (`kernel-common::fs`): a **superblock** (block 0)
  locating a flat **directory** (block 1) of fixed-size entries, each mapping a
  name to a contiguous **extent** (start block + byte length); file data in
  block 2+. Defined once, host-tested, used by both the kernel and `mkfs`.
- A host `mkfs` tool builds the disk image from named files using that format.
- The `blk` component stopped self-testing and became a **call/reply server**:
  receive a block number, read that sector into its identity-mapped DMA page,
  reply. A new kernel-side `call_message` lets a kernel task be the IPC client.
- The kernel filesystem reads through one door — `read_block(n)` (with a
  one-block cache) — then parses the superblock, scans the directory, and copies
  the file's extent out of the DMA page.

## The idea worth keeping
**The FS speaks only `read_block(n)`; the driver does the real I/O.** That one
boundary is the whole relationship between a filesystem and a block device: the
FS reasons about superblocks, directories, and extents in terms of numbered
blocks, and never touches virtqueues or interrupts. Underneath, one
`call_message` to the user-space driver fetches the block into shared,
identity-mapped DMA memory the kernel reads directly — no copy through IPC.

## Why this is step two of a real knowledge base
6a was the disk; 6b is the filesystem over it. 6c points the self-healer's
loader at this FS so it reads `knowledge-base/entries/*.md` from disk and
diagnoses against the real, runtime knowledge base instead of the compiled-in
`KB-0005` stub. The format already carries a length per file (multi-block
extents work today) and reserves dirent space for the write/append path 6c may
add.

## Proof
`fs: read 'KB-0005' (<n> bytes)` followed by the file's contents — including a
marker that lives in the file's **second** block — read off a virtio-blk image
built by `mkfs`, through the unprivileged driver.
```

- [ ] **Step 2: Mark Phase 6b done in the roadmap**

In `docs/roadmap/roadmap.md`, change the Phase 6b heading and add a done-line. Replace:

```markdown
### Phase 6b — A minimal filesystem

- **Goal:** a simple filesystem over the block device — locate and read a file
  by name from a disk image built at boot time.
- **You learn:** the on-disk layout (superblock / directory / file extents) and
  the block-cache boundary between a filesystem and a block device.
- **Done when:** the kernel reads a named file's contents off the disk.
```

with:

```markdown
### Phase 6b — A minimal filesystem  *(done — 2026-06-24)*

- **Goal:** a simple filesystem over the block device — locate and read a file
  by name from a disk image built at boot time.
- **You learn:** the on-disk layout (superblock / directory / file extents) and
  the block-cache boundary between a filesystem and a block device (see
  [learning note 0023](../learning/0023-minimal-filesystem.md)).
- **Done when:** `./tools/test-qemu.ps1` (its disk image now built by the host
  `mkfs` tool) shows the kernel locate a file by name and read its multi-block
  contents off the disk — the `blk` driver now a call/reply read-block server,
  the kernel filesystem its client. QEMU-only.
```

- [ ] **Step 3: Add the new glossary terms**

In `docs/glossary.md`, add entries for **filesystem**, **superblock**, **directory entry**, **extent**, and **block cache** (follow the file's existing format/alphabetization; only add terms not already present). Example wording:

```markdown
- **Block cache** — a small in-memory hold of recently-read disk blocks so the
  filesystem can re-read a block without a fresh device request. Phase 6b's is a
  single block (the one resident in the DMA page).
- **Directory entry** — a fixed-size record mapping a file name to its on-disk
  location (start block + byte length). Phase 6b packs these in one block.
- **Extent** — a contiguous run of blocks holding a file's data; a file is one
  extent (start block + length) in Phase 6b's format.
- **Filesystem** — the layer that turns named files into block reads/writes over
  a block device. Phase 6b's is minimal and read-only.
- **Superblock** — the filesystem's first block: format magic/version and the
  location of the directory.
```

- [ ] **Step 4: Verify cross-references**

Run: `pwsh ./tools/check-references.ps1`
Expected: PASS (the learning-note link and any KB/spec references resolve).

- [ ] **Step 5: Commit**

```bash
git add docs/learning/0023-minimal-filesystem.md docs/roadmap/roadmap.md docs/glossary.md
git commit -m "docs: Phase 6b minimal filesystem - learning note, roadmap, glossary"
```

---

## Self-Review

**Spec coverage** (each §3 deliverable → task):
- On-disk format + parser (§3.1, §3.2) → Task 1.
- `mkfs` host tool (§3.1, §5.2) → Task 2.
- `blk` server / `blk_req` sector param (§3.3) → Task 5.
- Kernel-side `call_message` (§3.4) → Task 4.
- `read_block` + one-block cache + `read_file` (§3.5) → Task 6.
- Retire the 6a self-test, swap the assertion (§3.6) → Tasks 3 + 6.
- Error handling (§3.7: bad superblock, not found, I/O error, too large, no device) → Task 6 (`fs_read_file`/`fs_read_block` logged paths) + the `else` branch.
- Testing (§4: host unit tests, mkfs round-trip, QEMU smoke with a two-block file + tail marker) → Tasks 1, 2, 3, 6.
- Deliverables 7–9 (learning note, roadmap, glossary) → Task 7.

**Placeholder scan:** none — every code/Step shows full content; no "TBD"/"add error handling"/"similar to".

**Type consistency:** `Superblock`/`DirEntry`/`lookup`/`block_count`/`BLOCK_SIZE`/`DIRENT_SIZE`/`DIRENTS_PER_BLOCK`/`DATA_START_BLOCK`/`DIR_BLOCK`/`NAME_LEN`/`FS_MAGIC`/`FS_VERSION` are defined in Task 1 and used identically in Tasks 2 and 6. `BLK_EP_CAP`/`BLK_IRQ_CAP`/`BLK_REPLY_SLOT`/`BLK_EP` defined in Task 5 Step 2 and used in Tasks 5–6. `call_message(cap_idx, badge) -> usize` defined in Task 4, called in Task 6. `blk_req(mmio, dma, write, avail_idx, sector)` re-signatured in Task 5 and called in Task 5's server loop. `fs_read_block`/`fs_read_file`/`fs_task`/`BLK_DMA_PA`/`FS_CACHED_BLOCK`/`FS_FILEBUF`/`FS_FILEBUF_LEN`/`KS_FS` all defined and used within Task 6. The smoke marker `FS-6B-TAIL-OK` and name `KB-0005` match between Task 2 (`mkfs` demo) and Tasks 3/6 (assertion + `fs_task`).
