# North Star

## Vision

A from-scratch, security-first, hardware-agnostic operating-system kernel that can eventually run across consumer devices (PC, laptop, mobile, tablet, IoT) and accommodate future hardware (quantum and AI accelerators) without architectural rewrites. Its defining feature is a **self-healing "knowledge organism"**: instead of depending on a human community for support, the OS diagnoses its own issues, records them with proven fixes, and consults that growing knowledge first.

This is a deliberate, multi-year, solo, open-source effort. **A tiny, verified "hello world" kernel is a legitimate success** — the goal is a trustworthy foundation that grows steadily, not a feature race.

## Priorities (the order that breaks ties)

When goals conflict, earlier ones win:

1. **A trusted, secure product.** Security is non-negotiable and architected in from the start, never bolted on.
2. **The self-healing knowledge organism.** The soul of the project: community-independent, self-diagnosing support.
3. *(Supporting)* **Future / quantum readiness.** Valued, but achieved through clean architecture rather than early investment in hardware that mostly doesn't exist yet.

## Non-goals

Explicitly out of scope — possibly forever. Naming these protects our focus:

- **POSIX / Linux compatibility** or running existing apps unmodified.
- **Feature parity** with mainstream operating systems.
- **Performance optimization** ahead of correctness and security.
- **Running an OS *on* a quantum processor** — a QPU is an accelerator, not a kernel target.
- **AI/ML inside the privileged kernel** — intelligence lives in isolated user space only.
- **Supporting many CPU architectures at once early on** — one clean target first, then ports.

## What success looks like, concretely

- Each milestone is small, real, finishable, and teaches the systems concept the next milestone needs.
- The architecture always has a *place* for the big ambitions (security, device-agnostic, self-healing), even when they are still stubs.
- Anyone reading the documentation can understand what the project is, why each major decision was made, and what comes next.
