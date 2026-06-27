# Phase 10 — Revisable KB (seen-N-times counter) — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** The self-healer increments a per-entry "seen N times" counter on each recurring diagnosis and persists it **in place** on disk, so the count accumulates across reboots.

**Architecture:** A fixed-width `seen: 00000` field in each entry's first block makes an update a same-width byte overwrite (no shifting, no free-block allocator). The crash path bumps the in-RAM count and marks the entry dirty; the existing KB-writer task persists dirty entries via the existing `fs_write_block`.

**Tech Stack:** Rust `no_std` kernel, pure host-tested `kernel-common::kb`, QEMU riscv64, PowerShell two-boot harness.

**Spec:** `docs/superpowers/specs/2026-06-27-phase-10-revisable-kb-design.md`

## Global Constraints

- **Commits:** Conventional Commits, NO Claude co-author; author Kathir (signing automated).
- **`kernel-common` is pure/host-tested:** `cargo test -p kernel-common`. **Arch host tests:** `cargo test -p kernel-arch-riscv64`. **Kernel build:** `./tools/build.ps1`. **Boot test:** `./tools/test-qemu.ps1`.
- The crash path (`note_diagnosis`) runs interrupts-off — **no I/O there**; persistence happens in the KB-writer task (interrupts on), like Phase 7.
- **Do not commit `knowledge-base/entries/KB-0006.md`** (still runtime-generated; the checker exempts it).
- The `seen:` field must sit in the entry's **first block** (top of frontmatter) so a single-block in-place write targets it.

---

## File Structure

- `libs/common/src/kb.rs` — `parse` reads `seen`; `serialize` emits `seen: 00000`; new `set_seen_in_block`.
- `knowledge-base/entries/KB-0005.md`, `knowledge-base/schema/issue-record.md` — add the `seen` field + document it.
- `arch/riscv64/src/heal.rs` — `KnownIssue` gains `seen`/`start_block`/`dirty`; `install` signature; `note_diagnosis` bumps+marks; `dirty_entry`; `entry` returns seen.
- `kernel/src/main.rs` — `fs_append_file` returns the start block; loader passes `start_block`+`seen`; writer installs KB-0006 with its start block and persists dirty entries; shell `kb` shows the count.
- `tools/test-qemu.ps1` — assert the cross-boot counter.
- `docs/learning/0028-revisable-kb.md`, `docs/learning/README.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md` — docs.

---

## Task 1: `kb` — parse/serialize `seen` + `set_seen_in_block`

**Files:**
- Modify: `libs/common/src/kb.rs` (struct, parse, serialize, new fn, tests)

**Interfaces:**
- Produces:
  - `KbRecord.seen: u32`
  - `serialize` emits a `seen: 00000` line (signature unchanged)
  - `pub fn set_seen_in_block(block: &mut [u8], count: u32) -> bool`

- [ ] **Step 1: Write the failing tests** (in the `kb.rs` tests module)

```rust
    #[test]
    fn parse_reads_seen_default_zero() {
        let r = parse(SAMPLE.as_bytes()).expect("parses");
        assert_eq!(r.seen, 0, "absent seen defaults to 0");
        let with = SAMPLE.replace("match-cause: page-fault\n", "match-cause: page-fault\nseen: 00042\n");
        assert_eq!(parse(with.as_bytes()).unwrap().seen, 42);
    }

    #[test]
    fn serialize_emits_a_parseable_seen() {
        let mut buf = [0u8; 512];
        let n = serialize("KB-0006", "t", "Restart.", "illegal-instruction", &mut buf).unwrap();
        assert_eq!(parse(&buf[..n]).unwrap().seen, 0);
    }

    #[test]
    fn set_seen_overwrites_in_place_and_round_trips() {
        // a block containing a fixed-width seen field
        let doc = "---\nid: KB-0005\ntitle: \"t\"\nmatch-cause: page-fault\nseen: 00000\nplaybook:\n  - \"Restart.\"\n---\n";
        let mut block = [0u8; 512];
        block[..doc.len()].copy_from_slice(doc.as_bytes());
        assert!(set_seen_in_block(&mut block, 7));
        assert_eq!(parse(&block).unwrap().seen, 7);
        assert!(set_seen_in_block(&mut block, 1234));
        assert_eq!(parse(&block).unwrap().seen, 1234);
    }

    #[test]
    fn set_seen_rejects_absent_or_malformed_field() {
        let mut no_field = [0u8; 64];
        let d = b"---\nid: KB-0001\nseen: bad\n---\n";
        no_field[..d.len()].copy_from_slice(d);
        assert!(!set_seen_in_block(&mut no_field, 3), "non-digit field rejected");
        let mut absent = [0u8; 32];
        absent[..16].copy_from_slice(b"---\nid: KB-0001\n");
        assert!(!set_seen_in_block(&mut absent, 3), "absent field rejected");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p kernel-common seen`
