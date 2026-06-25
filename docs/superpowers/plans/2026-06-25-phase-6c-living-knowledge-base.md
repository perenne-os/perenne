# Phase 6c — The Living Knowledge Base — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:executing-plans or superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The self-healer diagnoses a contained crash against a knowledge-base entry loaded and parsed from disk at boot, retiring the compiled-in `KB_0005` stub.

**Architecture:** A pure `no_std` frontmatter parser (`kernel_common::kb`) turns a KB `.md` file's bytes into a runtime record. At boot a kernel KB-loader task enumerates the on-disk directory, reads each entry through the existing blk-server FS path, parses it, and installs the tokened ones into a runtime table in `heal`. `heal::diagnose` (still a pure crash-path lookup) selects the entry whose on-disk `match-cause` token matches the trap cause. The crash-prone "patient" tasks are gated on a KB-ready rendezvous so they cannot crash before the table exists. `mkfs` now packs the real `knowledge-base/entries/*.md`.

**Tech Stack:** Rust (`no_std` kernel + host tests), RISC-V64, QEMU `virt`, virtio-blk.

## Global Constraints

- `libs/common` is `#![cfg_attr(not(test), no_std)]` — no allocation, no I/O in `kb.rs` or `fs.rs`.
- `arch/riscv64` does **not** depend on `kernel_common`; `heal::install` takes primitive `&str` args, not `KbRecord`.
- Single hart; `static mut` is accessed via `core::ptr::addr_of[_mut]!` with SAFETY comments (existing pattern).
- Git identity Kathir <kathirpsmy@gmail.com>; no Claude co-author; commit directly on branch `phase-6c-living-knowledge-base`.
- New endpoint id `GATE_EP = 5` (0 rtc, 1 crash, 2 entropy, 3 defer, 4 blk are taken).
- Smoke test always supplies the blk `-drive`; the loader still releases the patients when no disk is present (graceful degradation).

---

### Task 1: Add the `match-cause` field to the schema and KB-0005

**Files:**
- Modify: `knowledge-base/schema/issue-record.md`
- Modify: `knowledge-base/entries/KB-0005.md`

- [ ] **Step 1: Document the field** in `issue-record.md` — add a row to the Fields table after `playbook`:

```markdown
| `match-cause` | string | *(optional)* Machine-matchable token tying this issue to a runtime fault class the kernel can diagnose. Vocabulary: `page-fault` (a load/store/instruction page fault). Absent ⇒ not runtime-matchable. |
```

And add to Conventions:

```markdown
- `match-cause` is the only field the in-kernel self-healer matches on at runtime; it must be one of the documented tokens, added together with a kernel `Cause→token` arm.
```

- [ ] **Step 2: Add the token to KB-0005** — in `knowledge-base/entries/KB-0005.md` frontmatter, after the `component:` line add:

```yaml
match-cause: page-fault
```

Bump `updated: 2026-06-25`.

- [ ] **Step 3: Update the Notes** of `KB-0005.md` final paragraph to reflect 6c (the entry is now read from disk, not compiled): replace the last sentence ("Loading this `.md` at runtime … awaits a filesystem.") with:

```markdown
Phase 6c makes this real: the kernel reads this `.md` off the disk image at
boot, parses its frontmatter (`kernel_common::kb`), and installs it into the
self-healer's runtime table — so a contained crash is now diagnosed against
this on-disk entry (selected by `match-cause: page-fault`), not a compiled-in
copy.
```

- [ ] **Step 4: Verify references** still resolve:

Run: `pwsh -File tools/check-references.ps1`
Expected: no problems reported.

- [ ] **Step 5: Commit**

```bash
git add knowledge-base/schema/issue-record.md knowledge-base/entries/KB-0005.md
git commit -m "feat(kb): add machine-matchable match-cause token to the KB schema"
```

---

### Task 2: The `kb` frontmatter parser (host-tested)

