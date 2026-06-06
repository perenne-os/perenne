# Security Model

Security is the project's first priority ([north star](../vision/north-star.md)). This document describes the model; specific decisions are recorded in the [ADRs](../decisions/).

## 1. Capability-based access control

A **capability** is an unforgeable token that both *names* a resource and *grants permission* to use it. In our model a component can do **only** what its capabilities allow — there is **no ambient authority** (no "I'm allowed because I'm root"). If a service doesn't hold a capability for a device, that device may as well not exist for it.

Why this matters for a newcomer: in traditional systems, a program often runs with broad powers it never needed, and an attacker who hijacks it inherits those powers. With capabilities, each component is handed the *minimum* set of keys for its job, so a compromise stays small.

## 2. Minimal Trusted Computing Base (TCB)

The privileged core is kept as small as we can make it — only memory management, scheduling, IPC, and the capability system. Everything else runs unprivileged in user space. A small TCB means:

- Fewer lines of code that *must* be correct.
- A realistic path toward **formal verification** later (mathematically proving properties of the core), the way the seL4 microkernel did.

## 3. Post-quantum cryptography baseline

Per [ADR 0004](../decisions/0004-post-quantum-crypto.md), our cryptographic foundation targets **post-quantum** algorithms (e.g. NIST's ML-KEM and ML-DSA). The threat is not that our OS needs a quantum computer — it is that a *future* quantum computer could break today's encryption. Post-quantum algorithms are ordinary code running on ordinary chips; adopting them early is a concrete way to be genuinely "trusted." We prefer audited libraries over hand-rolled cryptography.

## 4. The safety cage for self-healing

The self-healing system ([self-healing.md](self-healing.md)) is powerful, so it is deliberately *caged*. Every automated action it takes must be:

- **Capability-checked** — it can only act where it holds explicit permission.
- **Logged** — every action is recorded for audit.
- **Reversible** — there is always a defined way to undo it.
- **Auditable** — a human (or another component) can review what it did and why.

The guiding fear: *an unchecked self-healing agent would become the system's single biggest vulnerability.* So the OS proposes and applies fixes **inside this cage**, never as an all-powerful actor. Intelligence (including any future AI) never runs in the privileged kernel — only in isolated user space.

## Summary

Small privileged core + capabilities everywhere + post-quantum crypto + a caged, auditable healer. Each layer assumes the others might fail and limits the blast radius when they do.
