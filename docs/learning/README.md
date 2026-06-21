# Learning Notes

This folder holds the author's learning notes as systems-programming concepts click into place. It is a deliberate part of the project, not an afterthought — understanding *why* something works is a deliverable ([principle #6](../vision/principles.md)).

## How to use it

- Notes are numbered: `0001-...`, `0002-...`.
- Capture **what confused you and what made it click** — that's the most valuable thing to your future self and to anyone learning from this project.
- Cross-link to the [glossary](../glossary.md) and [ADRs](../decisions/) where useful.

## Notes

- [0001 — Development environment](0001-dev-environment.md)
- [0002 — Boot and "hello world" (Phase 1)](0002-boot-and-hello-world.md)
- [0003 — Traps and the timer heartbeat (Phase 2a)](0003-traps-and-interrupts.md)
- [0004 — Memory and paging (Phase 2b)](0004-memory-and-paging.md)
- [0005 — Scheduling and context switching (Phase 2c)](0005-scheduling-and-context-switching.md)
- [0006 — User mode and system calls (Phase 3a)](0006-user-mode-and-syscalls.md)
- [0007 — U-mode tasks in the run queue (Phase 3b-i)](0007-user-scheduling.md)
- [0008 — Per-address-space isolation (Phase 3b-ii)](0008-address-space-isolation.md)
- [0009 — Capabilities and synchronous IPC (Phase 3b-iii)](0009-capabilities-and-ipc.md)
- [0010 — A post-quantum primitive: ML-KEM (Phase 3c)](0010-post-quantum-crypto.md)
- [0011 — Discovering hardware from the device tree (Phase 4a)](0011-device-tree.md)
- [0012 — A real UART console (Phase 4b)](0012-uart-console.md)
- [0013 — The first user-space component: an RTC driver (ADR 0007)](0013-first-user-space-component.md)
- [0014 — Self-healing, step one: diagnosis (Phase 5a)](0014-self-healing-diagnosis.md)
- [0015 — Self-healing, step two: the caged fix (Phase 5b)](0015-self-healing-the-caged-fix.md)
