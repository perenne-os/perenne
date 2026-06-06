# Kernel — Foundation Design (Phase 0)

- **Working title:** Kernel *(provisional — see Naming below)*
- **Date:** 2026-06-06
- **Status:** Approved — ready for implementation planning
- **Scope of this document:** The project's founding vision and the **Phase 0 foundation**. Later phases get their own brainstorm → spec → plan cycles.

---

## 1. Vision

A from-scratch, security-first, hardware-agnostic operating system kernel that can eventually run across consumer devices (PC, laptop, mobile, tablet, IoT) and accommodate future hardware (quantum and AI accelerators) without architectural rewrites. Its defining feature is a **self-healing "knowledge organism"**: instead of depending on a human community for support, the OS diagnoses its own issues, records them with proven fixes, and consults that growing knowledge first.

This is a deliberate, multi-year, solo, open-source effort. Correctness, security, and clarity come before speed or feature breadth. **A tiny, verified "hello world" kernel is a legitimate success** — the goal is a trustworthy foundation that grows steadily.

### North star (priorities, in order)

1. **A trusted, secure product** — security is non-negotiable and architected in from the start, not bolted on.
2. **The self-healing knowledge organism** — the soul of the project; community-independent, self-diagnosing support.
3. *(Supporting)* Future/quantum readiness — valued, but achieved through clean architecture rather than early investment.

### Non-goals (explicitly out of scope, possibly forever)

- POSIX/Linux compatibility or running existing apps unmodified.
- Feature parity with mainstream OSes.
- Performance optimization ahead of correctness and security.
- Running an OS *on* a quantum processor (a QPU is an accelerator, not a kernel target).
- Putting AI/ML models inside the privileged kernel.
- Supporting many architectures at once early on.

---

## 2. Guiding principles

- **Security from first principles.** Every design choice is evaluated against its effect on the attack surface (Trusted Computing Base). Smallest TCB wins.
- **Clean boundaries over chasing trends.** We stay future-proof by investing in well-defined interfaces (so new hardware/tech slots in), not by adopting today's hot technology early.
- **Start simple, then grow.** Each step is small, real, finishable, and teaches the concept the next step needs.
- **Everything is documented.** A solo, multi-year project survives on written rationale. Decisions are recorded as ADRs so future-us knows *why*.
- **The OS should explain itself.** Diagnosability and self-knowledge are first-class, beginning in Phase 0 with the knowledge-base seed.
- **Learning is part of the work.** The author is new to systems programming; concepts are explained and captured in `docs/learning/` and `docs/glossary.md`.

---

## 3. Key decisions (with rationale)

Each is recorded as an Architecture Decision Record (ADR) in `docs/decisions/`.

| # | Decision | Choice | Rationale |
|---|----------|--------|-----------|
| 0001 | Implementation language | **Rust** | Memory safety eliminates ~70% of typical OS vulnerability classes at compile time; modern tooling; growing real-world kernel use (Rust-for-Linux, Redox). Directly serves the security north star. |
| 0002 | Kernel architecture | **Microkernel** (capability-based, seL4-inspired) | Smallest privileged core = smallest attack surface; drivers/filesystems/networking run as isolated, restartable user-space services; the only architecture style with a formally verified secure kernel (seL4); natural home for self-healing (isolated services can be diagnosed and revived independently). |
| 0003 | First target architecture | **RISC-V (riscv64) on QEMU**, with portable design | Cleanest ISA to learn (no legacy boot complexity), future-forward and open; QEMU lets us develop safely on the existing laptop. Code kept portable so x86-64 (owned laptops) and ARM64 become *ports*, not rewrites. |
| 0004 | Cryptography baseline | **Post-quantum cryptography (PQC)** in the security foundation (e.g., NIST ML-KEM / ML-DSA) | "Quantum readiness" that matters today is protecting the OS against future quantum attackers. PQC is ordinary code on ordinary chips; adopting it early puts us ahead of nearly everyone and serves "trusted." |
| 0005 | Support model | **Self-healing "knowledge organism"** | Structured, growing memory of issues + proven fixes ("playbooks"). Deterministic and explainable at its core; AI/ML may be added later only as an isolated user-space *advisor* that suggests, never silently acts. All self-healing actions run inside a **safety cage**: capability-checked, logged, reversible, auditable. |
| 0006 | Project name | **Placeholder "Kernel"** (provisional) | Naming deferred to avoid blocking momentum. Designed to be rename-safe (see §8). |