**Files:**
- Create: `libs/common/src/kb.rs`
- Modify: `libs/common/src/lib.rs` (add `pub mod kb;`)
- Test: in `libs/common/src/kb.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `kernel_common::kb::KbRecord<'a> { id: &'a str, title: &'a str, playbook: &'a str, match_cause: Option<&'a str> }` and `kernel_common::kb::parse(bytes: &[u8]) -> Option<KbRecord<'_>>`.

- [ ] **Step 1: Write the failing tests** in `libs/common/src/kb.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "---\n\
id: KB-0042\n\
title: \"A sample issue\"\n\
component: test\n\
match-cause: page-fault\n\
symptoms:\n\
  - \"It logs: something bad happened\"\n\
playbook:\n\
  - \"Do the first reversible thing\"\n\
  - \"Then the second\"\n\
verification: \"It works\"\n\
---\n\
\n## Notes\nfree text\n";

    #[test]
    fn parses_the_runtime_fields() {
        let r = parse(SAMPLE.as_bytes()).expect("parses");
        assert_eq!(r.id, "KB-0042");
        assert_eq!(r.title, "A sample issue");
        assert_eq!(r.playbook, "Do the first reversible thing");
        assert_eq!(r.match_cause, Some("page-fault"));
    }

    #[test]
    fn match_cause_is_optional() {
        let no_token = SAMPLE.replace("match-cause: page-fault\n", "");
        let r = parse(no_token.as_bytes()).expect("parses");
        assert_eq!(r.match_cause, None);
        assert_eq!(r.id, "KB-0042");
    }

    #[test]
    fn rejects_a_file_without_frontmatter() {
        assert!(parse(b"no frontmatter here").is_none());
    }

    #[test]
    fn rejects_when_required_fields_missing() {
        // has id but no title/playbook
        assert!(parse(b"---\nid: KB-0001\n---\n").is_none());
    }

    #[test]
    fn parses_the_real_kb_0005() {
        let bytes = include_str!("../../../knowledge-base/entries/KB-0005.md");
        let r = parse(bytes.as_bytes()).expect("real KB-0005 parses");
        assert_eq!(r.id, "KB-0005");
        assert_eq!(r.match_cause, Some("page-fault"));
        assert!(r.playbook.starts_with("Restart the component"));
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p kernel-common kb`
Expected: FAIL (module `kb` does not exist).

- [ ] **Step 3: Implement** `libs/common/src/kb.rs`:

```rust
//! Minimal parser for the runtime subset of a knowledge-base entry's YAML
//! frontmatter (see `knowledge-base/schema/issue-record.md`). Pure, `no_std`,
//! no allocation, no I/O — host-tested and used by the in-kernel KB loader.
//! It is deliberately not a general YAML parser: it reads the few scalar
//! fields the self-healer needs and skips everything else.

/// The runtime-relevant fields of one `knowledge-base/entries/*.md` record,
/// borrowed from the source bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KbRecord<'a> {
    pub id: &'a str,
    pub title: &'a str,
    /// The first playbook step — the actionable line.
    pub playbook: &'a str,
    /// The machine-matchable fault token, if the entry declares one.
    pub match_cause: Option<&'a str>,
}

/// Trim whitespace and strip a single pair of surrounding ASCII double quotes.
fn clean(v: &str) -> &str {
    let v = v.trim();
    let v = v.strip_prefix('"').unwrap_or(v);
    v.strip_suffix('"').unwrap_or(v).trim()
}

/// Parse the frontmatter of a KB entry. Returns `None` unless `id`, `title`,
/// and a first `playbook` step are all present, or if the file does not open
/// with a `---` fence. `match_cause` is optional.
pub fn parse(bytes: &[u8]) -> Option<KbRecord<'_>> {
    let text = core::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    if lines.next()?.trim() != "---" {
        return None; // must open with a frontmatter fence
    }
    let mut id = None;
    let mut title = None;
    let mut playbook = None;
    let mut match_cause = None;
    let mut in_playbook = false;
    for line in lines {
        if line.trim() == "---" {
            break; // end of frontmatter
        }
        // Capture the first list item under `playbook:`.
        if in_playbook && playbook.is_none() {
            if let Some(item) = line.trim_start().strip_prefix("- ") {
                playbook = Some(clean(item));
                continue;
            }
        }
        if let Some((key, value)) = line.split_once(':') {
            match key.trim() {
                "id" => id = Some(clean(value)),
                "title" => title = Some(clean(value)),
                "match-cause" => match_cause = Some(clean(value)),
                "playbook" => in_playbook = true,
                _ => {}
            }
        }
    }
    Some(KbRecord { id: id?, title: title?, playbook: playbook?, match_cause })
}
```

Add `pub mod kb;` to `libs/common/src/lib.rs` next to `pub mod fs;`.

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p kernel-common kb`
Expected: PASS (5 tests).

