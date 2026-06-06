# Phase 0 Foundation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build the complete Phase 0 foundation for the Kernel project — repository skeleton, founding documentation, a compiling Rust workspace, pinned toolchain, and a verified QEMU/RISC-V development environment — with no kernel logic yet.

**Architecture:** A Cargo workspace whose member crates are intentional placeholders that compile on the host today and become the real microkernel, HAL, and libraries in later phases. Documentation is organized into vision, architecture, decisions (ADRs), roadmap, glossary, and learning notes. A `knowledge-base/` tree seeds the future self-healing system. Toolchain reproducibility is pinned via `rust-toolchain.toml`; the RISC-V cross-build configuration is staged but not activated until Phase 1 (so the host build stays green now).

**Tech Stack:** Rust (nightly, for future `build-std`), Cargo workspaces, `riscv64gc-unknown-none-elf` target, QEMU (`qemu-system-riscv64`), Git with SSH commit signing (already configured), Markdown docs. Platform: Windows (native), PowerShell.

---

## Conventions for this plan

- **Commits:** Commit signing via SSH is already working silently (ssh-agent + system ssh-keygen). Commit messages use Conventional Commits and **must not** include a `Co-Authored-By` trailer (user preference). During execution, commits may be run by the executor OR handed to the user to run — confirmed at execution handoff.
- **"Verify" steps** replace TDD test steps where there is no application logic. They are exact commands with expected output. Do not skip them.
- **Install steps** (Task 1) are run by the user in their own PowerShell terminal, because they may require interaction/elevation. All other file-creation steps can be performed by the executor.
- **Paths** are relative to the repo root `D:\Projects\Kernel` unless absolute.
- **Already done (do not redo):** `git init`, `.gitignore`, and the spec doc are committed (commit `9bcd30d`).

---

## File Structure (what gets created)

```
README.md                              Task 6
LICENSE                                Task 6
CONTRIBUTING.md                        Task 12
rust-toolchain.toml                    Task 1
Cargo.toml                             Task 3   (workspace root)
.cargo/config.toml                     Task 4
riscv64gc-unknown-none-elf.json        Task 4   (custom target spec, staged for Phase 1)

kernel/Cargo.toml + src/lib.rs         Task 3
hal/Cargo.toml + src/lib.rs            Task 3
arch/riscv64/Cargo.toml + src/lib.rs   Task 3
libs/common/Cargo.toml + src/lib.rs    Task 3
libs/crypto/Cargo.toml + src/lib.rs    Task 3
services/.gitkeep                      Task 2
tests/.gitkeep                         Task 2

tools/build.ps1                        Task 5
tools/run-qemu.ps1                     Task 5
tools/README.md                        Task 5

docs/vision/north-star.md              Task 7
docs/vision/principles.md              Task 7
docs/architecture/overview.md          Task 8
docs/architecture/security-model.md    Task 8
docs/architecture/hardware-abstraction.md  Task 8
docs/architecture/self-healing.md      Task 8
docs/decisions/README.md               Task 9
docs/decisions/0001-language-rust.md   Task 9
docs/decisions/0002-microkernel.md     Task 9
docs/decisions/0003-first-target-riscv.md  Task 9
docs/decisions/0004-post-quantum-crypto.md Task 9
docs/decisions/0005-self-healing-knowledge-organism.md  Task 9
docs/decisions/0006-project-name-placeholder.md  Task 9
docs/roadmap/roadmap.md                Task 10
docs/glossary.md                       Task 10
docs/learning/README.md                Task 10
docs/learning/0001-dev-environment.md  Task 10

knowledge-base/README.md               Task 11
knowledge-base/schema/issue-record.md  Task 11
knowledge-base/schema/example-0001.md  Task 11
knowledge-base/entries/.gitkeep        Task 11
```

---

## Task 1: Toolchain & environment setup

Installs and pins the build/run toolchain and proves the RISC-V virtual machine boots. **The user runs the install commands in their own PowerShell terminal.** The executor creates `rust-toolchain.toml` and verifies versions afterward.

**Files:**
- Create: `rust-toolchain.toml`

- [ ] **Step 1: User installs Rust (rustup) — if not already present**

User runs in their PowerShell:
```powershell
winget install --id Rustlang.Rustup -e --source winget
```
(If `rustup` already exists, skip. Restart the terminal afterward so PATH updates.)

- [ ] **Step 2: User installs QEMU**

User runs:
```powershell
winget install --id SoftwareFreedomConservancy.QEMU -e --source winget
```
After install, ensure QEMU is on PATH (default install dir is `C:\Program Files\qemu`). If `qemu-system-riscv64 --version` is not found in a new terminal, add `C:\Program Files\qemu` to PATH.

- [ ] **Step 3: Create `rust-toolchain.toml`** (executor)