### How future hardware (quantum, AI accelerators) fits

QPUs, NPUs, TPUs, and GPUs are **accelerators (devices), not kernel targets.** A classical CPU stays in control and dispatches jobs to them. Therefore they require **no early investment**: the same clean Hardware Abstraction Layer (HAL) that makes us device-agnostic lets a future accelerator register as "just another device." We design the *boundary* now; we implement specific accelerator support only when relevant, far in the future.

---

## 4. Architecture overview

```
              ┌───────────────────────────────────────────────┐
  USER SPACE  │  Apps   Drivers   Filesystem   Network   ...   │  ← isolated, restartable
  (isolated)  │  Self-healing services (knowledge organism,    │
              │  AI advisor later) — all inside the safety cage │
              └───────────────────────────────────────────────┘
                                   ▲  message passing (IPC)
                                   ▼
              ┌───────────────────────────────────────────────┐
  KERNEL      │   MICROKERNEL (tiny, privileged, verifiable)   │
  SPACE       │   memory · scheduling · IPC · capabilities     │
              └───────────────────────────────────────────────┘
                                   ▲
                                   ▼
              ┌───────────────────────────────────────────────┐
  HAL         │  Hardware Abstraction Layer (device-agnostic)  │  ← future chips slot in here
              └───────────────────────────────────────────────┘
                                   ▲
                                   ▼
              ┌───────────────────────────────────────────────┐
  HARDWARE    │  RISC-V first (QEMU) → x86-64, ARM64, ...      │
              └───────────────────────────────────────────────┘
```

- **Microkernel core (kernel space):** only memory management, scheduling, inter-process communication (IPC), and the capability system run privileged. Kept small enough to reason about — and, aspirationally, to verify.
- **User-space services:** drivers, filesystems, networking, and the self-healing system run as isolated processes. A crash is contained and recoverable.
- **HAL:** the device-agnostic boundary. Architecture-specific code lives under `arch/`; the HAL presents a uniform interface upward and is where future devices/accelerators register.

### Security model (summary)

- **Capability-based access control:** a component can only act on resources it holds an explicit, unforgeable capability for. No ambient authority.
- **Minimal TCB:** the privileged core is deliberately tiny.
- **Post-quantum crypto** as the cryptographic baseline.
- **Safety cage for self-healing:** any automated fix must be capability-checked, logged, reversible, and auditable. The healing system can never gain unchecked power — otherwise it becomes the system's biggest vulnerability.

### Self-healing knowledge organism (summary)

- A structured, machine- and human-readable store of **issue records** and **fix playbooks** under `knowledge-base/`.
- The OS consults this store *first* when it encounters a known condition.
- Phase 0 seeds the **schema and store**; actual diagnosis/healing logic is Phase 5.
- Evolution path: deterministic rules + knowledge base → (later) an isolated AI advisor that *suggests* fixes → always within the safety cage. The deterministic, explainable core is never replaced by a black box.

---

## 5. Roadmap

Each phase is brainstormed and specced separately when reached.

- **Phase 0 — Foundation & vision** *(this spec)*: repo skeleton, founding docs, ADRs, roadmap, glossary, knowledge-base seed, a Rust workspace that compiles, QEMU verified.
- **Phase 1 — Hello world from our own kernel:** boot a tiny kernel in QEMU and print to screen. Learn boot, freestanding Rust, the toolchain.
- **Phase 2 — The kernel grows up:** memory management, interrupts, basic scheduling — still in QEMU.
- **Phase 3 — Security spine:** capability-based isolation and post-quantum crypto primitives, designed in from here.
- **Phase 4 — Real hardware:** boot on an owned x86-64 laptop (first port) or a cheap RISC-V board.
- **Phase 5 — Self-healing seed:** first working version of the diagnosis/knowledge system.
- **Phase 6+ — Breadth:** more hardware (ARM/phones), fuller HAL, device drivers, the long tail.

---

## 6. Repository structure

