# ADR 0004: Cryptography baseline — post-quantum

- **Status:** Accepted
- **Date:** 2026-06-06

## Context

The project aims to be *trusted*. A future large-scale quantum computer could break much of today's public-key cryptography (RSA, classical Diffie-Hellman, ECC). The threat is not that our OS needs to *use* quantum hardware — quantum processors are accelerators, not kernel targets ([hardware-abstraction](../architecture/hardware-abstraction.md)) — but that our encryption must withstand a quantum-equipped attacker. In 2024 NIST finalized standardized post-quantum algorithms.

## Decision

Adopt a **post-quantum cryptography (PQC)** baseline in the security foundation (e.g. **ML-KEM** for key exchange, **ML-DSA** for signatures). Prefer **audited libraries** over hand-rolled cryptography.

## Consequences

- **Enables:** genuine future-readiness against quantum attackers, ahead of most systems; it is ordinary code on ordinary chips, so there is no special hardware dependency.
- **Costs / deferrals:** larger keys/signatures than classical schemes; concrete library selection is deferred to Phase 3, when crypto is actually integrated. The `libs/crypto` crate is a placeholder until then.