This pins the toolchain so every machine builds identically. Nightly is chosen because Phase 1 needs `build-std` (building the core library for a custom bare-metal target), which is nightly-only.

```toml
# Pins the Rust toolchain for reproducible builds across machines.
# Nightly is required for `-Z build-std` (building core/alloc for our
# bare-metal RISC-V target) starting in Phase 1.
[toolchain]
channel = "nightly"
components = ["rust-src", "rustfmt", "clippy", "llvm-tools"]
targets = ["riscv64gc-unknown-none-elf"]
profile = "minimal"
```

- [ ] **Step 4: Verify the Rust toolchain installed correctly**

Run (executor, after user confirms install + new terminal):
```powershell
rustc --version; cargo --version; rustup target list --installed
```
Expected: `rustc 1.x.x-nightly`, a cargo version, and `riscv64gc-unknown-none-elf` listed among installed targets. (Running any cargo/rustc command inside the repo auto-installs the toolchain pinned by `rust-toolchain.toml`.)

- [ ] **Step 5: Verify QEMU + RISC-V firmware boots (the "known-good image" check)**

Run:
```powershell
qemu-system-riscv64 --version
qemu-system-riscv64 -machine virt -nographic -bios default
```
Expected: the first command prints a QEMU version. The second boots QEMU's built-in OpenSBI firmware and prints the **OpenSBI banner** (an ASCII box reading "OpenSBI ..."), then idles. This proves the RISC-V virtual machine + firmware work *before our kernel exists*. Exit QEMU with `Ctrl-A` then `X`.

- [ ] **Step 6: Commit**

```powershell
git add rust-toolchain.toml
git commit -m "chore: pin Rust toolchain (nightly + riscv64 target) for reproducible builds"
```

---

## Task 2: Directory skeleton

Creates the directory tree with `.gitkeep` files so empty-but-intentional folders are tracked by git.

