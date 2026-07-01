# ADR 0008: Project name — Perenne

- **Status:** Accepted
- **Date:** 2026-06-30
- **Supersedes:** [ADR 0006](0006-project-name-placeholder.md) (provisional placeholder "Kernel")

## Context

[ADR 0006](0006-project-name-placeholder.md) deliberately deferred naming behind
the provisional working title **"Kernel"**, keeping the project rename-safe (the
name centralized in `PROJECT_NAME` and the greppable phrase "Kernel (working
title)") so the real name could be adopted later in a single, low-risk edit.

The project has now reached a coherent milestone — a secure capability microkernel
whose **self-healing knowledge organism** detects, diagnoses, heals, and *learns
from* its own faults across reboots, with post-quantum-keyed encrypted IPC and a
working network stack. It is time to give it a real, durable identity and a public
home.

## Decision

The project is named **Perenne** (pronounced *puh-REN-eh*), from the Latin/botanical
*perennial* — a living thing that dies back, recovers, and **returns each cycle,
renewed**. The name captures the soul of the project: an OS that survives its own
crashes, remembers the fix, and comes back stronger.

- **Project / brand name:** Perenne
- **Tagline:** *"An OS that remembers."* (alt: *"The self-renewing kernel."*)
- **GitHub organization:** `perenne-os`
- **Primary repository:** `perenne`

The crate names (`kernel`, `kernel-common`, `kernel-crypto`, `kernel-hal`,
`kernel-arch-riscv64`) are an **internal** detail and stay as-is for now — *Perenne
is a kernel*, so `kernel` remains an accurate crate/directory name. Re-prefixing the
crates (e.g. `perenne-kernel`) is an optional later polish, not required by the
brand.

## Why Perenne

- **On-thesis.** The defining feature is self-healing and renewal; "perennial" *is*
  that, in one word.
- **Distinctive & ownable.** A rare word, not a common term — the `perenne-os` org
  and `perenne` repo are available, with a clear search space (unlike "engram",
  which already names an unrelated AI OS).
- **Futuristic yet meaningful.** Reads as a coined, modern name while carrying an
  immediately understandable meaning (anyone hears "perennial").

## Consequences

- **Enables:** a real public identity under the `perenne-os` org; a coherent brand
  for the README/showcase capstone.
- **Costs:** none structural — thanks to ADR 0006's rename-safety, adopting the name
  was a single edit to `PROJECT_NAME` + the `Cargo.toml` metadata, plus doc/brand
  references.

## Rename status (per ADR 0006's checklist)

1. ✅ `[workspace.metadata.project]` `name`/`display-name` in the root `Cargo.toml`.
2. ✅ `PROJECT_NAME` in `libs/common/src/lib.rs` → `"Perenne"` (boot greeting now
   reads `… from Perenne …`).
3. ⏸️ Crate names — intentionally kept (see Decision); optional later.
4. ✅ Documentation brand references (README, vision, glossary) updated to
   **Perenne**; historical `docs/design/plans/*` records are left as-is (they
   document what was true at the time).
5. ⏸️ Repository / folder name — the GitHub repo will be `perenne` under
   `perenne-os`; the local working folder rename is cosmetic and optional.
