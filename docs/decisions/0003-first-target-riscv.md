# ADR 0003: First target architecture — RISC-V on QEMU

- **Status:** Accepted
- **Date:** 2026-06-06

## Context

The OS must be hardware-agnostic eventually, but development has to start on **one** CPU instruction set. The author owns x86-64 laptops and ARM64 phones. The candidates: x86-64 (owned hardware, but decades of legacy boot complexity), ARM64 (owned phones, but locked bootloaders make custom kernels very hard), and RISC-V (an open, modern, royalty-free ISA with no legacy baggage; no owned hardware, but excellent emulator support).

## Decision

Target **RISC-V (riscv64)** first, developed in the **QEMU** emulator, with the codebase kept portable.

## Consequences

- **Enables:** the cleanest learning path (time spent on real concepts, not 1980s boot hacks); a future-forward, open ISA aligned with the project's vision; safe development in an emulator without risking real hardware.
- **Costs:** real RISC-V hardware later means a cheap board purchase (~$50–130), or porting to an owned x86-64 laptop instead. Either is years away and optional.
- **Strategy:** architecture-specific code is isolated under `arch/`, so other CPUs (x86-64, ARM64) become *ports*, not rewrites.