```
Kernel/                         repo root (working title)
├── README.md                   Vision summary, status, how to build
├── LICENSE                     Apache-2.0
├── CONTRIBUTING.md             How others (and future-self) contribute
├── .gitignore                  Ignores build output, .superpowers/, etc.
│
├── docs/                       ── Human-facing documentation ──
│   ├── vision/
│   │   ├── north-star.md       Goals, non-goals, priorities
│   │   └── principles.md       Guiding principles
│   ├── architecture/
│   │   ├── overview.md         Microkernel layers, the big picture
│   │   ├── security-model.md   Capabilities, PQC, the safety cage, TCB
│   │   ├── hardware-abstraction.md   HAL; how devices/quantum/AI chips slot in
│   │   └── self-healing.md     The knowledge-organism design
│   ├── decisions/              ADRs — one short file per decision
│   │   ├── README.md           What an ADR is + index
│   │   ├── 0001-language-rust.md
│   │   ├── 0002-microkernel.md
│   │   ├── 0003-first-target-riscv.md
│   │   ├── 0004-post-quantum-crypto.md
│   │   ├── 0005-self-healing-knowledge-organism.md
│   │   └── 0006-project-name-placeholder.md
│   ├── roadmap/
│   │   └── roadmap.md          The phased plan (living document)
│   ├── glossary.md             Plain-language term definitions
│   └── learning/               Author's learning notes as concepts click
│       └── README.md
│
├── knowledge-base/             ── The self-knowledge seed (organism memory) ──
│   ├── README.md               What it is + how the OS will consult it
│   ├── schema/                 Format for issue + fix-playbook records
│   │   └── README.md
│   └── entries/                Individual diagnosed-issue records (grows over time)
│       └── .gitkeep
│
├── kernel/                     The microkernel (Rust) — compiling stub for now
├── arch/
│   └── riscv64/                Architecture-specific code (first target)
├── hal/                        Hardware Abstraction Layer (device-agnostic boundary)
├── services/                   User-space services: drivers, fs, net (empty for now)
├── libs/                       Shared libraries (crypto, common types)
│
├── tools/                      Scripts: build, run-in-QEMU, etc.
├── tests/                      Test harnesses
└── .cargo/                     Cargo + target-spec configuration
```

**Philosophy:** folders that are empty today (`services/`, `hal/`) still exist now, so every ambition has a home and the structure tells the story of where we're going. `docs/decisions/` preserves *why*; `knowledge-base/` is the first cell of the self-healing organism.

---

## 7. Phase 0 deliverables & acceptance criteria

Phase 0 is **documentation + skeleton + toolchain** — no kernel logic.

**Deliverables**
1. The full directory structure above, created.
2. Founding documents written: `README.md`, `north-star.md`, `principles.md`, the four `architecture/*` docs, ADRs 0001–0006, `roadmap.md`, `glossary.md`.
3. Knowledge-base seed: `knowledge-base/README.md` plus an initial issue+fix-playbook **schema** definition.
4. A Rust **workspace** (`Cargo.toml`) that compiles cleanly (even if members are near-empty stubs).
5. `.cargo/` config + RISC-V target setup so the toolchain is pinned and reproducible.
6. `tools/` script(s) to build and (placeholder) run under QEMU.
7. Apache-2.0 `LICENSE` and a `.gitignore`.
8. Git repository initialized with a clean first commit history.

**Acceptance criteria (how we know Phase 0 is done)**
- The repository builds with a single documented command and the build succeeds.
- QEMU is installed and verified runnable on the author's machine (a known-good image boots), proving the dev environment works — even before our kernel does anything.
- Every founding document listed above exists and is non-placeholder (real content).
- A newcomer (or future-self) can read `README.md` → `docs/vision/` → `docs/roadmap/` and understand what the project is, why each major decision was made, and what happens next.
- No kernel functionality is expected or implied.

---

## 8. Naming (rename-safety)

The name "Kernel" is provisional. To make a future rename seamless and complete:

- The name is referenced from a **single source of truth** (workspace/package metadata) wherever technically possible, rather than hardcoded across files.
- Documentation refers to it consistently as **"Kernel" (working title)** so references are easy to locate.
- **ADR 0006** records that the name is provisional and documents the exact rename procedure: which identifiers, package names, and document references to update, as a checklist.
- Goal: when the right name arrives, renaming is a reviewed find-and-replace with zero missed references.

---

## 9. Deferred / open questions (revisit in later phases)

- Final project name.
- Specific microkernel IPC design and capability representation (Phase 2–3).
- Choice of PQC library vs. from-scratch implementation (Phase 3) — *prefer audited libraries over hand-rolled crypto*.
- Exact knowledge-base record schema details beyond the seed (Phase 5).
- First real-hardware target: owned x86-64 laptop vs. a cheap RISC-V board (Phase 4).
- Bootloader approach for RISC-V/QEMU (Phase 1).
```
