# ADR 0001: Implementation language — Rust

- **Status:** Accepted
- **Date:** 2026-06-06

## Context

Security is the project's top priority. Across mainstream operating systems (Linux, Windows, macOS), roughly **70% of serious vulnerabilities are memory-safety bugs** — use-after-free, buffer overflows, data races — which are endemic to C and C++. Choosing the implementation language is therefore a security decision, not a style preference.

## Decision

Write the kernel in **Rust**.

## Consequences

- **Enables:** the compiler eliminates whole classes of memory-safety vulnerabilities before code runs; a strong type system; modern tooling (Cargo, built-in tests); real momentum in systems programming (the Linux kernel now accepts Rust; Redox is a full Rust OS).
- **Costs:** a steeper learning curve, especially for someone new to systems; a smaller (though fast-growing) bare-metal ecosystem than C; some `unsafe` code is unavoidable at the hardware boundary.
- **Mitigation:** keep `unsafe` blocks small, isolated, and reviewed, and document why each is sound.
