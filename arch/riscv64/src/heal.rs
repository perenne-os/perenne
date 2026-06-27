//! The self-healing knowledge organism — deterministic core (Phase 5a + 6c).
//!
//! When the microkernel contains a crashed component
//! (`sched::exit_current(Killed{cause})`), it consults this module to
//! DIAGNOSE the crash: match the fault against the knowledge base and return
//! the known issue + its fix playbook. The match is a pure, explainable table
//! lookup — never a black box — which is why it is safe in the kernel (ADR
//! 0005). 5a/6c only diagnose; the caged, isolated, user-space healer that
//! *acts* on the playbook is Phase 5b.
//!
//! Phase 6c made the table **data-driven**: instead of a compiled-in record,
//! the boot-time KB loader reads `knowledge-base/entries/*.md` off the disk,
//! parses each (`kernel_common::kb`), and `install`s the tokened ones here.
//! `diagnose` then selects the entry whose own on-disk `match-cause` token
//! corresponds to the crash cause. What stays in code is only the kernel's
//! job: decoding a raw trap into a stable token (`cause_token`).

use crate::trap::Cause;

const ID_CAP: usize = 16;
const TITLE_CAP: usize = 96;
const PLAYBOOK_CAP: usize = 256;
const TOKEN_CAP: usize = 24;
/// Runtime table capacity (one directory block holds at most 8 entries).
pub const MAX_ISSUES: usize = 8;

/// Diagnoses (across reboots, via the persisted `seen` counter) at which the
/// organism escalates an issue as chronically recurring. Above one boot's count
/// (KB-0005 is diagnosed 4×/boot) so escalation *requires* cross-boot memory.
pub const ESCALATE_AT: u32 = 6;

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
    /// How many times this issue has been diagnosed (the on-disk "seen N times"
    /// counter, loaded at boot and incremented in RAM on each diagnosis).
    seen: u32,
    /// Whether the organism has escalated this issue as chronically recurring
    /// (latched once `seen` crosses `ESCALATE_AT`; persisted on disk).
    escalated: bool,
    /// The entry's first block on disk, so the KB-writer can rewrite its `seen`
    /// field in place. `0` = not persistable (block 0 is the superblock).
    start_block: u32,
    /// The counter changed and needs persisting to disk.
    dirty: bool,
}

impl KnownIssue {
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
    pub fn id(&self) -> &str {
        self.id.as_str()
    }
    pub fn title(&self) -> &str {
        self.title.as_str()
    }
    pub fn playbook(&self) -> &str {
        self.playbook.as_str()
    }
    pub fn seen(&self) -> u32 {
        self.seen
    }
    pub fn escalated(&self) -> bool {
        self.escalated
    }
    fn match_cause(&self) -> &str {
        self.match_cause.as_str()
    }
}

/// The runtime knowledge base — populated by `install` at boot (single hart,
/// before any gated patient can crash), then read-only.
static mut KB_TABLE: [Option<KnownIssue>; MAX_ISSUES] = [None; MAX_ISSUES];
static mut KB_COUNT: usize = 0;

/// The most recent contained-crash diagnosis, for the shell's `diag` command.
/// Set from the crash path (single hart).
static mut LAST_DIAGNOSIS: Option<KnownIssue> = None;

/// A single pending novel cause token, latched by the crash path
/// (`note_unmatched`) and drained by the KB-writer task (`take_novel`). One
/// slot is enough: a token is recorded at most once (once installed it matches,
/// so `note_unmatched` never re-latches it).
static mut NOVEL_TOKEN: Option<&'static str> = None;

/// Install a parsed KB record into the runtime table. Returns false if the
/// table is full. Called only by the boot-time loader.
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

/// Number of entries installed (for boot logging).
pub fn loaded_count() -> usize {
    // SAFETY: single hart; read of a boot-populated counter.
    unsafe { core::ptr::read(core::ptr::addr_of!(KB_COUNT)) }
}

