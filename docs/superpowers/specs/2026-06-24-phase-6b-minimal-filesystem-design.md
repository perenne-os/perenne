# Kernel — Phase 6b Design: a minimal filesystem

- **Date:** 2026-06-24
- **Status:** Draft — awaiting review
- **Scope of this document:** Phase 6b only — a minimal, **read-only** filesystem
  over the Phase 6a virtio-blk driver. The kernel locates a file **by name** in a
  disk image built at boot and reads its contents off the disk. The second
  sub-phase of the Phase 6 arc (persistent storage & the living knowledge base).
  Fully QEMU-testable (reuses 6a's `-drive` + `virtio-blk-device`, but the image
  is now a real filesystem rather than 64 KiB of zeros).

---

## 0. Where this sits

Phase 6 makes the self-healing knowledge organism **real** — it reads its actual
knowledge base from persistent storage instead of the compiled-in `KB-0005`
stub. The arc is **6a block storage → 6b a minimal filesystem → 6c the living
knowledge base**. 6a gave us a user-space `blk` driver that round-trips a sector
over a virtqueue, with its DMA frame **identity-mapped** so the kernel reads
sector bytes straight from the physical page. 6b puts a filesystem on top: a
named file is found and read. 6c then points the self-healer's loader at it.

This phase realizes two things 6a explicitly deferred:
- **A block-I/O IPC service** — the `blk` component stops self-testing and
  becomes a `call`-driven **server**: "read block N" → it performs the virtio
  read into the identity-mapped DMA page → replies "done". The kernel filesystem
  is the **client**.
- **The block-cache boundary** — the filesystem's only door to the device is a
  single `read_block(n)` function (backed by a trivial one-block cache).

## 1. Goal

A kernel-side filesystem reads a named file's contents from a disk image. The
on-disk format is a deliberately minimal custom layout — a **superblock**, a
flat **directory** of fixed-size entries, and **contiguous file extents** — that
teaches exactly the concepts named in the roadmap (superblock / directory / file
extents) and the FS↔block-device boundary, with the least code.

**You learn (kept brief):** a simple on-disk layout (superblock locates the
directory; a directory entry maps a name to a start block + byte length; a file
is a contiguous run of blocks), and the boundary between a filesystem and a
block device — the FS speaks only `read_block(n)`, and underneath that one call
the block driver does the real virtio I/O.

**Done when** `./tools/test-qemu.ps1` (booting with a virtio-blk `-drive` whose
backing image is now a real filesystem) observes, alongside every existing
milestone **except** the retired 6a self-test line (see §3.6):

1. **A named file read off disk** — the kernel filesystem looks up a known file
   by name in the superblock/directory, reads its (multi-block) extent through
   the `blk` driver, and logs the file's length and contents; the smoke asserts
   a distinctive marker that lives in the file's **second** block (proving a
   multi-block extent read, not just block 0).

And off the bare target:

2. **Host unit tests** — the on-disk format round-trips (a `mkfs`-built image
   parses back to the same files) and `lookup` finds present names / rejects
   absent ones; the `mkfs` tool and the kernel parser share one layout module.

## 2. Non-goals (deferred)

- **Writing / creating files at runtime** — 6b is read-only. The image is built
  on the host by `mkfs`. Runtime append (to "record a new KB issue") is a 6c
  stretch, and the format already reserves room for it (a dirent carries a
  length; the superblock carries a file count).
- **Subdirectories** — a single flat directory. Names are leaf names.
- **Free-space management / allocation** — there is no allocator; `mkfs` lays
  files out contiguously and the kernel only reads.
- **A real block cache** — only a one-block "last block read" cache to make the
  FS↔device boundary concrete. Eviction, multi-block caching, and write-back are
  deferred until throughput matters.
- **A standard filesystem (FAT/ext2)** — those teach their own on-disk quirks
  (cluster chains, inodes) without serving 6b's goal better; a custom minimal
  format is smaller and hits the learning target directly.
- **Multi-sector virtio requests / scatter-gather** — each `read_block` is still
  one 512-byte sector per virtio request (as in 6a). A multi-block file is read
  one block at a time.

## 3. Design

