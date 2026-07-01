# Phase 11 — Counter-driven escalation — Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** When an issue's cross-boot `seen` count crosses a threshold, the organism escalates it — latches an `escalated` flag, persists it in place, and reports "chronic — flag for triage" in the diagnosis, loader, and shell.

**Architecture:** A second fixed-width field (`escalated: 0`) reuses Phase 10's in-place block-update machinery, written in the same block as `seen`. The crash path bumps `seen` and, on crossing `ESCALATE_AT`, latches `escalated`; the KB-writer persists both fields together.

**Tech Stack:** Rust `no_std` kernel, pure host-tested `kernel-common::kb`, QEMU riscv64, PowerShell two-boot harness.

**Spec:** `docs/design/specs/2026-06-27-phase-11-counter-driven-escalation-design.md`

## Global Constraints

- **Commits:** Conventional Commits, NO AI co-author; author Kathir (signing automated).
- **`kernel-common` pure/host-tested:** `cargo test -p kernel-common`. **Arch host tests:** `cargo test -p kernel-arch-riscv64`. **Build:** `./tools/build.ps1`. **Boot test:** `./tools/test-qemu.ps1`.
- Crash path (`note_diagnosis`) is interrupts-off — **no I/O**; the persist is in the KB-writer task (Phase 7/10 discipline).
- Escalation changes the organism's reporting + persisted status only — **restart control flow (Phase 5b) is unchanged**.
- The `escalated:` field must sit in the entry's **first block** (next to `seen`).

---

## File Structure

- `libs/common/src/kb.rs` — `KbRecord.escalated`; parse/serialize; generalize the in-place writer; `set_escalated_in_block`.
- `knowledge-base/entries/KB-0005.md`, `knowledge-base/schema/issue-record.md` — add the `escalated` field + doc.
- `arch/riscv64/src/heal.rs` — `KnownIssue.escalated`; `ESCALATE_AT`; `install` signature; `note_diagnosis` returns the escalation event; `dirty_entry`/`entry` return `escalated`.
- `arch/riscv64/src/sched.rs` — crash path logs the escalation.
- `kernel/src/main.rs` — loader/writer install with `escalated`; persist writes both fields.
- `arch/riscv64/src/shell.rs` — `kb` shows escalation.
- `tools/test-qemu.ps1` — cross-boot escalation assertions.
- `docs/learning/0029-counter-driven-escalation.md`, `docs/learning/README.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md` — docs.

---

## Task 1: `kb` — `escalated` field + generalized in-place writer

**Files:**
- Modify: `libs/common/src/kb.rs` (struct, parse, serialize, refactor + new fn, tests)

**Interfaces:**
- `KbRecord.escalated: bool`
- `serialize` emits `escalated: 0`
- `pub fn set_escalated_in_block(block: &mut [u8], escalated: bool) -> bool`
- `set_seen_in_block` unchanged signature (now delegates to a shared helper)

- [ ] **Step 1: Write the failing tests** (in `kb.rs` tests)

```rust
    #[test]
    fn parse_reads_escalated_default_false() {
        let r = parse(SAMPLE.as_bytes()).expect("parses");
        assert!(!r.escalated, "absent escalated defaults to false");
        let with = SAMPLE.replace("match-cause: page-fault\n", "match-cause: page-fault\nescalated: 1\n");
        assert!(parse(with.as_bytes()).unwrap().escalated);
    }

    #[test]
    fn serialize_emits_a_parseable_escalated() {
        let mut buf = [0u8; 512];
        let n = serialize("KB-0006", "t", "Restart.", "illegal-instruction", &mut buf).unwrap();
        assert!(!parse(&buf[..n]).unwrap().escalated);
    }

    #[test]
    fn set_escalated_overwrites_in_place_and_round_trips() {
        let doc = "---\nid: KB-0005\nmatch-cause: page-fault\nseen: 00000\nescalated: 0\nplaybook:\n  - \"R.\"\n---\ntitle: \"t\"\n";
        // (title placement is irrelevant to the field overwrite; keep it parseable)
        let doc = "---\nid: KB-0005\ntitle: \"t\"\nmatch-cause: page-fault\nseen: 00000\nescalated: 0\nplaybook:\n  - \"R.\"\n---\n";
        let mut block = [0u8; 512];
        block[..doc.len()].copy_from_slice(doc.as_bytes());
        assert!(set_escalated_in_block(&mut block, true));
        assert!(parse(&block).unwrap().escalated);
        assert!(set_escalated_in_block(&mut block, false));
        assert!(!parse(&block).unwrap().escalated);
    }

    #[test]
    fn set_escalated_rejects_absent_or_malformed() {
        let mut absent = [0u8; 32];
        absent[..16].copy_from_slice(b"---\nid: KB-0001\n");
        assert!(!set_escalated_in_block(&mut absent, true), "absent field rejected");
        let mut bad = [0u8; 64];
        let d = b"---\nescalated: x\n---\n";
        bad[..d.len()].copy_from_slice(d);
        assert!(!set_escalated_in_block(&mut bad, true), "non-digit field rejected");
    }
```

