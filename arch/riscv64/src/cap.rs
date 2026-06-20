//! Capabilities: unforgeable per-task authority tokens.
//!
//! A capability is an *index* into the calling task's own capability table
//! (its CSpace, on `task::Task`). A U-mode task holds only indices it was
//! granted; it cannot fabricate a `Capability` or name a kernel object it
//! was never given — that is what makes the token unforgeable. The "check"
//! a syscall performs is simply [`cap_lookup`] returning `Some`.
//!
//! Pure here (host-tested). The tables live on tasks; the IPC rendezvous
//! that consumes capabilities lives in `sched`.

/// Identifies a synchronous IPC endpoint (a rendezvous point). Symbolic:
/// there is no separate kernel object in 3b-iii — an endpoint's "wait
/// queue" is the set of tasks blocked on this id.
pub type EndpointId = usize;

/// Authority over one kernel object. One type today; more (memory, IRQ,
/// task control) arrive in later phases.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Capability {
    /// Authority to `send` to / `recv` from the endpoint with this id.
    Endpoint(EndpointId),
}

/// Look up capability `idx` in `caps`; if it is an endpoint capability,
/// return its id. `None` if the index is out of range, the slot is empty,
/// or (in future) the capability is the wrong type — i.e. the check failed.
pub fn cap_lookup(caps: &[Option<Capability>], idx: usize) -> Option<EndpointId> {
    match caps.get(idx) {
        Some(Some(Capability::Endpoint(id))) => Some(*id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn looks_up_a_granted_endpoint() {
        let caps = [None, Some(Capability::Endpoint(7)), None];
        assert_eq!(cap_lookup(&caps, 1), Some(7));
    }

    #[test]
    fn rejects_an_empty_slot() {
        let caps: [Option<Capability>; 3] = [None, None, None];
        assert_eq!(cap_lookup(&caps, 1), None);
    }

    #[test]
    fn rejects_an_out_of_range_index() {
        let caps = [Some(Capability::Endpoint(0))];
        assert_eq!(cap_lookup(&caps, 5), None);
    }
}
