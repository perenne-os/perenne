# Glossary

Plain-language definitions of terms used throughout these docs. Aimed at someone new to systems programming.

- **Kernel** — the core program of an operating system. It controls the CPU, memory, and devices, and decides which programs run when.
- **Kernel space** — the most privileged CPU mode. Code here can access any memory and any hardware directly. A bug here can compromise the whole machine.
- **User space** — the restricted mode ordinary programs run in. They must ask the kernel for access to resources, so a bug here is contained.
- **Monolithic kernel** — a design where most of the OS (drivers, filesystems, networking) runs together in kernel space. Fast, but a large attack surface (e.g. Linux).
- **Microkernel** — a design where only a tiny core runs in kernel space; everything else runs isolated in user space. Smaller attack surface; what this project uses (e.g. seL4).
- **Trusted Computing Base (TCB)** — all the code that must be correct for the system to be secure (essentially, everything privileged). Smaller is better.
- **Capability** — an unforgeable token that names a resource *and* grants permission to use it. A component can do only what its capabilities allow.
- **IPC (Inter-Process Communication)** — how isolated programs talk to each other, typically by passing messages. The microkernel's main job is to deliver these safely.
- **HAL (Hardware Abstraction Layer)** — a uniform interface that hides hardware differences, so the kernel doesn't need to know which specific device or chip it's running on.
- **ISA (Instruction Set Architecture)** — the "language" a CPU speaks (e.g. x86-64, ARM64, RISC-V). Code must be built for a specific ISA.
- **RISC-V** — a modern, open, royalty-free ISA. Our first target because it's clean to learn and future-forward.
- **QEMU** — software that emulates a whole computer, so we can run and test our kernel safely on an existing machine.
- **Emulator** — a program that imitates real hardware, letting software run as if on the real thing.
- **`no_std`** — a Rust mode that drops the standard library (which assumes an OS exists). Required for kernel code, since *we are* the OS.
- **Freestanding binary** — a program that runs without an underlying operating system or runtime — like a kernel.
- **Cross-compilation** — building a program on one kind of machine (e.g. an x86-64 laptop) to run on a different kind (e.g. a RISC-V computer).
- **Bootloader / firmware** — the first code that runs when a machine powers on; it initializes hardware and then hands control to the kernel.
- **OpenSBI** — the standard firmware/runtime layer for RISC-V that starts before the kernel; QEMU includes it by default.
- **Post-quantum cryptography (PQC)** — encryption algorithms designed to resist attacks from future quantum computers; ordinary software running on ordinary chips.
- **ADR (Architecture Decision Record)** — a short, dated note recording one significant decision and the reasoning behind it.
- **Self-healing / knowledge organism** — this project's support model: the OS keeps a growing memory of issues and proven fixes and consults itself to diagnose and repair problems.
- **Safety cage** — the rule that every automated self-healing action must be capability-checked, logged, reversible, and auditable, so the healer can never gain unchecked power.
- **Accelerator (GPU / NPU / TPU / QPU)** — a specialized coprocessor a normal CPU offloads work to (graphics, AI math, or quantum operations). Treated as a *device*, not something an OS runs on.