### 3.1 Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| On-disk format + parser | `libs/common` (new `fs` module, `no_std`, host-tested) | Layout constants, the `Superblock`/`DirEntry` structs, their fixed byte encodings, and pure functions: `parse_superblock(&[u8]) -> Option<Superblock>` and `lookup(dir_bytes, name) -> Option<DirEntry>`. No I/O, no arch code. |
| `mkfs` host tool | `tools/mkfs` (new host Rust bin) | Builds a raw disk image from a set of `(name, bytes)` files, using the **same** `libs/common::fs` layout. Writes superblock, directory, and contiguous file data. |
| The `blk` **server** | `kernel/src/main.rs` (`blk_component`) | Now a U-mode server: loop `recv` (badge = block number) → `blk_req` read of that sector into the DMA data page → `reply` (badge = status). Drops the 6a self-test. |
| Kernel-side `call` | `arch/riscv64/src/sched.rs` | `call_message(cap_idx, badge) -> reply_badge`: the S-mode counterpart to `recv_message`, so a kernel task can be an IPC **client** (send + block for reply), reusing the existing call/reply + one-shot-reply-cap machinery. |
| The filesystem | `kernel/src/main.rs` (new `fs_task`) | The FS↔device door `read_block(n)` (calls the `blk` server, then reads the identity-mapped DMA page; one-block cache); plus `read_file(name)` which parses the superblock, reads the directory, looks the name up, and reads the file's extent. Logs the result. |
| Wiring | `kernel/src/main.rs` `kmain` | As 6a, but grant the FS task the blk endpoint cap (it is now the **caller**), spawn it instead of the old result-consumer, and keep the blk component as the server holding the same endpoint. |

### 3.2 On-disk layout (512-byte blocks)

Block size = virtio sector size = **512 bytes** (so one FS block = one virtio
request, no block/sector translation). All multi-byte fields little-endian.

```
block 0   superblock
block 1   directory (one block = up to 8 entries of 64 bytes)
block 2+  file data, each file a contiguous run of blocks
```

**Superblock** (within block 0; the rest of the block is zero):
```
magic:        u32   // FS_MAGIC, identifies the format
version:      u32   // = 1
block_size:   u32   // = 512
dir_block:    u32   // = 1
dir_entries:  u32   // number of valid directory entries
total_blocks: u32   // image size in blocks (for sanity/bounds)
```

**DirEntry** (64 bytes; 8 per directory block):
```
name:        [u8; 48]   // NUL-padded leaf name
start_block: u32        // first data block of the file
byte_len:    u32        // exact file length in bytes
_reserved:   [u8; 8]    // zero (room for 6c: flags/created, etc.)
```

A file occupies `ceil(byte_len / 512)` contiguous blocks from `start_block`; the
final block is partially used (`byte_len % 512`). One directory block (8 entries)
is ample for 6b and 6c; multi-block directories are a later concern.

### 3.3 The `blk` server (was the 6a self-test)

The component keeps 6a's handshake, queue setup, and `blk_req` — but `blk_req`
gains a `sector: u64` parameter (6a hardcoded sector 0), and the body becomes a
serve loop instead of a one-shot self-test:

```
loop {
    let n = recv(BLK_EP, reply_slot);   // badge = block number to read
    let status = blk_req(mmio, dma, write=false, sector=n);  // fills DMA data page
    reply(reply_slot, badge = status);  // 0 = OK
}
```

The component never exits (a long-lived driver). It still blocks on its IRQ via
`wait_irq` inside `blk_req`, exactly as in 6a. It owns the MMIO + DMA mapping; the
data crosses to the kernel not through the IPC payload but through the
**identity-mapped DMA page** (`dma_pa + BLK_DATA_OFF`), which the kernel reads
directly. The write path in `blk_req` is retained (unused in 6b) for 6c.

### 3.4 Kernel-side `call` (the new IPC plumbing)

The FS is a kernel (S-mode) task and must *initiate* a request, which the U-mode
`sys_call` path cannot do for it. We add the S-mode counterpart to the existing
`recv_message`:

```
sched::call_message(cap_idx, badge) -> reply_badge
```

It performs an atomic send-and-await-reply against the endpoint: deliver the
badge to the waiting server (or block as a sender until it `recv`s), mint the
one-shot Reply capability the server replies through, then block the kernel task
until that reply arrives and return the reply badge. This reuses the call/reply
rendezvous and the one-shot reply-cap mechanism built in the reply-capabilities
increment; it adds only the kernel-task entry points (mirroring how
`recv_message` mirrors the `recv` syscall).

### 3.5 The filesystem (`read_block` and `read_file`)

`read_block(n) -> &[u8; 512]` — the single FS↔device boundary:
```
if n == cached_block { return &dma_page[BLK_DATA_OFF..][..512]; }   // one-block cache
let status = sched::call_message(BLK_EP_CAP, n);                    // ask the driver
assert status == 0;                                                // else surface I/O error
cached_block = n;
&dma_page[BLK_DATA_OFF..][..512]                                    // identity-mapped DMA
```