- [ ] **Step 2: Run to verify they fail**

Run: `cargo test -p kernel-common escalated`
Expected: FAIL — field / fn not found.

- [ ] **Step 3: Implement**

Add to `KbRecord` (after `seen`):

```rust
    /// Whether the organism has escalated this issue as chronically recurring
    /// (a fixed-width on-disk flag; `false` if absent).
    pub escalated: bool,
```

In `parse`, add a local (near `let mut seen = 0u32;`):

```rust
    let mut escalated = false;
```

Add a match arm (after the `"seen"` arm):

```rust
                "escalated" => escalated = clean(value) == "1",
```

Add to the returned record:

```rust
    Some(KbRecord { id: id?, title: title?, playbook: playbook?, match_cause, seen, escalated })
```

In `serialize`, emit it (after the `seen: 00000` line):

```rust
    put("escalated: 0\n")?;
```

Refactor the in-place writer into a shared helper and two wrappers. Replace the
existing `set_seen_in_block` body with a delegation, and add the helper + the new
wrapper:

```rust
/// Overwrite a fixed-width unsigned field `key: NNN…` in `block` with `value`
/// (zero-padded to `width`). Returns `false` if the field is absent or its value
/// is not exactly `width` ASCII digits — the in-place guard. Pure. Fixed width
/// is what makes the update a same-length overwrite (no shifting / rewrite).
fn set_uint_field(block: &mut [u8], key: &[u8], value: u32, width: usize) -> bool {
    let Some(pos) = block.windows(key.len()).position(|w| w == key) else {
        return false;
    };
    let start = pos + key.len();
    let end = start + width;
    if end > block.len() || !block[start..end].iter().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let max = 10u32.saturating_pow(width as u32).saturating_sub(1);
    let mut n = value.min(max);
    for i in (0..width).rev() {
        block[start + i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    true
}

/// Overwrite the fixed-width `seen: NNNNN` counter in `block` with `count`.
pub fn set_seen_in_block(block: &mut [u8], count: u32) -> bool {
    set_uint_field(block, b"seen: ", count, SEEN_WIDTH)
}

/// Overwrite the fixed-width `escalated: N` flag in `block` (`1` = escalated).
pub fn set_escalated_in_block(block: &mut [u8], escalated: bool) -> bool {
    set_uint_field(block, b"escalated: ", escalated as u32, 1)
}
```

(Delete the old inline body of `set_seen_in_block` — `SEEN_WIDTH` stays.)

- [ ] **Step 4: Run to verify they pass**