Expected: FAIL — `seen` field / `set_seen_in_block` not found.

- [ ] **Step 3: Implement**

Add `seen` to `KbRecord` (after `match_cause`):

```rust
    /// How many times the organism has diagnosed this issue (a fixed-width
    /// on-disk counter; `0` if the entry declares none).
    pub seen: u32,
```

In `parse`, add a local and a match arm, and the constructor field. After the
existing `let mut match_cause = None;`:

```rust
    let mut seen = 0u32;
```

In the `match key.trim()` block, add:

```rust
                "seen" => seen = clean(value).parse().unwrap_or(0),
```

Change the return to include `seen`:

```rust
    Some(KbRecord { id: id?, title: title?, playbook: playbook?, match_cause, seen })
```

In `serialize`, emit the field (after the `match-cause` line, before `playbook:`):

```rust
    put("seen: 00000\n")?;
```

Add the fixed-width width constant and the in-place updater (after `serialize`):

```rust
/// Width of the fixed `seen: NNNNN` counter field. Fixed width is what makes an
/// in-place update a same-length byte overwrite (no shifting the rest of the
/// entry, no rewrite, no allocator).
pub const SEEN_WIDTH: usize = 5;

/// Overwrite the fixed-width `seen: NNNNN` counter in `block` with `count`
/// (zero-padded to `SEEN_WIDTH`). Returns `false` if the field is absent or its
/// value is not exactly `SEEN_WIDTH` ASCII digits — the in-place guard. Pure.
pub fn set_seen_in_block(block: &mut [u8], count: u32) -> bool {
    const KEY: &[u8] = b"seen: ";
    let Some(pos) = block.windows(KEY.len()).position(|w| w == KEY) else {
        return false;
    };
    let start = pos + KEY.len();
    let end = start + SEEN_WIDTH;
    if end > block.len() || !block[start..end].iter().all(|b| b.is_ascii_digit()) {
        return false;
    }
    // Format count zero-padded to SEEN_WIDTH (saturating at all-nines).
    let mut n = count.min(99_999);
    for i in (0..SEEN_WIDTH).rev() {
        block[start + i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    true
}
```

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p kernel-common seen`
Expected: PASS (4 tests). Also run `cargo test -p kernel-common` — the existing
`serialize_round_trips_through_parse` still passes (it ignores `seen`).

- [ ] **Step 5: Commit**

```bash
git add libs/common/src/kb.rs
git commit -m "feat(kb): fixed-width seen counter — parse, serialize, set_seen_in_block"
```

---

## Task 2: Add the `seen` field to the KB schema + KB-0005

**Files:**
- Modify: `knowledge-base/entries/KB-0005.md`
- Modify: `knowledge-base/schema/issue-record.md`

- [ ] **Step 1: Add `seen: 00000` to KB-0005** — insert it near the **top** of the frontmatter (within the first 512 bytes), e.g. right after the `match-cause: page-fault` line:

```yaml
match-cause: page-fault
seen: 00000
```

- [ ] **Step 2: Document the convention** in `knowledge-base/schema/issue-record.md` — add a `seen` field description noting it is a **fixed-width 5-digit** counter the runtime self-healer increments in place (Phase 10), and must sit in the entry's first block.

- [ ] **Step 3: Verify the host parse still works**

Run: `cargo test -p kernel-common parses_the_real_kb_0005`
Expected: PASS (the real KB-0005 still parses; now also has `seen`).

- [ ] **Step 4: Commit**

```bash
git add knowledge-base/entries/KB-0005.md knowledge-base/schema/issue-record.md
git commit -m "feat(kb): KB-0005 + schema gain the fixed-width seen counter"
```

---

## Task 3: `heal` — counter, dirty flag, persistence accessors

**Files:**
- Modify: `arch/riscv64/src/heal.rs`
- Test: `arch/riscv64/src/heal.rs` tests.

**Interfaces:**
- `KnownIssue` gains `seen: u32`, `start_block: u32`, `dirty: bool`.
- `install(id, title, playbook, match_cause, seen, start_block) -> bool`
- `note_diagnosis(issue: &KnownIssue)` — bumps the matched entry's `seen`, marks it dirty, records last diagnosis.
- `dirty_entry() -> Option<(u32, u32)>` — `(start_block, seen)` of one dirty entry; clears its dirty flag.
- `entry(i) -> Option<(&'static str, &'static str, u32)>` — `(id, title, seen)`.

- [ ] **Step 1: Add the fields** to `KnownIssue` and `new`

```rust
pub struct KnownIssue {
    id: Buf<ID_CAP>,
    title: Buf<TITLE_CAP>,
    playbook: Buf<PLAYBOOK_CAP>,
    match_cause: Buf<TOKEN_CAP>,
    seen: u32,
    start_block: u32,
    dirty: bool,
}
```

```rust
    fn new(id: &str, title: &str, playbook: &str, match_cause: &str, seen: u32, start_block: u32) -> Self {
        KnownIssue {
            id: Buf::from_str(id),
            title: Buf::from_str(title),
            playbook: Buf::from_str(playbook),
            match_cause: Buf::from_str(match_cause),
            seen,
            start_block,
            dirty: false,
        }
    }