/// The `i`-th installed KB entry's `(id, title)`, for the shell's `kb` command.
pub fn entry(i: usize) -> Option<(&'static str, &'static str, u32, bool)> {
    // SAFETY: single hart; the table is boot-populated then read-only here.
    let table = unsafe { &*core::ptr::addr_of!(KB_TABLE) };
    table
        .get(i)
        .and_then(|slot| slot.as_ref())
        .map(|issue| (issue.id(), issue.title(), issue.seen(), issue.escalated()))
}

/// Record the most recent diagnosis, increment the matched entry's "seen N
/// times" counter, mark it dirty, and — if the counter just crossed
/// `ESCALATE_AT` — latch its `escalated` flag. Returns `Some(seen)` iff this
/// diagnosis *just* escalated the entry (for the crash path to log the one-time
/// event). Called from the crash path (interrupts off — no I/O; deferred write).
pub fn note_diagnosis(issue: &KnownIssue) -> Option<u32> {
    // Copy the record out first so we never hold a borrow into the table while
    // we mutate it below (single hart; crash path is not re-entrant).
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

/// Return the on-disk `(id, start_block, seen)` of one entry whose counter
/// changed and clear its dirty flag, so the KB-writer can persist it in place.
/// `None` if none are dirty. Skips `start_block == 0` (the superblock — never an
/// entry). Called only by the KB-writer task.
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

/// The most recent diagnosis as `(id, playbook)`, for the shell's `diag`
/// command. `None` until the organism has diagnosed a contained crash.
pub fn last_diagnosis() -> Option<(&'static str, &'static str)> {
    // SAFETY: single hart; read of a boot/crash-populated cell.
    let last = unsafe { &*core::ptr::addr_of!(LAST_DIAGNOSIS) };
    last.as_ref().map(|issue| (issue.id(), issue.playbook()))
}

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

/// Map a raw trap cause to a stable, knowledge-base-matchable token. This is
/// the kernel's job (it owns trap decoding); the *knowledge* keyed by the
/// token lives on disk. Pure, host-tested.
fn cause_token(cause: Cause) -> Option<&'static str> {
    match cause {
        Cause::LoadPageFault | Cause::StorePageFault | Cause::InstructionPageFault => {
            Some("page-fault")
        }
        Cause::IllegalInstruction => Some("illegal-instruction"),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> [Option<KnownIssue>; MAX_ISSUES] {
        let mut t: [Option<KnownIssue>; MAX_ISSUES] = Default::default();
        t[0] = Some(KnownIssue::new("KB-0099", "decoy", "do nothing", "", 0, false, 0));
        t[1] = Some(KnownIssue::new(
            "KB-0005",
            "fatal fault",
            "Restart the component",
            "page-fault",
            0,
            false,
            0,
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
    fn illegal_instruction_maps_to_its_token() {
        assert_eq!(cause_token(Cause::IllegalInstruction), Some("illegal-instruction"));
    }

    #[test]
    fn installed_table_lists_entries_and_max_id() {
        // install() mutates process-global state; this is the ONE test that
        // touches the global table, so it runs without cross-test interference.
        assert_eq!(max_kb_number(), 0);
        assert!(entry(0).is_none(), "empty table");
        assert!(install("KB-0005", "fatal fault", "p", Some("page-fault"), 0, false, 2));
        assert!(install("KB-0003", "decoy", "p", Some("x"), 0, false, 6));
        assert_eq!(max_kb_number(), 5);
        assert_eq!(entry(0), Some(("KB-0005", "fatal fault", 0, false)));
        assert_eq!(entry(1), Some(("KB-0003", "decoy", 0, false)));
        assert!(entry(2).is_none());

        // KB-0005 escalates once its seen counter crosses ESCALATE_AT. Re-fetch
        // the issue each call so we never hold a borrow across the mutation.
        let mut just = None;
        for _ in 0..ESCALATE_AT {
            let issue = diagnose(crate::trap::Cause::LoadPageFault).expect("matches KB-0005");
            just = note_diagnosis(issue);
        }
        assert_eq!(just, Some(ESCALATE_AT), "the diagnosis crossing the threshold reports it");
        assert_eq!(entry(0), Some(("KB-0005", "fatal fault", ESCALATE_AT, true)));
        let again = diagnose(crate::trap::Cause::LoadPageFault).unwrap();
        assert!(note_diagnosis(again).is_none(), "already escalated -> not reported again");
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
