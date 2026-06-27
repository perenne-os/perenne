# 0028 — Revisable knowledge: the "seen N times" counter

**One-line:** the organism's memory stops being write-once — the self-healer
increments a per-entry **"seen N times" counter** on each recurring diagnosis and
persists it **in place** on disk, so the count accumulates across reboots. Memory
that *updates*, not just grows.

## What changed
- A fixed-width **`seen: 00000`** field in each KB entry's frontmatter. `kb::parse`
  reads it; `kb::serialize` emits it; a pure `kb::set_seen_in_block` overwrites the
  5 digits in a block (host-tested, round-trips with `parse`).
- `heal::KnownIssue` gained `seen` + the entry's on-disk `start_block` + a `dirty`
  flag. On each diagnosis (`note_diagnosis`, crash path) the matched entry's `seen`
  is bumped and marked dirty.
- The existing **KB-writer task** gained a second job: drain `heal::dirty_entry`,
  read the entry's first block, `set_seen_in_block`, and write it back in place
  via the existing `fs_write_block` — logging `heal: persisted KB-0005 (seen N)`.
- Phase 9's shell `kb` now shows the count: `KB-0005 (seen 8)  …`.

## The idea worth keeping: in-place mutation needs fixed-width fields
Phase 7 could only *append* — a new file at the end of the volume. Updating an
existing record in place is different: if the counter grew `9 → 10` as text, it
would lengthen the line, shift every byte after it, and force rewriting the whole
entry (which would destroy a rich entry's other fields, since `serialize` only
emits the runtime subset). The fix is a **fixed-width field**: `seen` is always 5
digits, so an update overwrites *exactly those bytes* — no shifting, no
re-serialization, no free-block allocator. The general rule: *a value you intend
to mutate in place must have a fixed on-disk width.* The same off-the-crash-path
discipline as Phase 7 applies — the crash path only bumps RAM + marks dirty; the
disk write happens in a task with interrupts on.

## Safety detail
The persister skips `start_block == 0` (block 0 is the superblock — never an
entry), so a mis-located entry can never corrupt the volume header. `KB-0006`
(written at runtime) carries the real start block `fs_append_file` now returns, so
its counter persists too.

## Proof
Per boot, KB-0005 is diagnosed 4 times deterministically (`transient` once,
`flaky` three times across its bounded restarts). **Boot 1** ends with
`heal: persisted KB-0005 (seen 4)`. **Boot 2** of the *same image* (not rebuilt)
loads `seen=4`, the same crashes bring it to `heal: persisted KB-0005 (seen 8)`,
and the shell `kb` shows `KB-0005 (seen 8)`. The counter accumulated across a
reboot.

## What's next
Counter-driven **escalation** (flag a chronically recurring fault once `seen`
crosses a threshold); variable-length / growable records (a free-block allocator,
multi-block directories); and record deletion/compaction.