- [ ] **Step 5: Commit**

```bash
git add libs/common/src/kb.rs libs/common/src/lib.rs
git commit -m "feat(kb): host-tested frontmatter parser for KB entries"
```

---

### Task 3: `heal` runtime table + data-driven diagnose

**Files:**
- Modify: `arch/riscv64/src/heal.rs` (rewrite)
- Modify: `arch/riscv64/src/sched.rs:421-425` (call sites use accessor methods)
- Test: in `arch/riscv64/src/heal.rs` `#[cfg(test)] mod tests`

**Interfaces:**
- Produces: `heal::install(id: &str, title: &str, playbook: &str, match_cause: Option<&str>) -> bool`; `heal::loaded_count() -> usize`; `heal::diagnose(cause: Cause) -> Option<&'static KnownIssue>`; `KnownIssue::{id,title,playbook}(&self) -> &str`.
- Consumes (in `sched.rs`): `issue.id()`, `issue.title()`, `issue.playbook()`.

- [ ] **Step 1: Write the failing tests** (replace the existing `tests` module in `heal.rs`):

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> [Option<KnownIssue>; MAX_ISSUES] {
        let mut t: [Option<KnownIssue>; MAX_ISSUES] = Default::default();
        t[0] = Some(KnownIssue::new("KB-0099", "decoy", "do nothing", ""));
        t[1] = Some(KnownIssue::new(
            "KB-0005", "fatal fault", "Restart the component", "page-fault",
        ));
        t
    }

    #[test]
    fn page_faults_map_to_the_page_fault_token() {
        assert_eq!(cause_token(Cause::LoadPageFault), Some("page-fault"));
        assert_eq!(cause_token(Cause::StorePageFault), Some("page-fault"));
        assert_eq!(cause_token(Cause::InstructionPageFault), Some("page-fault"));
        assert_eq!(cause_token(Cause::Breakpoint), None);
    }

    #[test]
    fn match_selects_the_entry_by_its_on_disk_token() {
        let t = table();
        let hit = match_issue(&t, Cause::StorePageFault).expect("matches");
        assert_eq!(hit.id(), "KB-0005");
        assert_eq!(hit.playbook(), "Restart the component");
    }

    #[test]
    fn no_match_for_a_non_crash_cause_or_empty_table() {
        let t = table();
        assert!(match_issue(&t, Cause::SupervisorTimer).is_none());
        let empty: [Option<KnownIssue>; MAX_ISSUES] = Default::default();
        assert!(match_issue(&empty, Cause::LoadPageFault).is_none());
    }
}
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p kernel-arch-riscv64 heal`
Expected: FAIL (items not defined / types changed).

- [ ] **Step 3: Rewrite `heal.rs`** (keep the module doc, update it to say the table is loaded from disk at boot). Replace the body with:

```rust
use crate::trap::Cause;

const ID_CAP: usize = 16;
const TITLE_CAP: usize = 96;
const PLAYBOOK_CAP: usize = 256;
const TOKEN_CAP: usize = 24;
/// Runtime table capacity (one directory block holds at most 8 entries).
pub const MAX_ISSUES: usize = 8;

/// A fixed-capacity, copyable string — owns its bytes so a `KnownIssue` can
/// outlive the disk buffer it was parsed from (no allocator in the kernel).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Buf<const N: usize> {
    bytes: [u8; N],
    len: usize,
}

impl<const N: usize> Buf<N> {
    fn from_str(s: &str) -> Self {
        let mut k = s.len().min(N);
        while k > 0 && !s.is_char_boundary(k) {
            k -= 1; // never split a UTF-8 char
        }
        let mut bytes = [0u8; N];
        bytes[..k].copy_from_slice(&s.as_bytes()[..k]);
        Buf { bytes, len: k }
    }
    fn as_str(&self) -> &str {
        // bytes[..len] is a prefix of a &str cut at a char boundary -> valid.
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}

impl<const N: usize> Default for Buf<N> {
    fn default() -> Self {
        Buf { bytes: [0; N], len: 0 }
    }
}

/// A runtime knowledge record — the subset of a `knowledge-base/entries/*.md`
/// issue record the self-healer matches and reports, loaded from disk at boot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct KnownIssue {
    id: Buf<ID_CAP>,
    title: Buf<TITLE_CAP>,
    playbook: Buf<PLAYBOOK_CAP>,
    match_cause: Buf<TOKEN_CAP>,
}