Run: `cargo test -p kernel-common` (escalated tests + the existing seen/serialize tests still pass)
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add libs/common/src/kb.rs
git commit -m "feat(kb): escalated flag + generalized fixed-width in-place writer"
```

---

## Task 2: Add `escalated` to the schema + KB-0005

**Files:**
- Modify: `knowledge-base/entries/KB-0005.md`, `knowledge-base/schema/issue-record.md`

- [ ] **Step 1: Add `escalated: 0` to KB-0005** — right after the `seen: 00000` line (first block):

```yaml
seen: 00000
escalated: 0
```

- [ ] **Step 2: Document it** in `issue-record.md` — add an `escalated` row noting it is a **fixed-width 1-digit** latched flag the runtime self-healer sets in place (to `1`) once `seen` crosses the escalation threshold (Phase 11); it must sit in the entry's first block.

- [ ] **Step 3: Verify the host parse**

Run: `cargo test -p kernel-common parses_the_real_kb_0005`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add knowledge-base/entries/KB-0005.md knowledge-base/schema/issue-record.md
git commit -m "feat(kb): KB-0005 + schema gain the escalated flag"
```

---

## Task 3: `heal` — threshold, escalation, accessors

**Files:**
- Modify: `arch/riscv64/src/heal.rs`
- Test: `arch/riscv64/src/heal.rs` tests.

**Interfaces:**
- `KnownIssue.escalated: bool`; `pub const ESCALATE_AT: u32 = 6;`
- `install(id, title, playbook, match_cause, seen, escalated, start_block) -> bool`
- `note_diagnosis(issue: &KnownIssue) -> Option<u32>` — `Some(seen)` iff this diagnosis *just* escalated the matched entry.
- `dirty_entry() -> Option<(&'static str, u32, u32, bool)>` — `(id, start_block, seen, escalated)`
- `entry(i) -> Option<(&'static str, &'static str, u32, bool)>` — `(id, title, seen, escalated)`

- [ ] **Step 1: Add the field, threshold, and accessor**

Add `pub const ESCALATE_AT: u32 = 6;` near `MAX_ISSUES`. Add to `KnownIssue`:

```rust
    /// Whether the organism has escalated this issue as chronically recurring
    /// (latched once `seen` crosses `ESCALATE_AT`; persisted on disk).
    escalated: bool,
```

Update `new` (add `escalated` param, before `start_block`, set the field) and add
an accessor:

```rust
    fn new(id: &str, title: &str, playbook: &str, match_cause: &str, seen: u32, escalated: bool, start_block: u32) -> Self {
        KnownIssue {
            id: Buf::from_str(id),
            title: Buf::from_str(title),
            playbook: Buf::from_str(playbook),
            match_cause: Buf::from_str(match_cause),
            seen,
            escalated,
            start_block,
            dirty: false,
        }
    }
```

```rust
    pub fn escalated(&self) -> bool {
        self.escalated
    }
```

(Add the `escalated: bool` field to the struct definition between `seen` and
`start_block`.)

- [ ] **Step 2: Update `install`**

```rust
pub fn install(id: &str, title: &str, playbook: &str, match_cause: Option<&str>, seen: u32, escalated: bool, start_block: u32) -> bool {
    // SAFETY: single hart; called only from the boot KB loader / KB-writer.
    unsafe {
        let count = core::ptr::read(core::ptr::addr_of!(KB_COUNT));
        if count >= MAX_ISSUES {
            return false;
        }
        let issue = KnownIssue::new(id, title, playbook, match_cause.unwrap_or(""), seen, escalated, start_block);
        let table = &mut *core::ptr::addr_of_mut!(KB_TABLE);
        table[count] = Some(issue);
        core::ptr::write(core::ptr::addr_of_mut!(KB_COUNT), count + 1);
        true
    }
}
```

- [ ] **Step 3: Update `note_diagnosis` to escalate and report it**

```rust
/// Record the most recent diagnosis, increment the matched entry's "seen N
/// times" counter, mark it dirty, and — if the counter just crossed
/// `ESCALATE_AT` — latch its `escalated` flag. Returns `Some(seen)` iff this
/// diagnosis *just* escalated the entry (for the crash path to log the one-time
/// event). Crash path: interrupts off — no I/O; the disk write is deferred.
pub fn note_diagnosis(issue: &KnownIssue) -> Option<u32> {
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
                if slot.seen >= ESCALATE_AT && !slot.escalated {
                    slot.escalated = true;
                    return Some(slot.seen);
                }
                break;
            }
        }
    }
    None
}
```

- [ ] **Step 4: Update `dirty_entry` and `entry`** to carry `escalated`

