# ADR 0006: Project name — provisional placeholder

- **Status:** Accepted
- **Date:** 2026-06-06

## Context

A good name matters for an open-source project's identity, but deciding it early would block momentum, and a late rename is error-prone if the name is scattered across the codebase.

## Decision

Use the provisional working title **"Kernel"** and keep the project **rename-safe**: the name is referenced from as few places as possible, and documentation refers to it consistently as **"Kernel (working title)"** so every reference is easy to find.

## Consequences

- **Enables:** progress now; a clean, low-risk rename whenever the right name arrives.
- **Costs:** a temporary, generic identity.

## Rename procedure (checklist)

When the project is renamed, update — and only these:

1. `[workspace.metadata.project]` (`name`, `display-name`) in the root `Cargo.toml`.
2. `PROJECT_NAME` in `libs/common/src/lib.rs` (the single source of truth used by code).
3. Crate names (e.g. `kernel`, `kernel-common`, `kernel-crypto`, `kernel-hal`, `kernel-arch-riscv64`) if a renamed prefix is desired, plus the `path` dependency references.
4. Documentation references to **"Kernel (working title)"** (greppable by that exact phrase).
5. The repository / root folder name.

Because docs use the exact phrase "Kernel (working title)" and code centralizes the name in `PROJECT_NAME`, the rename is a reviewed find-and-replace with no missed references.