impl KnownIssue {
    fn new(id: &str, title: &str, playbook: &str, match_cause: &str) -> Self {
        KnownIssue {
            id: Buf::from_str(id),
            title: Buf::from_str(title),
            playbook: Buf::from_str(playbook),
            match_cause: Buf::from_str(match_cause),
        }
    }
    pub fn id(&self) -> &str { self.id.as_str() }
    pub fn title(&self) -> &str { self.title.as_str() }
    pub fn playbook(&self) -> &str { self.playbook.as_str() }
    fn match_cause(&self) -> &str { self.match_cause.as_str() }
}

/// The runtime knowledge base — populated by `install` at boot (single hart,
/// before any gated patient can crash), then read-only.
static mut KB_TABLE: [Option<KnownIssue>; MAX_ISSUES] = [None; MAX_ISSUES];
static mut KB_COUNT: usize = 0;

/// Install a parsed KB record into the runtime table. Returns false if the
/// table is full. Called only by the boot-time loader.
pub fn install(id: &str, title: &str, playbook: &str, match_cause: Option<&str>) -> bool {
    // SAFETY: single hart; called only from the boot KB loader before the
    // gated patients run; no concurrent access.
    unsafe {
        let count = core::ptr::read(core::ptr::addr_of!(KB_COUNT));
        if count >= MAX_ISSUES {
            return false;
        }
        let issue = KnownIssue::new(id, title, playbook, match_cause.unwrap_or(""));
        let table = &mut *core::ptr::addr_of_mut!(KB_TABLE);
        table[count] = Some(issue);
        core::ptr::write(core::ptr::addr_of_mut!(KB_COUNT), count + 1);
        true
    }
}

/// Number of entries installed (for boot logging).
pub fn loaded_count() -> usize {
    // SAFETY: single hart; read of a boot-populated counter.
    unsafe { core::ptr::read(core::ptr::addr_of!(KB_COUNT)) }
}

/// Map a raw trap cause to a stable, knowledge-base-matchable token. This is
/// the kernel's job (it owns trap decoding); the *knowledge* keyed by the
/// token lives on disk. Pure, host-tested.
fn cause_token(cause: Cause) -> Option<&'static str> {
    match cause {
        Cause::LoadPageFault | Cause::StorePageFault | Cause::InstructionPageFault => {
            Some("page-fault")
        }
        _ => None,
    }
}

/// Find the loaded issue whose on-disk `match-cause` token matches `cause`.
/// Pure over the table — host-tested — so the selection is explainable.
fn match_issue(table: &[Option<KnownIssue>], cause: Cause) -> Option<&KnownIssue> {
    let token = cause_token(cause)?;
    table.iter().flatten().find(|i| i.match_cause() == token)
}

/// Diagnose a contained crash against the disk-loaded knowledge base.
/// Deterministic, allocation-free, and a pure lookup (safe in the crash path).
pub fn diagnose(cause: Cause) -> Option<&'static KnownIssue> {
    // SAFETY: single hart; the table is filled at boot (patients are gated on
    // the load) and never mutated afterwards, so this shared read is sound.
    let table = unsafe { &*core::ptr::addr_of!(KB_TABLE) };
    match_issue(table, cause)
}
```

- [ ] **Step 4: Update the `sched.rs` call sites** at `arch/riscv64/src/sched.rs:421-425` — change field access to method calls:

```rust
                match crate::heal::diagnose(cause) {
                    Some(issue) => crate::println!(
                        "heal: diagnosed {} ({}) -> playbook: {}",
                        issue.id(), issue.title(), issue.playbook()
                    ),
                    None => crate::println!("heal: no known issue for {cause:?} (recorded for triage)"),
                }
```

- [ ] **Step 5: Run tests + build the kernel**

Run: `cargo test -p kernel-arch-riscv64 heal`
Expected: PASS (3 tests).
Run: `cargo build -p kernel-arch-riscv64 --target riscv64gc-unknown-none-elf`
Expected: builds (no use of the removed `KnownIssue` fields anywhere).

- [ ] **Step 6: Commit**

```bash
git add arch/riscv64/src/heal.rs arch/riscv64/src/sched.rs
git commit -m "feat(heal): data-driven diagnosis over a runtime KB table loaded at boot"
```

---

### Task 4: `mkfs` packs the real knowledge base

**Files:**
- Modify: `tools/mkfs/src/main.rs`

- [ ] **Step 1: Rewrite `main.rs`** to read and pack the real entries:

```rust
//! Build the Phase 6c filesystem image from the real knowledge base and write
//! it to the path given as the first argument. Each `knowledge-base/entries/
//! *.md` file is packed under its id (e.g. "KB-0005"); the in-kernel loader
//! enumerates the directory and parses each entry's frontmatter.