```rust
pub fn dirty_entry() -> Option<(&'static str, u32, u32, bool)> {
    // SAFETY: single hart; the KB-writer is the only drainer.
    unsafe {
        let table = &mut *core::ptr::addr_of_mut!(KB_TABLE);
        for slot in table.iter_mut().flatten() {
            if slot.dirty {
                slot.dirty = false;
                if slot.start_block != 0 {
                    return Some((slot.id(), slot.start_block, slot.seen, slot.escalated));
                }
            }
        }
        None
    }
}
```

```rust
pub fn entry(i: usize) -> Option<(&'static str, &'static str, u32, bool)> {
    // SAFETY: single hart; the table is boot-populated then read-only here.
    let table = unsafe { &*core::ptr::addr_of!(KB_TABLE) };
    table
        .get(i)
        .and_then(|slot| slot.as_ref())
        .map(|issue| (issue.id(), issue.title(), issue.seen(), issue.escalated()))
}
```

- [ ] **Step 5: Fix the heal tests** (the `table()` helper's `KnownIssue::new`, and `installed_table_lists_entries_and_max_id`'s `install`/`entry`), and add an escalation test

Update `KnownIssue::new(...)` calls in `table()` to insert the `escalated` arg
(`false`) before `start_block`:

```rust
        t[0] = Some(KnownIssue::new("KB-0099", "decoy", "do nothing", "", 0, false, 0));
        t[1] = Some(KnownIssue::new("KB-0005", "fatal fault", "Restart the component", "page-fault", 0, false, 0));
```

Update the `install`/`entry` assertions:

```rust
        assert!(install("KB-0005", "fatal fault", "p", Some("page-fault"), 0, false, 2));
        assert!(install("KB-0003", "decoy", "p", Some("x"), 0, false, 6));
        assert_eq!(max_kb_number(), 5);
        assert_eq!(entry(0), Some(("KB-0005", "fatal fault", 0, false)));
        assert_eq!(entry(1), Some(("KB-0003", "decoy", 0, false)));
        assert!(entry(2).is_none());
```

Add a focused escalation test (a fresh entry diagnosed up to the threshold). Note:
`note_diagnosis` mutates the global table, so this must be the entry installed by
the single global-state test — extend `installed_table_lists_entries_and_max_id`
to also exercise escalation on `KB-0005` (installed there at start_block 2):

```rust
        // KB-0005 escalates once seen crosses ESCALATE_AT.
        let issue = diagnose(crate::trap::Cause::LoadPageFault).expect("matches KB-0005");
        let mut just = None;
        for _ in 0..ESCALATE_AT {
            just = note_diagnosis(issue);
        }
        assert_eq!(just, Some(ESCALATE_AT), "the diagnosis crossing the threshold reports it");
        assert!(entry(0).unwrap().3, "KB-0005 is now escalated");
        assert!(note_diagnosis(issue).is_none(), "already escalated -> not reported again");
```

(`diagnose(LoadPageFault)` returns the installed KB-0005 by its `page-fault`
token; `issue` borrows the table but `note_diagnosis` copies out first.)

- [ ] **Step 6: Run the heal tests**

Run: `cargo test -p kernel-arch-riscv64 --lib heal`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add arch/riscv64/src/heal.rs
git commit -m "feat(heal): counter-driven escalation — threshold, latch, accessors"
```

---

## Task 4: Crash path logs the escalation

**Files:**
- Modify: `arch/riscv64/src/sched.rs` (`exit_current` diagnosis arm)

- [ ] **Step 1: Capture the escalation event and log it**

In `exit_current`, change the `Some(issue)` arm:

```rust
                    Some(issue) => {
                        crate::println!(
                            "heal: diagnosed {} ({}) -> playbook: {}",
                            issue.id(), issue.title(), issue.playbook()
                        );
                        if let Some(seen) = crate::heal::note_diagnosis(issue) {
                            crate::println!(
                                "heal: {} escalated (seen {seen}) -- recurring; flag for triage",
                                issue.id()
                            );
                        }
                    }
