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
    /// Authority to `restart` the task in this scheduler slot (Phase 5b).
    Restart(usize),
    /// Authority to call `getrandom` — draw from the kernel entropy pool.
    Randomness,
    /// Authority to `wait_irq` on this IRQ number (a device's interrupt).
    Interrupt(u32),
    /// One-shot authority to reply to the caller in this scheduler slot (minted
    /// by the kernel when a server receives a Call; consumed by `reply`).
    Reply(usize),
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

/// Read the whole `Capability` at `idx` (the value the kernel copies when a
/// component delegates it via `grant`). `None` for an empty slot or an
/// out-of-range index — which is the unforgeability guard: a component can only
/// delegate a capability it actually holds.
pub fn cap_at(caps: &[Option<Capability>], idx: usize) -> Option<Capability> {
    match caps.get(idx) {
        Some(Some(cap)) => Some(*cap),
        _ => None,
    }
}

/// Clear every capability slot in `caps` holding `Endpoint(ep)`, returning the
/// number cleared. This is how the kernel revokes an endpoint from one holder:
/// a cleared slot is `None`, so `cap_lookup` later returns `None` and the
/// holder's IPC fails — no new check on the hot path. Other endpoint ids and
/// other capability types are untouched. Pure.
pub fn revoke_in_caps(caps: &mut [Option<Capability>], ep: EndpointId) -> usize {
    let mut n = 0;
    for slot in caps.iter_mut() {
        if matches!(slot, Some(Capability::Endpoint(id)) if *id == ep) {
            *slot = None;
            n += 1;
        }
    }
    n
}

/// Look up capability `idx` in `caps`; if it is a restart capability, return
/// its target scheduler slot. `None` if the index is out of range, the slot
/// is empty, or the capability is the wrong type (e.g. an endpoint).
pub fn restart_target(caps: &[Option<Capability>], idx: usize) -> Option<usize> {
    match caps.get(idx) {
        Some(Some(Capability::Restart(slot))) => Some(*slot),
        _ => None,
    }
}

/// True iff capability `idx` is a `Randomness` capability (the authority to
/// draw from the kernel entropy pool). `false` for an empty slot, an
/// out-of-range index, or the wrong capability type.
pub fn has_randomness(caps: &[Option<Capability>], idx: usize) -> bool {
    matches!(caps.get(idx), Some(Some(Capability::Randomness)))
}

/// Return the IRQ an `Interrupt` capability at `idx` authorizes waiting on.
/// `None` for an empty slot, an out-of-range index, or the wrong cap type.
pub fn interrupt_irq(caps: &[Option<Capability>], idx: usize) -> Option<u32> {
    match caps.get(idx) {
        Some(Some(Capability::Interrupt(irq))) => Some(*irq),
        _ => None,
    }
}

/// The caller a one-shot `Reply` capability at `idx` answers. `None` for an
/// empty slot, an out-of-range index, or the wrong capability type.
pub fn reply_caller(caps: &[Option<Capability>], idx: usize) -> Option<usize> {
    match caps.get(idx) {
        Some(Some(Capability::Reply(slot))) => Some(*slot),
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

    #[test]
    fn looks_up_a_granted_restart_target() {
        let caps = [None, Some(Capability::Restart(4)), None];
        assert_eq!(restart_target(&caps, 1), Some(4));
    }

    #[test]
    fn restart_target_rejects_wrong_type_empty_and_oob() {
        let caps = [Some(Capability::Endpoint(0)), None];
        assert_eq!(restart_target(&caps, 0), None, "an Endpoint cap is not a Restart cap");
        assert_eq!(restart_target(&caps, 1), None, "empty slot");
        assert_eq!(restart_target(&caps, 9), None, "out of range");
    }

    #[test]
    fn has_randomness_checks_the_slot() {
        let caps = [None, Some(Capability::Randomness), Some(Capability::Endpoint(0))];
        assert!(has_randomness(&caps, 1));
        assert!(!has_randomness(&caps, 2), "an Endpoint cap is not Randomness");
        assert!(!has_randomness(&caps, 0), "empty slot");
        assert!(!has_randomness(&caps, 9), "out of range");
    }

    #[test]
    fn interrupt_irq_returns_the_irq() {
        let caps = [None, Some(Capability::Interrupt(8)), Some(Capability::Randomness)];
        assert_eq!(interrupt_irq(&caps, 1), Some(8));
        assert_eq!(interrupt_irq(&caps, 2), None, "Randomness is not an Interrupt cap");
        assert_eq!(interrupt_irq(&caps, 0), None, "empty slot");
        assert_eq!(interrupt_irq(&caps, 9), None, "out of range");
    }

    #[test]
    fn reply_caller_returns_the_caller_slot() {
        let caps = [None, Some(Capability::Reply(4)), Some(Capability::Randomness)];
        assert_eq!(reply_caller(&caps, 1), Some(4));
        assert_eq!(reply_caller(&caps, 2), None, "Randomness is not a Reply cap");
        assert_eq!(reply_caller(&caps, 0), None, "empty slot");
        assert_eq!(reply_caller(&caps, 9), None, "out of range");
    }

    #[test]
    fn cap_at_reads_the_whole_capability() {
        let caps = [None, Some(Capability::Endpoint(7)), Some(Capability::Randomness)];
        assert_eq!(cap_at(&caps, 1), Some(Capability::Endpoint(7)));
        assert_eq!(cap_at(&caps, 2), Some(Capability::Randomness));
    }

    #[test]
    fn cap_at_rejects_empty_and_out_of_range() {
        let caps = [None, Some(Capability::Endpoint(0))];
        assert_eq!(cap_at(&caps, 0), None, "empty slot");
        assert_eq!(cap_at(&caps, 9), None, "out of range");
    }

    #[test]
    fn revoke_clears_matching_endpoint_caps_and_counts() {
        let mut caps = [
            Some(Capability::Endpoint(7)),
            Some(Capability::Endpoint(3)),
            Some(Capability::Endpoint(7)),
            Some(Capability::Restart(7)),
        ];
        assert_eq!(revoke_in_caps(&mut caps, 7), 2, "two Endpoint(7) caps cleared");
        assert_eq!(caps[0], None);
        assert_eq!(caps[2], None);
        assert_eq!(caps[1], Some(Capability::Endpoint(3)), "a different endpoint id is untouched");
        assert_eq!(caps[3], Some(Capability::Restart(7)), "a non-endpoint cap with the same number is untouched");
    }

    #[test]
    fn revoke_absent_endpoint_clears_nothing() {
        let mut caps = [Some(Capability::Endpoint(1)), None, Some(Capability::Randomness)];
        assert_eq!(revoke_in_caps(&mut caps, 9), 0);
        assert_eq!(caps[0], Some(Capability::Endpoint(1)));
    }

    #[test]
    fn cap_lookup_rejects_a_restart_cap() {
        // The endpoint lookup must not accept a Restart cap as an endpoint.
        let caps = [Some(Capability::Restart(3))];
        assert_eq!(cap_lookup(&caps, 0), None);
    }
}
