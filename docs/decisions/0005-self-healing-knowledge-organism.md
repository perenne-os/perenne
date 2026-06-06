# ADR 0005: Support model — self-healing knowledge organism

- **Status:** Accepted
- **Date:** 2026-06-06

## Context

Mainstream operating systems rely on a human community for support: when something breaks, a person searches for someone who has solved it before. The project wants **community-independent** support — an OS that diagnoses, documents, and fixes its own issues. The danger in automating this is that an unchecked autonomous "fixer" (especially an AI one) would become the system's single biggest vulnerability.

## Decision

Build a self-healing **"knowledge organism"**: a structured, growing memory of issues and proven fix *playbooks* that the OS consults first. The core is **deterministic and explainable**; an AI may be added **later** only as an **isolated user-space advisor** that suggests but never silently acts. Every action runs inside a **safety cage** — capability-checked, logged, reversible, and auditable.

## Consequences

- **Enables:** trustworthy, explainable self-support; knowledge that compounds over time; a clean place for future AI that never compromises the kernel.
- **Costs / scope:** Phase 0 only seeds the **schema and store** (`knowledge-base/`); diagnosis and healing logic come in Phase 5.
- **Hard rule:** AI/ML never runs in the privileged kernel, and the deterministic core is never replaced by a black box — AI augments it from inside the cage.
