//! The self-healing knowledge organism — deterministic core (Phase 5a).
//!
//! When the microkernel contains a crashed component
//! (`sched::exit_current(Killed{cause})`), it consults this module to
//! DIAGNOSE the crash: match the fault against a compiled-in knowledge base
//! and return the known issue + its fix playbook. The match is a pure,
//! explainable table lookup — never a black box — which is why it is safe in
//! the kernel (ADR 0005). 5a only diagnoses; the caged, isolated, user-space
//! healer that *acts* on the playbook is Phase 5b.
//!
//! The compiled records here are the machine-readable subset of the human
//! knowledge base (`knowledge-base/entries/`); a real loader awaits a
//! filesystem.

use crate::trap::Cause;

/// A compiled-in knowledge record — the runtime subset of a
/// `knowledge-base/entries/*.md` issue record.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KnownIssue {
    pub id: &'static str,
    pub title: &'static str,
    pub playbook: &'static str,
}

/// KB-0005: a user-space component terminated by a fatal fault. Mirrors
/// `knowledge-base/entries/KB-0005.md`.
static KB_0005: KnownIssue = KnownIssue {
    id: "KB-0005",
    title: "user-space component terminated by a fatal fault",
    playbook: "restart the component (bounded retries); if it keeps crashing, stop and flag for triage",
};

/// Diagnose a contained crash by matching its `cause` to a known issue.
/// Deterministic and total over `Cause` (returns `None` for non-crash
/// causes). Pure — host-tested, explainable, no allocation.
pub fn diagnose(cause: Cause) -> Option<&'static KnownIssue> {
    match cause {
        Cause::LoadPageFault | Cause::StorePageFault | Cause::InstructionPageFault => Some(&KB_0005),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnoses_a_fatal_fault_as_kb_0005() {
        assert_eq!(diagnose(Cause::LoadPageFault).map(|i| i.id), Some("KB-0005"));
        assert_eq!(diagnose(Cause::StorePageFault).map(|i| i.id), Some("KB-0005"));
        assert_eq!(diagnose(Cause::InstructionPageFault).map(|i| i.id), Some("KB-0005"));
    }

    #[test]
    fn no_diagnosis_for_a_non_crash_cause() {
        assert!(diagnose(Cause::Breakpoint).is_none());
        assert!(diagnose(Cause::SupervisorTimer).is_none());
        assert!(diagnose(Cause::Unknown { interrupt: false, code: 2 }).is_none());
    }
}