**Files:**
- Create: `services/.gitkeep`, `tests/.gitkeep` (other dirs are created implicitly by later tasks' files)

- [ ] **Step 1: Create placeholder-tracking files**

Create `services/.gitkeep` with content:
```
# This directory will hold user-space services (drivers, filesystem, network)
# in the microkernel design. Empty for now (Phase 6+). Tracked so the
# structure exists from day one.
```

Create `tests/.gitkeep` with content:
```
# Integration / system test harnesses live here. Empty for now.
```

- [ ] **Step 2: Verify the directories exist**

Run:
```powershell
Get-ChildItem -Recurse -Filter ".gitkeep" | Select-Object FullName
```
Expected: lists `services\.gitkeep` and `tests\.gitkeep`.

- [ ] **Step 3: Commit**

```powershell
git add services/.gitkeep tests/.gitkeep
git commit -m "chore: scaffold services/ and tests/ directories"
```

---

## Task 3: Rust workspace that compiles on the host

Creates the Cargo workspace and placeholder member crates. These compile on the host **today** (ordinary library crates) and are replaced with real `no_std` code in later phases. Keeping them host-compiling gives an honest green "it builds" signal for Phase 0.

**Files:**
- Create: `Cargo.toml` (workspace root)
- Create: `kernel/Cargo.toml`, `kernel/src/lib.rs`
- Create: `hal/Cargo.toml`, `hal/src/lib.rs`
- Create: `arch/riscv64/Cargo.toml`, `arch/riscv64/src/lib.rs`
- Create: `libs/common/Cargo.toml`, `libs/common/src/lib.rs`
- Create: `libs/crypto/Cargo.toml`, `libs/crypto/src/lib.rs`

- [ ] **Step 1: Create the workspace root `Cargo.toml`**

```toml
# Workspace root for the Kernel project (working title).
# Member crates are intentional Phase 0 placeholders that compile on the
# host. They become the real microkernel, HAL, arch layer, and libraries
# in later phases. See docs/roadmap/roadmap.md.
[workspace]
resolver = "2"
members = [
    "kernel",
    "hal",
    "arch/riscv64",
    "libs/common",
    "libs/crypto",
]

[workspace.package]
version = "0.0.0"
edition = "2021"
license = "Apache-2.0"
authors = ["Kathir"]
repository = ""

# Single source of truth for the provisional project name (see ADR 0006).
# When the project is renamed, this and the crate names are the only
# identifiers to update.
[workspace.metadata.project]
name = "kernel"
display-name = "Kernel (working title)"
provisional-name = true
```

- [ ] **Step 2: Create the `common` library crate**

`libs/common/Cargo.toml`:
```toml
[package]
name = "kernel-common"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
```

`libs/common/src/lib.rs`:
```rust
//! Shared types and utilities used across the Kernel project.
//!
//! Phase 0 placeholder. Real shared types (capabilities, error types,
//! IDs) arrive in later phases.

/// Returns the provisional project name.
///
/// Centralizing this constant keeps the working title in one place so a
/// future rename (ADR 0006) is a single edit.
pub const PROJECT_NAME: &str = "Kernel (working title)";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn project_name_is_set() {
        assert!(!PROJECT_NAME.is_empty());
    }
}
```

- [ ] **Step 3: Create the `crypto` library crate**

`libs/crypto/Cargo.toml`:
```toml
[package]
name = "kernel-crypto"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
```

`libs/crypto/src/lib.rs`:
```rust
//! Cryptographic primitives for the Kernel project.
//!
//! Phase 0 placeholder. Per ADR 0004, this will provide a post-quantum
//! cryptography baseline (e.g. ML-KEM / ML-DSA) in Phase 3, preferring
//! audited libraries over hand-rolled crypto.

/// Marker for the planned post-quantum baseline. Not yet implemented.
pub const PQC_PLANNED: bool = true;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pqc_is_planned() {
        assert!(PQC_PLANNED);
    }
}
```

- [ ] **Step 4: Create the `arch/riscv64` crate**

`arch/riscv64/Cargo.toml`:
```toml
[package]
name = "kernel-arch-riscv64"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
```

`arch/riscv64/src/lib.rs`:
```rust
//! RISC-V (riscv64) architecture-specific code — the first target.
//!
//! Phase 0 placeholder. Boot, trap handling, and CPU-specific logic
//! arrive in Phase 1+. Other architectures (x86-64, ARM64) get sibling
//! crates later; the HAL keeps them interchangeable.

/// The architecture identifier this crate targets.
pub const ARCH: &str = "riscv64";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arch_is_riscv64() {
        assert_eq!(ARCH, "riscv64");
    }
}
```

- [ ] **Step 5: Create the `hal` crate**

`hal/Cargo.toml`:
```toml
[package]
name = "kernel-hal"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
```

`hal/src/lib.rs`:
```rust
//! Hardware Abstraction Layer — the device-agnostic boundary.
//!
//! Phase 0 placeholder. This is where every device (today's hardware and
//! future accelerators like GPUs/NPUs/QPUs) registers behind a uniform
//! interface, keeping the kernel hardware-agnostic. See
//! docs/architecture/hardware-abstraction.md.

/// True once at least one backend is wired up. None in Phase 0.
pub const HAS_BACKEND: bool = false;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_backend_in_phase_0() {
        assert!(!HAS_BACKEND);
    }
}
```

- [ ] **Step 6: Create the `kernel` crate**

`kernel/Cargo.toml`:
```toml
[package]
name = "kernel"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
kernel-common = { path = "../libs/common" }
```

`kernel/src/lib.rs`:
```rust
//! The microkernel (working title).
//!
//! Phase 0 placeholder that compiles on the host. In Phase 1 this becomes
//! a `no_std` freestanding binary that boots under QEMU on riscv64 and
//! prints "hello world". For now it only exposes the project name.

use kernel_common::PROJECT_NAME;

/// Returns a startup banner string. Real boot code arrives in Phase 1.
pub fn banner() -> String {
    format!("{PROJECT_NAME} — Phase 0 foundation")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn banner_mentions_project() {
        assert!(banner().contains("working title"));
    }
}
```

- [ ] **Step 7: Verify the whole workspace builds and tests pass on the host**

Run:
```powershell
cargo build
cargo test
```
Expected: `cargo build` finishes with `Finished` and no errors. `cargo test` runs the small unit tests in each crate and reports all passing (`test result: ok`). If `cargo` tries to build for `riscv64` and fails, Task 4 has not been done yet and `.cargo/config.toml` must NOT set a default target — confirm no default target is set.

- [ ] **Step 8: Commit**

```powershell
git add Cargo.toml kernel hal arch libs
git commit -m "feat: scaffold Cargo workspace with placeholder crates (kernel, hal, arch, libs)"
```

---

## Task 4: RISC-V cross-build configuration (staged for Phase 1)

Stages the custom target spec and `.cargo/config.toml`. Critically, **do not set a default build target here** — that would break the Phase 0 host build. The RISC-V target is installed via `rust-toolchain.toml` (Task 1); Phase 1 will activate `build-std` and the target.

**Files:**
- Create: `.cargo/config.toml`
- Create: `riscv64gc-unknown-none-elf.json`

- [ ] **Step 1: Create `.cargo/config.toml`**

```toml
# Cargo configuration.
#
# Phase 0: intentionally does NOT set a default `[build] target`, so
# `cargo build`/`cargo test` run on the host and stay green.
#
# Phase 1 will uncomment the lines below to cross-build the freestanding
# kernel for RISC-V and run it under QEMU. They are documented here now so
# the intent is captured.
#
# [build]
# target = "riscv64gc-unknown-none-elf"
#
# [unstable]
# build-std = ["core", "compiler_builtins", "alloc"]
# build-std-features = ["compiler-builtins-mem"]
#
# [target.riscv64gc-unknown-none-elf]
# runner = "qemu-system-riscv64 -machine virt -nographic -bios default -kernel"

[alias]
# Convenience aliases usable today.
lint = "clippy --workspace --all-targets"
```

- [ ] **Step 2: Create the custom target spec `riscv64gc-unknown-none-elf.json`**

This documents the exact bare-metal target Phase 1 will build for. (The built-in target of the same name exists; keeping an explicit JSON makes the configuration visible and tweakable.)

```json
{
  "llvm-target": "riscv64",
  "data-layout": "e-m:e-p:64:64-i64:64-i128:128-n32:64-S128",
  "arch": "riscv64",
  "target-pointer-width": "64",
  "is-builtin": false,
  "os": "none",
  "executables": true,
  "linker": "rust-lld",
  "linker-flavor": "ld.lld",
  "panic-strategy": "abort",
  "relocation-model": "static",
  "code-model": "medium",
  "cpu": "generic-rv64",
  "features": "+m,+a,+f,+d,+c",
  "llvm-abiname": "lp64d",
  "max-atomic-width": 64,
  "disable-redzone": true,
  "eh-frame-header": false
}
```

- [ ] **Step 3: Verify the host build is still green (config did not break it)**

Run:
```powershell
cargo build
```
Expected: still `Finished`, still builds for the host (no attempt to build for riscv64). This confirms `.cargo/config.toml` correctly left the default target unset.

- [ ] **Step 4: Verify the custom target JSON is valid**

Run:
```powershell
rustc --print target-spec-json -Z unstable-options --target riscv64gc-unknown-none-elf.json
```
Expected: prints a JSON target spec without an error. (This only validates the file parses; we do not build against it until Phase 1.)

- [ ] **Step 5: Commit**

```powershell
git add .cargo/config.toml riscv64gc-unknown-none-elf.json
git commit -m "chore: stage RISC-V cross-build config and target spec for Phase 1"
```

---

## Task 5: Developer tooling scripts

PowerShell helper scripts so common actions are one command and documented.

**Files:**
- Create: `tools/build.ps1`, `tools/run-qemu.ps1`, `tools/README.md`

- [ ] **Step 1: Create `tools/build.ps1`**

```powershell
# Builds the workspace. Phase 0: host build.
# Usage: ./tools/build.ps1
$ErrorActionPreference = "Stop"
Write-Host "Building workspace (host)..." -ForegroundColor Cyan
cargo build
cargo test
Write-Host "Build + tests OK." -ForegroundColor Green
```

- [ ] **Step 2: Create `tools/run-qemu.ps1`**

```powershell
# Phase 0: proves the RISC-V virtual machine + firmware boot, before our
# kernel exists. Boots QEMU's built-in OpenSBI firmware.
# Phase 1 will extend this to load our kernel binary with `-kernel`.
# Exit QEMU with: Ctrl-A then X
$ErrorActionPreference = "Stop"
Write-Host "Booting QEMU RISC-V (OpenSBI firmware). Exit with Ctrl-A X." -ForegroundColor Cyan
qemu-system-riscv64 -machine virt -nographic -bios default
```

- [ ] **Step 3: Create `tools/README.md`**

Content must include: purpose of the folder; a one-line description of each script; the exact run commands (`./tools/build.ps1`, `./tools/run-qemu.ps1`); and the note that `run-qemu.ps1` currently boots firmware only (our kernel comes in Phase 1) and how to exit QEMU (`Ctrl-A` then `X`).

- [ ] **Step 4: Verify the build script runs**

Run:
```powershell
./tools/build.ps1
```
Expected: prints "Building workspace (host)...", builds, runs tests, prints "Build + tests OK." in green.

- [ ] **Step 5: Commit**

```powershell
git add tools
git commit -m "chore: add build and run-qemu helper scripts"
```

---

## Task 6: README and LICENSE

The repository's front door and its open-source license.

**Files:**
- Create: `LICENSE`, `README.md`

- [ ] **Step 1: Create `LICENSE` (Apache-2.0)**

Fetch the exact, standard Apache License 2.0 text. The executor must write the **full, verbatim** Apache-2.0 text (the standard ~11 KB license). Fill the copyright line in the appendix as `Copyright 2026 Kathir`. Do not paraphrase or truncate the license. The canonical text is at https://www.apache.org/licenses/LICENSE-2.0.txt — reproduce it exactly.

- [ ] **Step 2: Create `README.md`**

Content must include, in this order:
1. Project title: **Kernel** *(working title — see ADR 0006)*.
2. A 2–3 sentence elevator pitch drawn from the spec: a from-scratch, security-first, hardware-agnostic microkernel in Rust with a self-healing "knowledge organism" support model.
3. **Status:** "Phase 0 — foundation. No kernel functionality yet; this is the documented skeleton and verified dev environment."
4. **Why this exists** — 3 bullets: trusted/secure-by-design; community-independent self-healing; future-hardware-ready via clean abstraction.
5. **The decisions so far** — a short table mirroring the spec's decision table (Rust, microkernel, RISC-V/QEMU, post-quantum crypto, self-healing knowledge organism, provisional name) with one-line rationale each.
6. **Repository layout** — a trimmed version of the directory tree from the spec with one-line descriptions.
7. **Getting started** — prerequisites (Rust nightly via rustup, QEMU) and the two commands: `./tools/build.ps1` and `./tools/run-qemu.ps1`, including how to exit QEMU.
8. **Roadmap** — the phase list (0→6+), linking to `docs/roadmap/roadmap.md`.
9. **Documentation map** — links to `docs/vision/`, `docs/architecture/`, `docs/decisions/`, `docs/glossary.md`, `knowledge-base/`.
10. **License** — Apache-2.0, linking to `LICENSE`.

- [ ] **Step 3: Verify links and structure**

Run:
```powershell
Test-Path README.md, LICENSE
Select-String -Path README.md -Pattern "Phase 0", "Apache-2.0", "working title" | Select-Object Line
```
Expected: both files exist (`True`, `True`); the grep finds the status, license, and working-title references.

- [ ] **Step 4: Commit**

```powershell
git add README.md LICENSE
git commit -m "docs: add README and Apache-2.0 license"
```

---

## Task 7: Vision documents

**Files:**
- Create: `docs/vision/north-star.md`, `docs/vision/principles.md`

- [ ] **Step 1: Create `docs/vision/north-star.md`**

Content must capture, faithfully to the spec §1: the one-paragraph vision; the **north star priority order** (1. trusted secure product, 2. self-healing knowledge organism, 3. supporting: future/quantum readiness via clean architecture); and the **non-goals** list (no POSIX/Linux compat, no feature parity, no perf-before-correctness, no OS-on-a-QPU, no AI in the privileged kernel, no many-architectures-at-once early). State explicitly that "a tiny verified hello-world kernel is a legitimate success."

- [ ] **Step 2: Create `docs/vision/principles.md`**

Content must list and briefly explain each guiding principle from spec §2: security from first principles (smallest TCB wins); clean boundaries over chasing trends; start simple then grow; everything is documented (ADRs); the OS should explain itself (diagnosability first-class); learning is part of the work. One short paragraph per principle.

- [ ] **Step 3: Verify**

Run:
```powershell
Test-Path docs/vision/north-star.md, docs/vision/principles.md
Select-String -Path docs/vision/north-star.md -Pattern "non-goal", "self-healing", "hello"
```
Expected: both exist; pattern matches found.

- [ ] **Step 4: Commit**

```powershell
git add docs/vision
git commit -m "docs: add vision (north star + principles)"
```

---

## Task 8: Architecture documents

**Files:**
- Create: `docs/architecture/overview.md`, `security-model.md`, `hardware-abstraction.md`, `self-healing.md`

- [ ] **Step 1: Create `docs/architecture/overview.md`**

Content: explain the microkernel layering from spec §4. Include the ASCII layer diagram (user space → microkernel core → HAL → hardware) from the spec. Define, in beginner-friendly terms: kernel space vs user space, the Trusted Computing Base (TCB), and why isolated user-space services enable both security and self-healing. Link to the other three architecture docs.

- [ ] **Step 2: Create `docs/architecture/security-model.md`**

Content from spec §4 "Security model": capability-based access control (no ambient authority — a component acts only via unforgeable capabilities it holds); minimal TCB; post-quantum crypto baseline (ADR 0004); and the **safety cage** for self-healing (every automated action is capability-checked, logged, reversible, auditable; the healer can never gain unchecked power). Explain each in plain language for a learner.

- [ ] **Step 3: Create `docs/architecture/hardware-abstraction.md`**

Content from spec §3 "future hardware" + §4 HAL: the HAL is the device-agnostic boundary; architecture-specific code lives under `arch/`; QPUs/NPUs/TPUs/GPUs are *accelerators (devices), not kernel targets*, so they need no early investment and slot in behind the HAL later. State the portability strategy: RISC-V first, x86-64/ARM64 as ports, not rewrites.

- [ ] **Step 4: Create `docs/architecture/self-healing.md`**

Content from spec §4 "self-healing": the knowledge organism is a structured, machine- and human-readable store of issue records + fix playbooks under `knowledge-base/`; the OS consults it first; Phase 0 seeds schema + store, diagnosis logic is Phase 5; evolution path = deterministic rules + KB → later an isolated AI *advisor* that suggests (never silently acts) → always inside the safety cage; the deterministic explainable core is never replaced by a black box. Link to `knowledge-base/README.md`.

- [ ] **Step 5: Verify**

Run:
```powershell
Get-ChildItem docs/architecture/*.md | Select-Object Name
Select-String -Path docs/architecture/security-model.md -Pattern "capabilit", "safety cage", "post-quantum"
```
Expected: four files listed; pattern matches found.

- [ ] **Step 6: Commit**

```powershell
git add docs/architecture
git commit -m "docs: add architecture (overview, security, HAL, self-healing)"
```

---

## Task 9: Architecture Decision Records (ADRs)

Short, permanent records of *why* each decision was made. Use a consistent template.

**Files:**
- Create: `docs/decisions/README.md` and `0001`–`0006` ADR files.

**ADR template** (each ADR file follows this; fill from the spec's decision table §3):
```markdown
# ADR NNNN: <Title>

- **Status:** Accepted
- **Date:** 2026-06-06

## Context
<the problem / forces at play, 2–4 sentences>

## Decision
<what we chose, stated plainly>

## Consequences
<trade-offs accepted, what this enables, what it costs>
```

- [ ] **Step 1: Create `docs/decisions/README.md`**

Content: explain what an ADR is (a short immutable record of one significant decision and its rationale) and why a solo multi-year project relies on them. Include an **index table** listing ADRs 0001–0006 with title and status, each linking to its file.

- [ ] **Step 2: Create `docs/decisions/0001-language-rust.md`**

Context: OS vulnerabilities are dominated (~70%) by memory-safety bugs in C; security is our north star. Decision: Rust. Consequences: compile-time elimination of whole vuln classes; steeper learning curve; smaller-but-growing kernel ecosystem (Rust-for-Linux, Redox); some `unsafe` still required and must be isolated/reviewed.

- [ ] **Step 3: Create `docs/decisions/0002-microkernel.md`**

Context: architecture style sets the security ceiling; TCB size = attack surface. Decision: capability-based microkernel (seL4-inspired). Consequences: smallest privileged core; isolated restartable services (enables self-healing); only style with a formally verified secure kernel; cost = IPC/message-passing complexity and performance care, learned gradually.

- [ ] **Step 4: Create `docs/decisions/0003-first-target-riscv.md`**

Context: must pick one ISA to learn on while staying portable; owner has x86-64 laptops and ARM64 phones. Decision: RISC-V (riscv64) on QEMU first, portable design. Consequences: cleanest to learn, future-forward; develop safely in emulator; real hardware later = cheap board or port to laptop; other ISAs become ports via the HAL.

- [ ] **Step 5: Create `docs/decisions/0004-post-quantum-crypto.md`**

Context: future quantum computers threaten today's crypto; "trusted" is the goal; quantum hardware is not a kernel target. Decision: adopt a post-quantum cryptography baseline (e.g. NIST ML-KEM/ML-DSA) in the security foundation; prefer audited libraries over hand-rolled crypto. Consequences: ahead of the curve; ordinary code on ordinary chips; library selection deferred to Phase 3.

- [ ] **Step 6: Create `docs/decisions/0005-self-healing-knowledge-organism.md`**

Context: want community-independent support; AI must not become the biggest vulnerability. Decision: self-healing "knowledge organism" — structured issue+fix memory, deterministic explainable core, AI as an isolated advisor later, all inside the safety cage (capability-checked, logged, reversible, auditable). Consequences: trustworthy and explainable; KB schema seeded in Phase 0; diagnosis logic in Phase 5; never put AI/ML in the privileged kernel.

- [ ] **Step 7: Create `docs/decisions/0006-project-name-placeholder.md`**

Context: naming shouldn't block momentum, but late renames are error-prone. Decision: use provisional name "Kernel"; keep it rename-safe. Consequences + **rename procedure checklist**: update (a) `[workspace.metadata.project]` in root `Cargo.toml`, (b) `PROJECT_NAME` in `libs/common/src/lib.rs`, (c) crate names if desired, (d) doc references to "Kernel (working title)", (e) repo/folder name. State that docs consistently use "Kernel (working title)" so references are greppable.

- [ ] **Step 8: Verify**

Run:
```powershell
Get-ChildItem docs/decisions/*.md | Select-Object Name
Select-String -Path docs/decisions/0006-project-name-placeholder.md -Pattern "rename", "PROJECT_NAME"
```
Expected: seven files (README + 0001–0006); rename-procedure references found.

- [ ] **Step 9: Commit**

```powershell
git add docs/decisions
git commit -m "docs: add ADRs 0001-0006 recording foundational decisions"
```

---

## Task 10: Roadmap, glossary, and learning notes

**Files:**
- Create: `docs/roadmap/roadmap.md`, `docs/glossary.md`, `docs/learning/README.md`, `docs/learning/0001-dev-environment.md`

- [ ] **Step 1: Create `docs/roadmap/roadmap.md`**

Content: the phased roadmap from spec §5, as a living document. For each phase (0–6+) give: the goal (one line), what the user learns, and the "done" signal. Mark Phase 0 as **in progress**. Note that each phase gets its own brainstorm → spec → plan cycle.

- [ ] **Step 2: Create `docs/glossary.md`**

Content: plain-language definitions of the terms a newcomer meets in these docs. Must include at least: kernel, kernel space, user space, microkernel, monolithic kernel, Trusted Computing Base (TCB), capability, IPC, HAL, ISA, RISC-V, QEMU, emulator, `no_std`, freestanding binary, cross-compilation, bootloader/firmware, OpenSBI, post-quantum cryptography, ADR, self-healing/knowledge organism, safety cage, accelerator (GPU/NPU/TPU/QPU). One or two sentences each.

- [ ] **Step 3: Create `docs/learning/README.md`**

Content: explain this folder holds the author's learning notes as systems concepts click into place; numbered notes; encourage capturing "what confused me and what made it click." Link to note 0001.

- [ ] **Step 4: Create `docs/learning/0001-dev-environment.md`**

Content: record what was set up in Phase 0 — Rust nightly via rustup and why nightly (build-std later), the riscv64 target, QEMU, and the OpenSBI boot check (what the OpenSBI banner means). Note the **native-Windows** choice and that **WSL2 is an available alternative** later for a more Unix-style workflow. Include the exit-QEMU tip (`Ctrl-A` then `X`).

- [ ] **Step 5: Verify**

Run:
```powershell
Test-Path docs/roadmap/roadmap.md, docs/glossary.md, docs/learning/README.md, docs/learning/0001-dev-environment.md
Select-String -Path docs/glossary.md -Pattern "microkernel", "OpenSBI", "capability"
```
Expected: all exist; glossary terms found.

- [ ] **Step 6: Commit**

```powershell
git add docs/roadmap docs/glossary.md docs/learning
git commit -m "docs: add roadmap, glossary, and learning notes"
```

---

## Task 11: Knowledge-base seed

The first cell of the self-healing organism: a documented schema and store, with one worked example. No runtime logic — just the data format the future system will read.

**Files:**
- Create: `knowledge-base/README.md`, `knowledge-base/schema/issue-record.md`, `knowledge-base/schema/example-0001.md`, `knowledge-base/entries/.gitkeep`

- [ ] **Step 1: Create `knowledge-base/README.md`**

Content: what this tree is (the OS's growing memory of issues + proven fixes); how the OS will *consult it first* before external help (per ADR 0005); the safety-cage reminder (fixes are capability-checked, logged, reversible, auditable); the directory layout (`schema/` defines the record format, `entries/` holds individual records); and that Phase 0 only seeds the format — diagnosis/healing logic is Phase 5.

- [ ] **Step 2: Create `knowledge-base/schema/issue-record.md`**

Content: define the **issue record schema** as a documented set of fields, with a YAML-frontmatter convention so records are both human- and machine-readable. Specify these fields with type and meaning:
- `id` (string, e.g. `KB-0001`)
- `title` (short summary)
- `status` (`open` | `diagnosed` | `fixed` | `wont-fix`)
- `severity` (`low` | `medium` | `high` | `critical`)
- `component` (subsystem, e.g. `boot`, `memory`, `hal`)
- `symptoms` (observable signs)
- `diagnosis` (root cause once known)
- `playbook` (ordered, reversible steps to fix)
- `verification` (how to confirm the fix worked)
- `created` / `updated` (ISO dates)
- `references` (links to ADRs/docs/commits)

Include a blank template block users can copy.

- [ ] **Step 3: Create `knowledge-base/schema/example-0001.md`**

Content: a filled, realistic example record using the schema, documenting a *real* issue already solved in this project — the **SSH commit-signing failure** (git used the bundled ssh-keygen that couldn't reach the Windows ssh-agent; fix: set `gpg.ssh.program` to the system OpenSSH ssh-keygen, with the key cached in ssh-agent). This both validates the schema and seeds the organism with a genuine first memory. Use `id: KB-0001`, `status: fixed`, `component: dev-environment`, a reversible playbook, and a verification step.

- [ ] **Step 4: Create `knowledge-base/entries/.gitkeep`**

Content:
```
# Individual diagnosed-issue records accumulate here over the project's
# life. The format is defined in ../schema/issue-record.md.
```

- [ ] **Step 5: Verify**

Run:
```powershell
Get-ChildItem -Recurse knowledge-base | Select-Object FullName
Select-String -Path knowledge-base/schema/example-0001.md -Pattern "KB-0001", "gpg.ssh.program"
```
Expected: README, schema/issue-record.md, schema/example-0001.md, entries/.gitkeep listed; example references found.

- [ ] **Step 6: Commit**

```powershell
git add knowledge-base
git commit -m "docs: seed knowledge-base (schema + first real issue record)"
```

---

## Task 12: CONTRIBUTING and final acceptance verification

Adds the contributor guide and verifies the whole Phase 0 against the spec's acceptance criteria.

**Files:**
- Create: `CONTRIBUTING.md`

- [ ] **Step 1: Create `CONTRIBUTING.md`**

Content: how to set up (link Task 1 prerequisites / `docs/learning/0001-dev-environment.md`); how to build and run (`./tools/build.ps1`, `./tools/run-qemu.ps1`); the commit conventions (Conventional Commits, no co-author trailer, signed commits); where decisions go (add an ADR for any significant choice); the principle that every change keeps the host build green and docs updated. Keep it short and welcoming; note the project is early and solo.

- [ ] **Step 2: Acceptance — single-command build succeeds**

Run:
```powershell
./tools/build.ps1
```
Expected: builds and tests pass ("Build + tests OK.").

- [ ] **Step 3: Acceptance — QEMU dev environment verified**

Run:
```powershell
qemu-system-riscv64 -machine virt -nographic -bios default
```
Expected: OpenSBI banner prints (RISC-V VM + firmware boot). Exit with `Ctrl-A` then `X`.

- [ ] **Step 4: Acceptance — all founding documents exist and are non-empty**

Run:
```powershell
$required = @(
  "README.md","LICENSE","CONTRIBUTING.md","rust-toolchain.toml","Cargo.toml",
  "docs/vision/north-star.md","docs/vision/principles.md",
  "docs/architecture/overview.md","docs/architecture/security-model.md",
  "docs/architecture/hardware-abstraction.md","docs/architecture/self-healing.md",
  "docs/decisions/README.md","docs/decisions/0001-language-rust.md",
  "docs/decisions/0002-microkernel.md","docs/decisions/0003-first-target-riscv.md",
  "docs/decisions/0004-post-quantum-crypto.md","docs/decisions/0005-self-healing-knowledge-organism.md",
  "docs/decisions/0006-project-name-placeholder.md",
  "docs/roadmap/roadmap.md","docs/glossary.md","docs/learning/README.md","docs/learning/0001-dev-environment.md",
  "knowledge-base/README.md","knowledge-base/schema/issue-record.md","knowledge-base/schema/example-0001.md"
)
$missing = $required | Where-Object { -not (Test-Path $_) -or ((Get-Item $_).Length -eq 0) }
if ($missing) { "MISSING/EMPTY:"; $missing } else { "ALL PRESENT AND NON-EMPTY" }
```
Expected: `ALL PRESENT AND NON-EMPTY`.

- [ ] **Step 5: Acceptance — newcomer readability check (manual)**

Manually confirm: reading `README.md` → `docs/vision/north-star.md` → `docs/roadmap/roadmap.md` explains what the project is, why each major decision was made (via ADR links), and what Phase 1 is. Fix any gaps found.

- [ ] **Step 6: Commit**

```powershell
git add CONTRIBUTING.md
git commit -m "docs: add CONTRIBUTING guide; complete Phase 0 foundation"
```

- [ ] **Step 7: Confirm clean tree**

Run:
```powershell
git status -s
git log --oneline
```
Expected: clean working tree; a sequence of well-described Phase 0 commits.

---

## Self-Review (completed)

**Spec coverage:** Every spec §7 deliverable maps to a task — directory structure (T2), founding docs (T6–T10), knowledge-base seed (T11), compiling Rust workspace (T3), `.cargo`/RISC-V setup (T1 target install + T4 config), tools (T5), LICENSE/.gitignore (T6 / already done), git init (already done). Every acceptance criterion is verified in T12. All six ADRs (incl. naming, spec §8) are in T9.

**Placeholder scan:** No "TBD/TODO/implement later" left as work items. Prose-doc tasks specify exact required content/sections sourced from the approved spec (not invention) — the executor transcribes decided content into the right files rather than inventing requirements.

**Type consistency:** `PROJECT_NAME` (libs/common) is referenced consistently by kernel; crate names (`kernel`, `kernel-common`, `kernel-crypto`, `kernel-arch-riscv64`, `kernel-hal`) are used consistently across `Cargo.toml` members and the dependency in `kernel/Cargo.toml`. The rename checklist (ADR 0006) points at the real identifiers defined here.

**Scope:** Phase 0 only — no kernel logic; the host build stays green and the RISC-V cross-build is staged, not activated. Phase 1 owns the freestanding boot.
```