```

Add a `seen` accessor near `playbook`:

```rust
    pub fn seen(&self) -> u32 {
        self.seen
    }
```

- [ ] **Step 2: Update `install`**

```rust
pub fn install(id: &str, title: &str, playbook: &str, match_cause: Option<&str>, seen: u32, start_block: u32) -> bool {
    // SAFETY: single hart; called only from the boot KB loader / KB-writer.
    unsafe {
        let count = core::ptr::read(core::ptr::addr_of!(KB_COUNT));
        if count >= MAX_ISSUES {
            return false;
        }
        let issue = KnownIssue::new(id, title, playbook, match_cause.unwrap_or(""), seen, start_block);
        let table = &mut *core::ptr::addr_of_mut!(KB_TABLE);
        table[count] = Some(issue);
        core::ptr::write(core::ptr::addr_of_mut!(KB_COUNT), count + 1);
        true
    }
}
```

- [ ] **Step 3: Update `note_diagnosis`** to bump the matched entry's counter and mark it dirty

```rust
pub fn note_diagnosis(issue: &KnownIssue) {
    // Copy out the id first so we never hold a borrow into the table while we
    // mutate it below (single hart; crash path is not re-entrant).
    let copy = *issue;
    // SAFETY: single hart, interrupts off in the crash path.
    unsafe {
        core::ptr::write(core::ptr::addr_of_mut!(LAST_DIAGNOSIS), Some(copy));
        let id = copy.id();
        let table = &mut *core::ptr::addr_of_mut!(KB_TABLE);
        for slot in table.iter_mut().flatten() {
            if slot.id() == id {
                slot.seen = slot.seen.saturating_add(1);
                slot.dirty = true;
                break;
            }
        }
    }
}
```

- [ ] **Step 4: Add `dirty_entry`** (near `take_novel`)

```rust
/// Return the on-disk `(start_block, seen)` of one entry whose counter changed
/// and clear its dirty flag, so the KB-writer can persist it. `None` if none are
/// dirty. Skips `start_block == 0` (block 0 is the superblock — never an entry).
pub fn dirty_entry() -> Option<(u32, u32)> {
    // SAFETY: single hart; the KB-writer is the only drainer.
    unsafe {
        let table = &mut *core::ptr::addr_of_mut!(KB_TABLE);
        for slot in table.iter_mut().flatten() {
            if slot.dirty {
                slot.dirty = false;
                if slot.start_block != 0 {
                    return Some((slot.start_block, slot.seen));
                }
            }
        }
        None
    }
}
```

- [ ] **Step 5: Update `entry`** to include the count

```rust
pub fn entry(i: usize) -> Option<(&'static str, &'static str, u32)> {
    // SAFETY: single hart; the table is boot-populated then read-only here.
    let table = unsafe { &*core::ptr::addr_of!(KB_TABLE) };
    table
        .get(i)
        .and_then(|slot| slot.as_ref())
        .map(|issue| (issue.id(), issue.title(), issue.seen()))
}
```

- [ ] **Step 6: Fix the heal tests** that call `install`/`entry` for the new signatures

In `installed_table_lists_entries_and_max_id`, update the `install` calls to pass
`seen` and `start_block`, and the `entry` assertions to the 3-tuple:

```rust
        assert!(install("KB-0005", "fatal fault", "p", Some("page-fault"), 0, 2));
        assert!(install("KB-0003", "decoy", "p", Some("x"), 0, 6));
        assert_eq!(max_kb_number(), 5);
        assert_eq!(entry(0), Some(("KB-0005", "fatal fault", 0)));
        assert_eq!(entry(1), Some(("KB-0003", "decoy", 0)));
        assert!(entry(2).is_none());
