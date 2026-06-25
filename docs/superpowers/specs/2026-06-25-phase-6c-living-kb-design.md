# Kernel — Phase 6c Design: The living knowledge base

- **Date:** 2026-06-25
- **Status:** Draft — awaiting review
- **Scope of this document:** Phase 6c only — the self-healer diagnoses a
  contained crash against the **real, runtime** knowledge base read from disk,
  instead of the compiled-in `KB_0005` stub in `heal.rs`. Read-only: the
  filesystem stays read-only; recording a newly-seen issue back to disk (the
  roadmap's stretch) is deferred to a later phase. Fully QEMU-testable.

---

## 0. Where 6c sits

Phase 6 makes the self-healing knowledge organism **real** — persistent
storage, then a knowledge base read (and eventually grown) from disk
([ADR 0005](../decisions/0005-self-healing-knowledge-organism.md), the
project's #1 differentiator). 6a gave a user-space virtio-blk driver; 6b a
minimal read-only filesystem (the kernel reads a named file off disk through
the blk server). 6c closes the loop: the diagnosis half of self-healing
(Phase 5a) stops consulting a compiled-in record and instead consults a table
**loaded from the on-disk knowledge base** at boot.

Today `heal::diagnose(cause)` is a pure lookup that maps a trap `Cause`
(load/store/instruction page fault) to a single `static KB_0005`
(`id`/`title`/`playbook`), compiled into the kernel. The matching *and* the
content are hardcoded. 6c moves the **knowledge** — "this class of symptom
means this issue, here is the playbook" — onto disk, where adding an entry no
longer requires recompiling the kernel.

## 1. Goal

The kernel reads the real `knowledge-base/entries/*.md` files off the disk
image at boot, parses each entry's frontmatter, and builds an in-memory KB
table. `heal::diagnose` then matches a contained crash against **that
disk-loaded table**, selecting the entry whose own (on-disk) match token
corresponds to the crash cause. The compiled-in `KB_0005` record is retired.

**Done when** `./tools/test-qemu.ps1` observes a contained crash diagnosed
against a KB entry **loaded and parsed from disk** — the diagnosis prints the
issue id, title, and playbook text that came from `KB-0005.md` on the image
(not a string baked into `heal.rs`) — proving the organism is no longer
hardcoded.

**You learn (kept brief):** closing the self-healing loop with persistence —
how a deterministic in-kernel diagnosis becomes *data-driven* by loading and
parsing the organism's own memory from disk at boot, while keeping the
crash-path lookup pure (no I/O where it must not happen), and where the line
sits between "knowledge" (on disk) and "the kernel's raw-trap decoding" (in
code).

## 2. The shape, and the one constraint that forces it

`diagnose` runs inside `sched::exit_current`, in the crash trap path, with
interrupts off — it **cannot** do disk I/O (a disk read needs IPC to the blk
server, i.e. the scheduler). So the read + parse must happen **earlier, at
boot**, caching a parsed table in memory; `diagnose` stays a pure lookup over
that table. This is exactly what ADR 0005 wants — diagnosis stays the
deterministic, explainable, in-kernel table lookup — only its *data* now comes
from disk. The user-space healer (Phase 5b) is unchanged: it still only
*acts* (the caged restart via capabilities).

```
boot ─► blk server spawned (recv-blocks, ready to serve)
      ─► KB-loader (kernel task): enumerate dir ─► for each entry:
             fs_read_file(name) ─► kb::parse(frontmatter) ─►
             if it has a match token, install into heal::KB_TABLE
      ─► loader RELEASES the gated patients (see §5)
crash ─► exit_current ─► heal::diagnose(cause):
             cause ─► token ("page-fault") ─► scan KB_TABLE ─► play it back
```

## 3. The on-disk schema addition (the match key)

One new, **optional** frontmatter field, with a tiny fixed vocabulary — not a
query language:

```yaml
match-cause: page-fault   # the runtime-matchable token
```

- The only token for now is **`page-fault`**, covering `LoadPageFault |
  StorePageFault | InstructionPageFault` — exactly the set KB-0005 documents.
- Entries without the field (KB-0001..0004, the dev-environment issues) are
  simply non-matchable: loaded if present, but never selected by `diagnose`.
- `knowledge-base/entries/KB-0005.md` gains `match-cause: page-fault`. The
  schema doc (`knowledge-base/schema/issue-record.md`) documents the field and
  its vocabulary.

The vocabulary grows only by adding a token **and** a kernel `Cause→token` arm
together — a deliberate, reviewed change, because decoding a raw hardware trap
into a stable token is inherently the kernel's job. What is data-driven is the
*knowledge*; what stays in code is the *raw-trap decoding*.

## 4. Components

### 4.1 `kb` parser — `libs/common/src/kb.rs` (pure, host-tested)

Sibling to `fs.rs`; `no_std`, no allocation, no I/O. Parses the few scalar
frontmatter fields the runtime needs out of a file's bytes:

```rust
pub struct KbRecord<'a> {
    pub id: &'a str,
    pub title: &'a str,
    pub playbook: &'a str,          // the first playbook step (the action)
    pub match_cause: Option<&'a str>,
}
pub fn parse(bytes: &[u8]) -> Option<KbRecord<'_>>;
```

A minimal line scanner: find the `---` frontmatter block, read `key: value`
lines, trim and strip surrounding quotes, take the first `- ` item under
`playbook:`. Not a general YAML parser — anything it cannot parse yields
`None` (the entry is skipped and logged), never a panic. Host tests cover: a
well-formed entry; an entry without `match-cause`; quoted vs unquoted values;
the real `KB-0005.md` content; and a malformed block.

### 4.2 `heal.rs` — runtime table + data-driven diagnose (riscv runtime + pure core)

`KnownIssue` stops being a `&'static` record and becomes owned, fixed-capacity
storage (no allocator): small inline byte buffers for `id`, `title`,
`playbook`, `match_cause`, each with a length and a `&str` accessor. A
module-private `KB_TABLE` (a fixed-size array + count, single-hart `static
mut` with SAFETY comments, mirroring `FS_CACHED_BLOCK`) holds the installed
entries.

- `pub fn install(rec: &KbRecord) -> bool` — copy a parsed record into the
  next table slot (bounded; returns false if full or oversized).
- `fn cause_token(c: Cause) -> Option<&'static str>` — the kernel's raw-trap →
  token map (`*PageFault → "page-fault"`). Pure, host-tested.
- `fn match_issue<'a>(table: &'a [KnownIssue], cause: Cause) -> Option<&'a
  KnownIssue>` — `cause_token` then scan for an entry whose `match_cause`
  equals the token. Pure, host-tested.
- `pub fn diagnose(cause: Cause) -> Option<&'static KnownIssue>` — the
  riscv wrapper over the global table; what `exit_current` calls. Its
  signature is unchanged, so the crash path is untouched.

The selection is data-driven: the loader installs **every** tokened entry it
finds on disk; `diagnose` picks KB-0005 because *its on-disk token matched*,
not because any code names KB-0005.

### 4.3 `mkfs` — pack the real KB (`tools/mkfs/src/main.rs`)

Stop writing synthetic demo content. Read the real
`knowledge-base/entries/*.md` (located via `CARGO_MANIFEST_DIR`), name each
file by its id, and pack them all (KB-0001..0005 fit in the single directory
block — `DIRENTS_PER_BLOCK = 8`). The image now carries the actual knowledge
base; the loader filters by token.

### 4.4 KB-loader — `kernel/src/main.rs` (replaces `fs_task`)

The existing `fs_task` becomes the loader. It:
1. reads the superblock + directory block, enumerates the `dir_entries`;
2. for each, `fs_read_file(name)` → `kb::parse` → `heal::install` if tokened;
   logs a one-line summary (`heal: loaded N KB entr… from disk`);
3. releases the gated patients (§5), then idles as before.

`fs_read_file` and the one-block cache are reused unchanged. The directory
enumeration adds a tiny `fs::dir_name_at(dir_bytes, i)` helper (or reuses
`DirEntry::decode`) in `libs/common::fs`.

## 5. Ordering: gating the patients until the KB is loaded

Round-robin reaches the `transient`/`flaky` patients (low slots) before the
loader, so without intervention they would crash before the table exists.
Fix: **gate each patient's first-run fault behind a KB-ready rendezvous.**

- New endpoint `GATE_EP` (next free id, `5`). Both patients hold its cap.
- On **first run only** (`a0 == 0`), a patient does `sys_recv(GATE_CAP, …)`
  then `sys_reply(…)` before its deliberate fault. On a **restart** (`a0 >
  0`) it skips the gate — the table is already loaded.
- After installing the table, the loader calls `sched::call_message(GATE_CAP,
  …)` once per patient to release them.

This is deterministic regardless of scheduling: a patient cannot reach its
fault until released, and the loader releases only after the table is built.
The blk-server rendezvous already self-synchronizes (a caller blocks in the
`Call` state until the server `recv`s), so the loader's slot does not matter.
`diagnose` still degrades gracefully — an empty table logs "no known issue …
(recorded for triage)" — so a stray early crash is safe, not a panic.

## 6. Testing

- **Host unit tests** (`cargo test`): `kb::parse` (the cases in §4.1, incl.
  the real `KB-0005.md` bytes); `heal::cause_token` and `heal::match_issue`
  over a hand-built table (right token selects, wrong/absent token → `None`,
  non-crash cause → `None`); `mkfs` round-trip still builds a parseable image.
- **Smoke test** (`./tools/test-qemu.ps1`): replace the synthetic
  `FS-6B-TAIL-OK` assertion with assertions proving the **real** KB was read
  and is the diagnosis source — e.g. `heal: loaded \d+ KB entr` and a
  `diagnosed KB-0005` line that includes playbook text from `KB-0005.md`
  ("Restart the component…"). Keep the existing 5a/5b lines (`killed by
  LoadPageFault`, `restarted 'transient'`, `giving up on 'flaky'`).

## 7. Out of scope (explicitly)

- **Write-back** (record a new issue to disk): needs a writable FS layer
  (extent allocation, directory append, superblock update). Its own later
  phase.
- **A richer match vocabulary / multiple runtime fault classes**: only
  `page-fault` exists today; more tokens arrive with the crashes that need
  them.
- **Parsing the full YAML** (symptoms, diagnosis prose, references): the
  runtime needs only id/title/playbook/match-cause; the rest stays
  human-only.

## 8. Risks

- **Buffer sizes:** a KB file must fit `FS_FILEBUF_LEN` (4096; KB-0005.md is
  ~1.5 KB) and its fields must fit the fixed `KnownIssue` buffers — the parser
  truncates/round-trips within bounds, never overflows.
- **Directory growth:** packing all entries must stay within the single
  directory block (≤ 8 files); asserted by `mkfs`.
- **Parse fragility:** the scanner tolerates only the known frontmatter shape;
  unpar+seable entries are skipped + logged, never fatal.