```

- [ ] **Step 2: Build**

Run: `./tools/build.ps1`
Expected: builds (kernel `main.rs` install/dirty_entry/entry callers updated next; if it errors there, that's Task 5).

- [ ] **Step 3: Commit**

```bash
git add arch/riscv64/src/sched.rs
git commit -m "feat(heal): crash path logs a just-escalated issue"
```

---

## Task 5: Loader/writer install + persist both fields

**Files:**
- Modify: `kernel/src/main.rs`

- [ ] **Step 1: Loader passes `escalated`** — in `kb_loader_task`:

```rust
                    if let Some(rec) = kb::parse(bytes) {
                        if rec.match_cause.is_some()
                            && heal::install(rec.id, rec.title, rec.playbook, rec.match_cause, rec.seen, rec.escalated, ent.start_block)
                        {
                            loaded += 1;
                        }
                    }
```

- [ ] **Step 2: Writer installs KB-0006 with `escalated=false`** — in `kb_writer_task`:

```rust
                        heal::install(id, title, DEFAULT_PLAYBOOK, Some(token), 0, false, start);
```

- [ ] **Step 3: Persist both fields** — update the dirty-drain pass in `kb_writer_task`:

```rust
            // Phase 10/11 — persist any entry whose seen-counter (and possibly
            // escalation) changed, in place.
            while let Some((id, start_block, seen, escalated)) = heal::dirty_entry() {
                let mut block = [0u8; kernel_common::fs::BLOCK_SIZE];
                match fs_read_block(start_block) {
                    Some(b) => block.copy_from_slice(b),
                    None => continue,
                }
                let ok = kb::set_seen_in_block(&mut block, seen)
                    & kb::set_escalated_in_block(&mut block, escalated);
                if ok && fs_write_block(start_block, &block) {
                    println!("heal: persisted {id} (seen {seen}{})", if escalated { ", escalated" } else { "" });
                }
            }
