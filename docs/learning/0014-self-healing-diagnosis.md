# 0014 — Self-healing, step one: diagnosis (Phase 5a)

**One-line:** when the kernel contains a crashed component, it now *consults
itself* — matching the crash to a known issue and logging the fix playbook.

## What changed
- New `arch/riscv64/src/heal.rs`: a pure, host-tested rule engine.
  `diagnose(cause)` maps a fatal fault to a compiled-in knowledge record
  (`KB-0005`, mirroring `knowledge-base/entries/KB-0005.md`).
- `exit_current`'s containment path (where a faulting component is already
  terminated) now also calls `heal::diagnose` and logs the diagnosis +
  playbook. A `flaky` component crashes on purpose to exercise it.

## The point (ADR 0005)
This is the first runtime cell of the "knowledge organism": detect → consult
the knowledge base. It is deliberately **diagnosis only**, and deliberately
**deterministic** — a transparent table lookup, not a black box — which is
why it is safe in the kernel. The part that *acts* (a bounded, reversible,
capability-caged restart) is Phase 5b, and it moves into an isolated
user-space healer, because an agent with the power to act is exactly what
must be confined.

## Why diagnosis before action
"Deterministic core first": prove the OS can recognize and explain a problem
before you give anything the authority to change the system in response. The
matching is host-tested and auditable; the action will be caged.

## Proof
`flaky` touches memory it doesn't own → `sched: task 'flaky' killed by
LoadPageFault` (contained) → `heal: diagnosed KB-0005 … -> playbook: restart
…`, while the RTC component and the heartbeat keep running. Next: 5b applies
the (caged) fix.
