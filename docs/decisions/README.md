# Architecture Decision Records (ADRs)

An **ADR** is a short, dated, immutable record of one significant decision: the context that forced it, the choice we made, and the consequences we accepted. We don't edit old ADRs to change history; if a decision is reversed, we add a new ADR that supersedes the old one.

Why a solo, multi-year project relies on these: memory fades, and "why did I do it this way?" is the most expensive question to re-answer years later. ADRs make the *reasoning* durable, not just the code.

## Index

| # | Title | Status |
|---|-------|--------|
| [0001](0001-language-rust.md) | Implementation language: Rust | Accepted |
| [0002](0002-microkernel.md) | Kernel architecture: microkernel | Accepted |
| [0003](0003-first-target-riscv.md) | First target architecture: RISC-V on QEMU | Accepted |
| [0004](0004-post-quantum-crypto.md) | Cryptography baseline: post-quantum | Accepted |
| [0005](0005-self-healing-knowledge-organism.md) | Support model: self-healing knowledge organism | Accepted |
| [0006](0006-project-name-placeholder.md) | Project name: provisional placeholder | Accepted |
| [0007](0007-extensibility-user-space-components.md) | Extensibility: capability-holding user-space components | Accepted |

## Template

```markdown
# ADR NNNN: <Title>

- **Status:** Accepted
- **Date:** YYYY-MM-DD

## Context
<the problem / forces at play>

## Decision
<what we chose, stated plainly>

## Consequences
<trade-offs accepted, what this enables, what it costs>
```
