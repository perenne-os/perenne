# Architecture Overview

This document is the big picture. The detailed pieces live in
[`security-model.md`](security-model.md),
[`hardware-abstraction.md`](hardware-abstraction.md), and
[`self-healing.md`](self-healing.md).

## The layers

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

## Key concepts (for newcomers)

- **Kernel space vs user space.** A CPU can run code at different privilege levels. *Kernel space* is the most privileged: code there can touch any memory and any device. *User space* is restricted: it must ask the kernel for access. A bug in kernel space can compromise the whole machine; a bug in user space is contained.
- **Trusted Computing Base (TCB).** The set of code that must be correct for the system to be secure — essentially everything running in kernel space. The smaller the TCB, the smaller the attack surface. Minimizing it is our central security strategy.
- **Microkernel.** We keep the kernel deliberately *tiny* — only memory management, scheduling, inter-process communication (IPC), and the capability system run privileged. Everything else (drivers, filesystems, networking) runs as ordinary, isolated programs in user space.

## Why this shape

Putting drivers and services in **isolated user-space processes** buys us two things at once:

1. **Security.** Most code — including the historically buggiest code, like device drivers — runs unprivileged. A compromised driver cannot take over the system.
2. **Self-healing.** Because each service is an independent, restartable unit, the OS can notice one crashing, record what happened, and revive it without rebooting. A monolithic kernel (where everything shares one privileged address space) literally cannot do this.

The cost is that components talk to each other by passing messages (IPC) rather than calling functions directly. That adds complexity and some overhead, which we take on deliberately and learn gradually.

## Portability

The kernel is written to be **hardware-agnostic**. Architecture-specific code is quarantined under `arch/` (starting with `arch/riscv64`), and the Hardware Abstraction Layer (`hal/`) presents a uniform interface upward. Supporting a new CPU later (x86-64, ARM64) is a *port* of those lower layers, not a rewrite of the kernel.
