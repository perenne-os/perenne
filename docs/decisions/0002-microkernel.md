# ADR 0002: Kernel architecture — microkernel

- **Status:** Accepted
- **Date:** 2026-06-06

## Context

The architecture style sets the security ceiling. The amount of code running with full privilege (the Trusted Computing Base) is the attack surface. The main options are *monolithic* (drivers, filesystems, networking all run privileged — e.g. Linux), *microkernel* (only a tiny core is privileged; everything else runs isolated in user space — e.g. seL4, QNX), and *hybrid* (a pragmatic blend — e.g. macOS, Windows).

## Decision

Build a **capability-based microkernel**, inspired by seL4.

## Consequences

- **Enables:** the smallest possible privileged core (smallest attack surface); drivers and services run as isolated, **restartable** user-space processes — which both contains compromises and is the natural foundation for self-healing ([ADR 0005](0005-self-healing-knowledge-organism.md)); it is the only architecture style with a kernel that has been **formally proven** secure.
- **Costs:** components communicate via message passing (IPC) instead of direct function calls, adding complexity and some performance overhead. We accept this deliberately and learn it gradually.