```

(Also update any other `install(...)` call in the heal tests — search the test
module — to the 6-arg form.)

- [ ] **Step 7: Run the heal tests**

Run: `cargo test -p kernel-arch-riscv64 --lib heal`
Expected: PASS.

- [ ] **Step 8: Commit**

```bash
git add arch/riscv64/src/heal.rs
git commit -m "feat(heal): per-entry seen counter, dirty flag, persistence accessors"
```

---

## Task 4: Loader/writer install with start block + parsed seen

**Files:**
- Modify: `kernel/src/main.rs` (`fs_append_file` return, loader `install`, writer `install`)
- Test: boot test (Task 7).

**Interfaces:**
- `fs_append_file(name, contents) -> Option<u32>` (the new file's start block) instead of `bool`.

- [ ] **Step 1: `fs_append_file` returns the start block** — change its signature and the final write to return `Some(plan.start_block)` on success, `None` on any failure:

Change `fn fs_append_file(name: &str, contents: &[u8]) -> bool {` to `-> Option<u32>`, change each `return false;` to `return None;`, change `if !fs_write_block(...) { return false; }` blocks to `if !fs_write_block(...) { return None; }`, and change the final line:

```rust
        let mut new_sb = [0u8; fs::BLOCK_SIZE];
        plan.new_superblock.encode(&mut new_sb);
        if fs_write_block(0, &new_sb) {
            Some(plan.start_block)
        } else {
            None
        }
```

- [ ] **Step 2: Loader passes `start_block` + parsed `seen`** — in `kb_loader_task`, update the `install` call:

```rust
                    if let Some(rec) = kb::parse(bytes) {
                        if rec.match_cause.is_some()
                            && heal::install(rec.id, rec.title, rec.playbook, rec.match_cause, rec.seen, ent.start_block)
                        {
                            loaded += 1;
                        }
                    }
```

- [ ] **Step 3: Writer installs KB-0006 with its returned start block** — in `kb_writer_task`, update the append + install:

```rust
                let mut doc = [0u8; kernel_common::fs::BLOCK_SIZE];
                if let Some(len) = kb::serialize(id, title, DEFAULT_PLAYBOOK, token, &mut doc) {
                    if let Some(start) = fs_append_file(id, &doc[..len]) {
                        // Install it now too, so a re-crash this boot matches.
                        heal::install(id, title, DEFAULT_PLAYBOOK, Some(token), 0, start);
                        println!("heal: recorded {id} ({token}) to disk");
                    } else {
                        println!("heal: could not record {id} ({token}) to disk");
                    }
                }
```

- [ ] **Step 4: Build**

Run: `./tools/build.ps1`
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat(fs): fs_append_file returns the start block; install with seen+location"
```

---

## Task 5: Persist dirty entries in the KB-writer task

**Files:**
- Modify: `kernel/src/main.rs` (`kb_writer_task`)
- Test: boot test (Task 7).

- [ ] **Step 1: Add the persist pass** to `kb_writer_task`, after the novel-token block (still inside the `loop`, before `sched::yield_now()`):

```rust
            // Phase 10 — persist any entry whose seen-counter changed, in place.
            while let Some((start_block, seen)) = heal::dirty_entry() {
                let mut block = [0u8; kernel_common::fs::BLOCK_SIZE];
                match fs_read_block(start_block) {
                    Some(b) => block.copy_from_slice(b),
                    None => continue,
                }
                if kb::set_seen_in_block(&mut block, seen) && fs_write_block(start_block, &block) {
                    println!("heal: persisted KB-{:04} (seen {seen})", seen_id_hint(start_block));
                }
            }
```

Wait — the log needs the entry's id, not derivable from `start_block` alone. Instead, change `heal::dirty_entry` callers to also know the id. Simpler: have `dirty_entry` return the id too. Update the plan's Task 3 `dirty_entry` to `Option<(&'static str, u32, u32)>` = `(id, start_block, seen)`:

```rust
pub fn dirty_entry() -> Option<(&'static str, u32, u32)> {
    // SAFETY: single hart; the KB-writer is the only drainer.
    unsafe {
        let table = &mut *core::ptr::addr_of_mut!(KB_TABLE);
        for slot in table.iter_mut().flatten() {
            if slot.dirty {
                slot.dirty = false;
                if slot.start_block != 0 {
                    return Some((slot.id(), slot.start_block, slot.seen));
                }
            }
        }
        None
    }
}
```

Then the writer pass is:

```rust
            // Phase 10 — persist any entry whose seen-counter changed, in place.
            while let Some((id, start_block, seen)) = heal::dirty_entry() {
                let mut block = [0u8; kernel_common::fs::BLOCK_SIZE];
                match fs_read_block(start_block) {
                    Some(b) => block.copy_from_slice(b),
                    None => continue,
                }
                if kb::set_seen_in_block(&mut block, seen) && fs_write_block(start_block, &block) {
                    println!("heal: persisted {id} (seen {seen})");
                }
            }
```

(Apply the `dirty_entry` signature `(&'static str, u32, u32)` in Task 3 Step 4 instead of `(u32, u32)`. The id borrows the static table, which lives forever — `&'static` is sound.)

- [ ] **Step 2: Build**

Run: `./tools/build.ps1`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add kernel/src/main.rs arch/riscv64/src/heal.rs
git commit -m "feat(heal): KB-writer persists changed seen counters in place"
```

---

## Task 6: Shell `kb` shows the counter

**Files:**
- Modify: `arch/riscv64/src/shell.rs` (the `kb` dispatch arm)

- [ ] **Step 1: Update the `kb` command** in `dispatch` to print the count (the `entry` tuple is now 3 elements):

```rust
        "kb" => {
            let mut i = 0;
            while let Some((id, title, seen)) = crate::heal::entry(i) {
                crate::println!("{id} (seen {seen})  {title}");
                i += 1;
            }
            if i == 0 {
                crate::println!("(knowledge base empty)");
            }
        }
```

- [ ] **Step 2: Build**

Run: `./tools/build.ps1`
Expected: clean build.

- [ ] **Step 3: Commit**

```bash
git add arch/riscv64/src/shell.rs
git commit -m "feat(shell): kb command shows each entry's seen counter"
```

---

## Task 7: Cross-boot counter assertions in the boot test

**Files:**
- Modify: `tools/test-qemu.ps1`

**Interfaces:** the markers `heal: persisted KB-0005 (seen 4)` (boot 1) and `heal: persisted KB-0005 (seen 8)` (boot 2).

- [ ] **Step 1: Update the shell `kb` assertion** — the boot-1 `$mustMatch1` line that asserts the `kb` output now includes the count. Change:

```powershell
    "KB-0005  User-space component terminated by a fatal fault",
```
to:
```powershell
    "KB-0005 \(seen \d+\)  User-space component terminated by a fatal fault",
```

- [ ] **Step 2: Add the boot-1 persist assertion** to `$mustMatch1`:

```powershell
    "heal: persisted KB-0005 \(seen 4\)",
```

- [ ] **Step 3: Add the boot-2 carry-over assertion** to `$mustMatch2` (the cross-boot proof — boot 2 starts from boot 1's `seen=4` and reaches 8):

```powershell
    "heal: persisted KB-0005 \(seen 8\)",
```

- [ ] **Step 4: Update the PASS banner** — append: `; and Phase 10 revisable knowledge: the self-healer increments a per-entry 'seen N times' counter and persists it in place, so it accumulates across reboots (KB-0005 seen 4 on the first boot, 8 on the second of the same image).`

- [ ] **Step 5: Run the full boot test**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS: …; and Phase 10 revisable knowledge: …`.

Debugging aids:
- If boot 1 lacks `seen 4` → the persister isn't running or the field isn't found: confirm `seen: 00000` is in KB-0005's first block, and that `set_seen_in_block` finds it.
- If boot 2 shows `seen 4` not `8` → the in-place write didn't persist to the image (confirm the drive is shared/not-rebuilt between boots — it is, from Phase 7).
- Timing: the persist adds a few blk writes; if boot 1 runs past 60s, raise the per-boot deadline a little (it is 60s from Phase 7).

- [ ] **Step 6: Commit**

```bash
git add tools/test-qemu.ps1
git commit -m "test: cross-boot seen-counter accumulation (Phase 10)"
```

---

## Task 8: Documentation

**Files:**
- Create: `docs/learning/0028-revisable-kb.md`
- Modify: `docs/learning/README.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md`

- [ ] **Step 1: Learning note** `docs/learning/0028-revisable-kb.md` — short (memory: learning-notes-minimal). Cover: what changed (an in-place per-entry seen counter, persisted across reboots); the idea worth keeping (in-place mutation needs **fixed-width fields** — same width = overwrite, no rewrite/allocator; and the same off-the-crash-path discipline as Phase 7); the proof (KB-0005 seen 4 → 8 across two boots of one image); what's next (variable-length/growable records, counter-driven escalation). Follow `0024`/`0025` in style.

- [ ] **Step 2: Index** it in `docs/learning/README.md` (add the `0028` line).

- [ ] **Step 3: Roadmap** — replace `## Phase 10+ — Breadth` with a completed `## Phase 10 — Revisable knowledge: the seen-N-times counter (done — 2026-06-27)` (goal / you-learn / done-when citing learning note 0028), and re-add a `## Phase 11+ — Breadth` placeholder.

- [ ] **Step 4: Glossary** — add **Revisable record / in-place update** and **Fixed-width field** near the storage/KB terms.

- [ ] **Step 5: Cross-reference check**

Run: `./tools/check-references.ps1`
Expected: passes.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0028-revisable-kb.md docs/learning/README.md docs/roadmap/roadmap.md docs/glossary.md
git commit -m "docs: Phase 10 revisable KB — learning note 0028, roadmap, glossary"
```

---

## Self-Review (completed during planning)

- **Spec coverage:** fixed-width counter + parse/serialize/set_seen → Task 1; schema/data → Task 2; in-RAM counter + dirty + accessors → Task 3; install-with-location + append-returns-start → Task 4; in-place persist → Task 5; shell display → Task 6; cross-boot proof → Task 7; docs → Task 8. All spec sections map to a task.
- **Type consistency:** `set_seen_in_block(&mut [u8], u32) -> bool` consistent (Tasks 1, 5); `install(id, title, playbook, match_cause, seen, start_block)` consistent (Tasks 3, 4); `dirty_entry() -> Option<(&'static str, u32, u32)>` consistent (Task 3 Step 4 as revised in Task 5, and Task 5 caller); `entry(i) -> Option<(&str,&str,u32)>` consistent (Tasks 3, 6); `fs_append_file -> Option<u32>` consistent (Task 4 producer, Task 4 Step 3 consumer).
- **Note:** `dirty_entry` returns `(&'static str, u32, u32)` (id, start_block, seen) — Task 3 Step 4 shows the `(u32, u32)` form first; **use the 3-tuple form from Task 5 Step 1** (it supersedes it, for the persist log's id).
- **Open verification during execution:** confirm KB-0005 is still diagnosed exactly 4×/boot (transient 1 + flaky 3) so `seen 4`/`8` hold; watch boot-1 timing (a few extra blk writes) against the 60s deadline.
