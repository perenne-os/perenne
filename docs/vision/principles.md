# Guiding Principles

These are the standing rules of thumb we apply to every decision. They are how the vision survives contact with thousands of small choices over many years.

## 1. Security from first principles

Every design choice is judged by its effect on the **attack surface** — specifically the size of the *Trusted Computing Base* (the code that runs with full privilege). Smallest TCB wins. We prefer designs that make whole classes of bugs impossible over designs that try to catch them.

## 2. Clean boundaries over chasing trends

We stay future-proof by investing in **well-defined interfaces**, so new hardware or technology can slot in — not by adopting today's hottest technology early. Chasing trends creates churn that ages badly; clean boundaries endure. A 2030 AI chip should plug into our hardware-abstraction layer without touching the kernel.

## 3. Start simple, then grow

Each step is small, real, and finishable, and it teaches the concept the next step needs. We would rather ship a working "hello world" than a half-finished grand design. Complexity is added only when a concrete need demands it (YAGNI).

## 4. Everything is documented

A solo, multi-year project survives on written rationale. Significant decisions are recorded as **Architecture Decision Records (ADRs)** so future-us knows *why*, not just *what*. Documentation is not bureaucracy; it is how the project stays coherent.

## 5. The OS should explain itself

Diagnosability and self-knowledge are first-class concerns, beginning in Phase 0 with the knowledge-base seed. An OS that can describe its own state and history is one that can eventually heal itself — and one a human can trust.

## 6. Learning is part of the work

The author is new to systems programming, and that is by design: this is a learning expedition. Concepts are explained as they arise and captured in `docs/learning/` and `docs/glossary.md`. Understanding *why* something works is a deliverable, not a detour.