use std::{fs, path::PathBuf};

fn entries_dir() -> PathBuf {
    // tools/mkfs -> repo root is two levels up.
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../knowledge-base/entries")
}

fn main() {
    let out = std::env::args().nth(1).expect("usage: mkfs <image-path>");
    let dir = entries_dir();

    let mut files: Vec<(String, Vec<u8>)> = Vec::new();
    for ent in fs::read_dir(&dir).expect("read knowledge-base/entries") {
        let path = ent.expect("dirent").path();
        if path.extension().and_then(|e| e.to_str()) != Some("md") {
            continue;
        }
        let id = path.file_stem().unwrap().to_string_lossy().into_owned();
        let bytes = fs::read(&path).expect("read entry");
        files.push((id, bytes));
    }
    files.sort_by(|a, b| a.0.cmp(&b.0)); // deterministic directory order
    assert!(!files.is_empty(), "no KB entries found in {}", dir.display());

    let refs: Vec<(&str, &[u8])> = files.iter().map(|(n, b)| (n.as_str(), b.as_slice())).collect();
    let img = mkfs::build_image(&refs);
    fs::write(&out, &img).expect("write image");
    eprintln!(
        "mkfs: wrote {} ({} bytes); packed {} KB entries",
        out,
        img.len(),
        refs.len()
    );
}
```

- [ ] **Step 2: Build + run mkfs** to a scratch image and confirm it packs the entries:

Run: `cargo run -p mkfs -- "$TEMP/kb-test.img"`
Expected: stderr `mkfs: wrote … packed 5 KB entries`.

- [ ] **Step 3: Run the existing mkfs lib test** (unchanged round-trip still holds):

Run: `cargo test -p mkfs`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add tools/mkfs/src/main.rs
git commit -m "feat(mkfs): pack the real knowledge-base/entries into the image"
```

---

### Task 5: FS directory enumeration + the KB-loader + patient gating

**Files:**
- Modify: `libs/common/src/fs.rs` (add `dir_entry_at` + `DirEntry::name_str`)
- Modify: `kernel/src/main.rs` (replace `fs_task` with `kb_loader_task`; gate patients; wire caps)
- Test: `libs/common/src/fs.rs` tests

**Interfaces:**
- Produces: `kernel_common::fs::dir_entry_at(dir_bytes: &[u8], i: usize) -> Option<DirEntry>`; `DirEntry::name_str(&self) -> &str`.
- Consumes: `heal::install`, `kb::parse`, `sched::call_message`.

- [ ] **Step 1: Write failing fs tests** (append to `libs/common/src/fs.rs` tests):

```rust
    #[test]
    fn dir_entry_at_and_name_str_enumerate() {
        let mut dir = [0u8; BLOCK_SIZE];
        DirEntry::new("KB-0005", 2, 10).encode(&mut dir[0..DIRENT_SIZE]);
        DirEntry::new("KB-0004", 3, 20).encode(&mut dir[DIRENT_SIZE..2 * DIRENT_SIZE]);
        assert_eq!(dir_entry_at(&dir, 0).unwrap().name_str(), "KB-0005");
        assert_eq!(dir_entry_at(&dir, 1).unwrap().name_str(), "KB-0004");
        assert!(dir_entry_at(&dir, DIRENTS_PER_BLOCK).is_none()); // out of block
    }
```

- [ ] **Step 2: Run to verify failure**

Run: `cargo test -p kernel-common dir_entry_at`
Expected: FAIL (function/method missing).

- [ ] **Step 3: Implement the fs helpers** in `libs/common/src/fs.rs`. Add a method to the `impl DirEntry` block:

```rust
    /// The entry's name as a `&str` (NUL padding trimmed).
    pub fn name_str(&self) -> &str {
        let end = self.name.iter().position(|&c| c == 0).unwrap_or(NAME_LEN);
        core::str::from_utf8(&self.name[..end]).unwrap_or("")
    }
```