```

(`&` not `&&` so both fields are written even though we only log on success of
both; both are required to be present in a valid entry.)

- [ ] **Step 4: Build**

Run: `./tools/build.ps1`
Expected: clean build.

- [ ] **Step 5: Commit**

```bash
git add kernel/src/main.rs
git commit -m "feat(heal): install/persist the escalated flag with seen, in place"
```

---

## Task 6: Shell `kb` shows escalation

**Files:**
- Modify: `arch/riscv64/src/shell.rs`

- [ ] **Step 1: Update the `kb` arm** (the `entry` tuple is now 4 elements):

```rust
        "kb" => {
            let mut i = 0;
            while let Some((id, title, seen, escalated)) = crate::heal::entry(i) {
                if escalated {
                    crate::println!("{id} (seen {seen}, escalated)  {title}");
                } else {
                    crate::println!("{id} (seen {seen})  {title}");
                }
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
git commit -m "feat(shell): kb command shows escalated entries"
```

---

## Task 7: Cross-boot escalation assertions

**Files:**
- Modify: `tools/test-qemu.ps1`

**Interfaces:** boot 2 markers `heal: KB-0005 escalated (seen 6) -- recurring; flag for triage` and the shell `kb` escalated line.

- [ ] **Step 1: Add the boot-2 escalation assertion** to `$mustMatch2`:

```powershell
    "heal: KB-0005 escalated \(seen 6\) -- recurring; flag for triage",
```

- [ ] **Step 2: Update the boot-1 shell `kb` assertion** so it tolerates the not-yet-escalated form (boot 1 ends at seen 4, not escalated). The boot-1 line stays:

```powershell
    "KB-0005 \(seen \d+\)  User-space component terminated by a fatal fault",
```

(This matches `KB-0005 (seen 4)  …` in boot 1; it does **not** require
`escalated`, which is correct — boot 1 must not escalate.)

- [ ] **Step 3: Add a boot-2 shell escalated assertion** to `$mustMatch2` (boot 2's self-demo `kb` runs after escalation):

```powershell
    "KB-0005 \(seen \d+, escalated\)  User-space component terminated by a fatal fault",
```

- [ ] **Step 4: Update the PASS banner** — append: `; and Phase 11 counter-driven escalation: once an issue's cross-boot 'seen' count crosses a threshold the organism escalates it (KB-0005 reaches seen 4 on the first boot, then on the second boot of the same image crosses the threshold and is flagged chronic) - the organism's first adaptive behavior, a decision that requires its persistent memory.`

- [ ] **Step 5: Run the full boot test**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS: …; and Phase 11 counter-driven escalation: …`.

Debugging aids:
- If boot 2 lacks the escalation line → confirm `ESCALATE_AT = 6`, that KB-0005 carries `seen=4` from boot 1 (the `persisted KB-0005 (seen 4)` line from Phase 10 still appears in boot 1), and that `note_diagnosis` returns `Some` on the crossing.
- If boot 1 *does* escalate → the threshold is ≤ 4; it must be > 4 so escalation requires cross-boot memory.
- If the boot-2 shell line lacks `escalated` → the self-demo ran before the threshold was crossed; the demo runs once `last_diagnosis().is_some()`, which is early, but `kb` reads the live table — confirm the demo prints after escalation, or rely on the `heal: KB-0005 escalated` line as the primary proof and drop the shell-escalated assertion if timing is racy.

- [ ] **Step 6: Commit**

```bash
git add tools/test-qemu.ps1
git commit -m "test: cross-boot counter-driven escalation (Phase 11)"
```

---

## Task 8: Documentation

**Files:**
- Create: `docs/learning/0029-counter-driven-escalation.md`
- Modify: `docs/learning/README.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md`

- [ ] **Step 1: Learning note** `docs/learning/0029-counter-driven-escalation.md` — short. Cover: what changed (a threshold-driven, latched `escalated` flag persisted in place); the idea worth keeping (the organism's first behavior that depends on *accumulated history* — escalation that **requires** cross-boot memory, set the threshold above one boot's count to prove it); reused machinery (Phase 10's in-place fixed-width writer, generalized); the proof (seen 4 boot 1, escalate in boot 2); what's next (act on escalation: suppress futile restarts / quarantine; per-component tracking; de-escalation). Follow `0028` in style.

- [ ] **Step 2: Index** in `docs/learning/README.md` (`0029` line).

- [ ] **Step 3: Roadmap** — replace `## Phase 11+ — Breadth` with a completed `## Phase 11 — Counter-driven escalation (done — 2026-06-27)` (goal / you-learn / done-when citing note 0029), and re-add a `## Phase 12+ — Breadth` placeholder.

- [ ] **Step 4: Glossary** — add **Escalation (self-healing)** and **Adaptive behavior** near the self-healing terms.

- [ ] **Step 5: Cross-reference check**

Run: `./tools/check-references.ps1`
Expected: passes.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0029-counter-driven-escalation.md docs/learning/README.md docs/roadmap/roadmap.md docs/glossary.md
git commit -m "docs: Phase 11 counter-driven escalation — learning note 0029, roadmap, glossary"
```

---

## Self-Review (completed during planning)

- **Spec coverage:** escalated field + generalized writer → Task 1; schema/data → Task 2; threshold + latch + accessors → Task 3; crash-path log → Task 4; loader/writer/persist → Task 5; shell → Task 6; cross-boot proof → Task 7; docs → Task 8. All spec sections map to a task.
- **Type consistency:** `install(id,title,playbook,match_cause,seen,escalated,start_block)` consistent (Tasks 3, 5); `note_diagnosis -> Option<u32>` consistent (Tasks 3, 4); `dirty_entry -> Option<(&str,u32,u32,bool)>` consistent (Tasks 3, 5); `entry -> Option<(&str,&str,u32,bool)>` consistent (Tasks 3, 6); `set_escalated_in_block(&mut [u8], bool) -> bool` consistent (Tasks 1, 5).
- **Open verification during execution:** confirm KB-0005 still diagnosed exactly 4×/boot (so `seen 4`→escalate-at-6 in boot 2 holds); watch the boot-2 shell `kb`-escalated timing (Task 7 Step 3) — if the self-demo races ahead of the escalation, keep the `heal: KB-0005 escalated` line as the primary assertion.