`read_file(name) -> (len, &[u8])`:
```
sb  = parse_superblock(read_block(0))           // verify FS_MAGIC/version
dir = read_block(sb.dir_block)                  // one directory block
ent = fs::lookup(dir, name)                     // linear scan → DirEntry or None
for i in 0..ceil(ent.byte_len/512):             // read the contiguous extent
    copy read_block(ent.start_block + i) into a kernel file buffer
return first ent.byte_len bytes
```

`fs_task` (kernel task) calls `read_file` for the known test name and logs the
length + contents (and the tail marker the smoke asserts). The file buffer is a
fixed-size kernel static sized for 6b/6c needs (a few KiB).

### 3.6 The 6a self-test is retired (and replaced by a stronger proof)

6a's `blk: sector round-trip ok` was scaffolding to prove the driver in
isolation. 6b supersedes it: the FS reading real sectors **through** the driver
is a strictly stronger proof that the driver works. The smoke test drops the
`blk: sector round-trip ok` assertion and asserts the new `fs:` read line
instead. (The write path remains in `blk_req`, just unexercised until 6c.)

### 3.7 Error handling summary

| Situation | Behavior |
|-----------|----------|
| No virtio-blk device (or smoke without `-drive`) | FS not spawned; `kmain` logs `blk: no virtio-blk device found`; boot continues (as 6a). |
| Superblock magic/version wrong | `fs: bad superblock` logged; FS task stops (no file read). Smoke would fail — image is built by `mkfs`, so this only fires on a real bug. |
| Name not found in the directory | `fs: file '<name>' not found` logged. |
| `read_block` status ≠ 0 (device I/O error) | `fs: block read error` logged; surfaced, not silently ignored. |
| File extent exceeds `total_blocks` or the kernel buffer | Truncated to the buffer / bounds-checked; `fs: file too large` logged. |
| Component touches memory it doesn't own | Contained + diagnosed as any U-mode fault (5a). |

## 4. Testing

- **Host unit tests** (`libs/common::fs`): encode a `Superblock` and a couple of
  `DirEntry`s, parse them back, assert equality (round-trip); `lookup` finds a
  present name and a name in the second slot, and returns `None` for an absent
  name and for an empty directory; `byte_len` → block-count math
  (`ceil(len/512)`), including exact-multiple and off-by-one boundaries.
- **Host test for `mkfs`**: build an in-memory image from two files (one
  spanning two blocks), then parse it with `libs/common::fs` and confirm both
  files' names, lengths, and bytes come back intact — proving `mkfs` and the
  kernel parser agree on the format.
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first, failing):
  build the disk image with `mkfs` (replacing the 64 KiB-of-zeros scratch file)
  containing a test file whose contents span **two** blocks with a distinctive
  marker near the end (in block 2); pass it via the existing `-drive` +
  `virtio-blk-device`; assert the `fs:` read line including that tail marker;
  drop the retired `blk: sector round-trip ok` assertion; keep every other
  milestone (rng + blk coexist by DeviceID, as in 6a).

## 5. Deliverables

1. `libs/common`: the `fs` layout module (`Superblock`, `DirEntry`, encode/decode,
   `parse_superblock`, `lookup`, block-count math) + host tests.
2. `tools/mkfs`: a host Rust bin that builds a raw image from named files using
   `libs/common::fs` + a host test (build → parse round-trip).
3. `arch/riscv64/src/sched.rs`: `call_message` (kernel-side IPC client).
4. `kernel/src/main.rs`: `blk_req` gains a `sector` param; `blk_component`
   becomes a serve loop; the new `fs_task` (`read_block` + one-block cache +
   `read_file`); `kmain` wiring (grant the FS task the blk endpoint, spawn it in
   place of the old consumer); `MAX_TASKS`/buffer statics as needed.
5. `tools/test-qemu.ps1`: build the image with `mkfs`; assert the `fs:` line;
   drop the retired round-trip assertion.
6. Host tests + smoke, all green.
7. Short learning note `docs/learning/0023-minimal-filesystem.md`.
8. Roadmap: Phase 6b marked done.
9. Glossary: only genuinely new terms (filesystem, superblock, directory entry,
   extent, block cache).

## 6. Open questions (for 6c / later)

- **The KB loader** — 6c parses `knowledge-base/entries/*.md` read via this FS and
  diagnoses against them; this phase only proves "read a named file."
- **Runtime write/append** — recording a newly-seen issue back to disk (6c
  stretch) needs a write path (`blk_req` already has one) and minimal free-block
  tracking (a next-free-block cursor in the superblock).
- **A real block cache** and multi-block reads once the KB is large enough to
  care about throughput.