And a free function near `lookup`:

```rust
/// Decode the `i`-th directory entry packed in one directory block.
/// `None` if `i` is past the block.
pub fn dir_entry_at(dir_bytes: &[u8], i: usize) -> Option<DirEntry> {
    let off = i * DIRENT_SIZE;
    if i >= DIRENTS_PER_BLOCK || off + DIRENT_SIZE > dir_bytes.len() {
        return None;
    }
    Some(DirEntry::decode(&dir_bytes[off..off + DIRENT_SIZE]))
}
```

- [ ] **Step 4: Run to verify pass**

Run: `cargo test -p kernel-common`
Expected: PASS (all fs + kb tests).

- [ ] **Step 5: Add gate constants** in `kernel/src/main.rs` near the other endpoint consts (after `BLK_IRQ_CAP`):

```rust
    /// Phase 6c — the KB-ready gate. Patients block on this endpoint on their
    /// first run so they cannot crash before the loader builds the on-disk KB
    /// table; the loader releases them once it has.
    const GATE_EP: usize = 5;
    const GATE_CAP: usize = 0;        // a patient's gate-endpoint cap slot
    const GATE_REPLY_SLOT: usize = 1; // a patient's reply cap slot
    const GATE_LOADER_CAP: usize = 1; // the loader's gate-endpoint cap slot
    /// Set true at boot if a virtio-blk device backs the FS (the loader needs
    /// it to read the KB; without it the loader just releases the patients).
    static mut KB_HAS_BLK: bool = false;
```

- [ ] **Step 6: Gate `transient_task`** — in `kernel/src/main.rs`, inside the `if generation == 0 {` block of `transient_task`, BEFORE the deliberate fault asm, add:

```rust
            // Phase 6c gate: wait until the KB loader has built the on-disk
            // table so our crash is diagnosed against the real KB. First run
            // only — a restart (generation > 0) skips this.
            // SAFETY: we hold Endpoint(GATE_EP) at GATE_CAP; recv then reply.
            unsafe {
                let _ = sys_recv(GATE_CAP, GATE_REPLY_SLOT);
                sys_reply(GATE_REPLY_SLOT, 0);
            }
```

- [ ] **Step 7: Gate `flaky_task`** — give it the same first-run gate. Replace the body of `flaky_task` with:

```rust
    extern "C" fn flaky_task() -> ! {
        let generation: usize;
        // SAFETY: read the launch generation the kernel placed in a0.
        unsafe {
            core::arch::asm!("mv {g}, a0", g = out(reg) generation, options(nomem, nostack, preserves_flags));
        }
        if generation == 0 {
            // Phase 6c gate (first run only): block until the KB is loaded.
            // SAFETY: we hold Endpoint(GATE_EP) at GATE_CAP; recv then reply.
            unsafe {
                let _ = sys_recv(GATE_CAP, GATE_REPLY_SLOT);
                sys_reply(GATE_REPLY_SLOT, 0);
            }
        }
        let _v: u8;
        // SAFETY: the deliberate fault. 0x80200000 is the kernel .text base
        // (no U bit); the U-mode load faults before completing and the kernel
        // contains this component. Control never returns here.
        unsafe {
            core::arch::asm!(
                "lb {v}, 0({p})",
                v = out(reg) _v,
                p = in(reg) 0x8020_0000usize,
                options(nostack),
            );
            sys_exit(0) // unreachable: the load faults first
        }
    }
```

- [ ] **Step 8: Grant the gate cap to the patients** — at their spawn sites in the boot sequence, after each `spawn_user`, add the gate endpoint cap at slot `GATE_CAP`. After the `transient` spawn (around line 113):

```rust
        sched::grant_cap(transient, GATE_CAP, Capability::Endpoint(GATE_EP));
```

After the `flaky` spawn (around line 123):

```rust
        sched::grant_cap(flaky, GATE_CAP, Capability::Endpoint(GATE_EP));
```

- [ ] **Step 9: Replace `fs_task` with `kb_loader_task`** in `kernel/src/main.rs`:

