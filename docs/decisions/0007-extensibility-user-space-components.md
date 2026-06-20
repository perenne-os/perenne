# ADR 0007: Extensibility model — capability-holding user-space components

- **Status:** Accepted
- **Date:** 2026-06-20

## Context

We want the OS to be a small, maximally-trustworthy **core** whose
functionality — drivers, filesystems, services, and vendor/community
features — can be **extended by others** (developers, vendors, partners,
the community) based on need, rather than us building everything ourselves.
The hard part is allowing *open* extension without (a) growing the Trusted
Computing Base ([principle #1](../vision/principles.md): smallest TCB wins)
or (b) having to *trust* the people who write the extensions.

This is not a new direction — it is the natural realization of the
[microkernel choice (ADR 0002)](0002-microkernel.md), now that the
capabilities and synchronous IPC it depends on actually exist (Phase 3b).
ADR 0002 already noted drivers/services should run as isolated, restartable
user-space processes; this ADR makes the *extensibility* consequence
explicit and commits to it as a defining shape of the project.

## Decision

The kernel is a **minimal trusted core** (the "secure shell"). All
extensible functionality lives as **unprivileged user-space components**
that hold **capabilities** and communicate only through **capability-checked
IPC**. Concretely:

- A driver, service, or feature is a component granted exactly the
  capabilities it needs (e.g. a capability to a device's MMIO/IRQ, or to an
  IPC endpoint) — **least authority**.
- Third parties add components **without modifying, or being trusted by, the
  core.** A component's authority is bounded by its capability set, so a
  buggy or malicious extension cannot exceed what it was granted.
- We seed the ecosystem with a few **first-party example components** (a
  driver/service moved out of the kernel) to demonstrate the pattern and
  invite contribution — community/need-driven growth, not a feature factory.

## Consequences

- **Enables:** a need- and community-driven ecosystem without us building
  everything; open extension that does **not** erode security (extensions
  are caged by capabilities); a steadily **shrinking TCB** as functionality
  moves out of the kernel; and a clean fit with self-healing
  ([ADR 0005](0005-self-healing-knowledge-organism.md)) — the self-healer is
  itself such a caged, restartable component, not privileged kernel code.
- **Costs / deferred:** a real extension ecosystem needs **stable
  interfaces** — IPC protocols, a component/manifest format,
  capability-grant and discovery conventions, an ABI. These are designed
  **incrementally, as concrete components demand them**
  ([principle #3](../vision/principles.md): YAGNI), not up front. And
  "community-driven" presumes a community a solo project will not have for a
  long time; the architecture keeps the door open in the meantime
  ([north-star](../vision/north-star.md): the architecture always has a
  *place* for the big ambition, even as a stub).
- **A distinction kept explicit:** self-healing keeps the OS
  **community-*independent* for support** (it diagnoses and fixes itself
  instead of depending on humans), while this model keeps it **open for
  capability *extension*** (others add functionality). Complementary, not
  contradictory.