```rust
    /// The KB loader (Phase 6c): enumerate the on-disk directory, read and
    /// parse each `knowledge-base/entries/*.md`, and install the tokened ones
    /// into the self-healer's runtime table — so a later contained crash is
    /// diagnosed against the real, on-disk knowledge base. Then release the
    /// gated patients and idle.
    extern "C" fn kb_loader_task() -> ! {
        use kernel_common::{fs, kb};
        let mut scanned = 0usize;
        let mut loaded = 0usize;
        // SAFETY: single hart; KB_HAS_BLK set once at boot.
        let has_blk = unsafe { core::ptr::read(core::ptr::addr_of!(KB_HAS_BLK)) };
        if has_blk {
            'load: {
                let sb = match fs_read_block(0).and_then(fs::Superblock::decode) {
                    Some(sb) => sb,
                    None => {
                        println!("kb: bad superblock; KB not loaded");
                        break 'load;
                    }
                };
                let mut dir = [0u8; fs::BLOCK_SIZE];
                match fs_read_block(sb.dir_block) {
                    Some(b) => dir.copy_from_slice(b),
                    None => {
                        println!("kb: directory read failed; KB not loaded");
                        break 'load;
                    }
                }
                for i in 0..sb.dir_entries as usize {
                    let ent = match fs::dir_entry_at(&dir, i) {
                        Some(e) => e,
                        None => break,
                    };
                    scanned += 1;
                    // `ent` (and its name) live on our stack; fs_read_file only
                    // touches the DMA page and FS_FILEBUF.
                    let bytes = match fs_read_file(ent.name_str()) {
                        Some(b) => b,
                        None => continue,
                    };
                    if let Some(rec) = kb::parse(bytes) {
                        if rec.match_cause.is_some()
                            && heal::install(rec.id, rec.title, rec.playbook, rec.match_cause)
                        {
                            loaded += 1;
                        }
                    }
                }
            }
        } else {
            println!("kb: no disk; KB not loaded");
        }
        println!(
            "heal: loaded {} KB entr{} from disk (scanned {})",
            loaded,
            if loaded == 1 { "y" } else { "ies" },
            scanned
        );
        // Release the two gated patients now that the table is built.
        sched::call_message(GATE_LOADER_CAP, 0);
        sched::call_message(GATE_LOADER_CAP, 0);
        loop {
            sched::yield_now();
            // SAFETY: wait for the next interrupt between yields.
            unsafe { core::arch::asm!("wfi") };
        }
    }
```

- [ ] **Step 10: Wire the loader in boot** — in the `if let Some(blk) = blk_base` block, set the flag and rename the spawned task. Replace the fs spawn (lines ~221-224) with the blk-present setup, and spawn the loader UNCONDITIONALLY after the block. Concretely, inside the `if let Some(blk)` block, after `BLK_DMA_PA = dma_pa;`, add:

```rust
            // SAFETY: set once at boot before the loader runs; single hart.
            unsafe { KB_HAS_BLK = true; }
```

Replace the existing fs spawn lines:

```rust
            // The kernel filesystem client, holding the service endpoint cap.
            let fs = sched::spawn("fs", fs_task,
                core::ptr::addr_of!(KS_FS) as usize + TASK_STACK);
            sched::grant_cap(fs, BLK_EP_CAP, Capability::Endpoint(BLK_EP));
```

with the loader's BLK cap grant (the spawn moves out of the block):

```rust
            sched::grant_cap(kb_loader, BLK_EP_CAP, Capability::Endpoint(BLK_EP));
```

Then, AFTER the whole `if let Some(blk) … else …` block (and before the `idle` spawn), add the unconditional loader spawn + gate cap, declared so the in-block grant can reference it. To keep the borrow order simple, spawn the loader BEFORE the `if let Some(blk)` block:

```rust
        // Phase 6c — the KB loader. Spawned unconditionally so it always
        // releases the gated patients; it reads the KB only if a disk backs
        // the FS (KB_HAS_BLK, set in the blk block below).
        let kb_loader = sched::spawn("kb", kb_loader_task,
            core::ptr::addr_of!(KS_FS) as usize + TASK_STACK);
        sched::grant_cap(kb_loader, GATE_LOADER_CAP, Capability::Endpoint(GATE_EP));
```

(Reuses the `KS_FS` kernel stack the old `fs` task used.)

- [ ] **Step 11: Build the kernel**

Run: `cargo build -p kernel --target riscv64gc-unknown-none-elf`
Expected: builds cleanly. Fix any unused-import/symbol warnings (the old `fs_task` is gone).

- [ ] **Step 12: Commit**

```bash
git add libs/common/src/fs.rs kernel/src/main.rs
git commit -m "feat(fs,heal): boot KB loader reads + installs the on-disk knowledge base, gating patients"
```

---

### Task 6: Smoke test asserts diagnosis from disk + run QEMU

**Files:**
- Modify: `tools/test-qemu.ps1`

- [ ] **Step 1: Update assertions** in `tools/test-qemu.ps1` `$mustMatch`. Remove the two lines:

```
    "fs: read 'KB-0005'",
    "FS-6B-TAIL-OK",
```

and replace with proof the real KB drove the diagnosis:

```
    "heal: loaded \d+ KB entr",
    "heal: diagnosed KB-0005 \(User-space component terminated by a fatal fault\)",
    "playbook: Restart the component, up to a bounded number of retries",
```

(The playbook string is the first step of `KB-0005.md`, different from the old compiled stub — its presence proves the diagnosis text came from disk.)

- [ ] **Step 2: Update the PASS banner** (line ~142) to mention 6c — append to the existing message before the closing quote:

```
 and Phase 6c the living knowledge base: the self-healer reads knowledge-base/entries/*.md off the disk at boot, parses each entry, and diagnoses the contained crash against the on-disk KB-0005 (selected by its match-cause token) — the organism is no longer hardcoded.
```

- [ ] **Step 3: Run the full smoke test**

Run: `pwsh -File tools/test-qemu.ps1`
Expected: `BOOT TEST PASS …`. If it fails, read the missing patterns it prints and the serial log; the most likely culprits are a gate deadlock (a patient never released) or a buffer truncation in `KnownIssue` (widen the cap).

- [ ] **Step 4: Commit**

```bash
git add tools/test-qemu.ps1
git commit -m "test: smoke asserts the crash is diagnosed against the on-disk KB (Phase 6c)"
```

---

### Task 7: Documentation

**Files:**
- Create: `docs/learning/0024-living-knowledge-base.md`
- Modify: `docs/roadmap/roadmap.md` (mark 6c done)
- Modify: `docs/glossary.md`

- [ ] **Step 1: Write the learning note** `docs/learning/0024-living-knowledge-base.md` — keep it short (summary, not tutorial, per the project's learning-notes convention): what 6c did (KB read+parsed from disk, data-driven diagnosis), the one constraint that shaped it (no I/O in the crash path ⇒ load at boot, pure lookup stays), the kernel-vs-disk line (raw-trap decoding in code, knowledge on disk), and the patient-gating trick. Link the spec and KB-0005.

- [ ] **Step 2: Mark 6c done** in `docs/roadmap/roadmap.md` — change the `### Phase 6c — The living knowledge base` heading to `*(done — 2026-06-25)*` and tighten the bullets to past tense, noting write-back deferred.

- [ ] **Step 3: Add glossary entries** to `docs/glossary.md` — `match-cause` (the KB token), `KB loader` (the boot task), and a cross-reference under the existing self-healing/knowledge-base terms.

- [ ] **Step 4: Verify references**

Run: `pwsh -File tools/check-references.ps1`
Expected: no problems.

- [ ] **Step 5: Commit**

```bash
git add docs/learning/0024-living-knowledge-base.md docs/roadmap/roadmap.md docs/glossary.md
git commit -m "docs: Phase 6c living knowledge base - learning note, roadmap, glossary"
```

---

## Self-Review

- **Spec coverage:** §3 schema → Task 1; §4.1 parser → Task 2; §4.2 heal table/diagnose → Task 3; §4.3 mkfs → Task 4; §4.4 loader + §5 gating + fs enum → Task 5; §6 testing → Tasks 2/3/4/5 (host) + Task 6 (smoke); §7 out-of-scope respected (no write-back); docs → Task 7. All covered.
- **Types consistent:** `KbRecord` fields (`id/title/playbook/match_cause`) used identically in Tasks 2 & 5; `heal::install(id,title,playbook,match_cause: Option<&str>)` defined in Task 3, called in Task 5; accessor methods `id()/title()/playbook()` defined in Task 3 and used in the Task 3 `sched.rs` edit; `dir_entry_at`/`name_str` defined and used in Task 5.
- **Ordering/gating:** `GATE_EP=5` free (0–4 taken); loader spawned unconditionally and releases twice; patients gate first-run only; restart path verified to skip the gate. No deadlock when blk absent (loader still releases).
- **No placeholders:** every code step shows complete code.
