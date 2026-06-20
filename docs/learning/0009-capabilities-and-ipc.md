# 0009 — Capabilities and synchronous IPC (Phase 3b-iii)

**One-line:** two isolated components now talk only through a
capability-checked synchronous endpoint — the finale of Phase 3b.

## What changed
- Each task has a small capability table (`Task.caps`). A capability is an
  *index* into your own table (`Capability::Endpoint(id)`); the kernel
  looks it up (`cap_lookup`). Unforgeable: you can only name objects you
  were granted, and can't fabricate a reference.
- An endpoint is just an id; its wait queue is the set of tasks `Blocked`
  on it (scanned). `send`/`recv` rendezvous: deliver-and-wake a waiting
  peer, or `block_current()` until one arrives.
- `TaskState::Blocked` joins Ready/Running/Exited; `block_current` mirrors
  `yield_now` (+ the satp swap). The message is register-only (badge + 3
  words) — no memory copy, so `switch_context`/trap asm stay unchanged.

## The key idea
Blocking inside a syscall = park the task (switch away) and, when the peer
delivers, write the message into the parked task's saved trap frame and let
the normal trap return (`sret`) hand it back in registers. The idle task is
always Ready, so the system never fully deadlocks.

## Proof (smoke test)
server recv's and blocks; client sends 0x42; the kernel delivers it across
address spaces; the server exits with code 66 (= 0x42). A rogue without the
endpoint capability is rejected and exits 7. Phase 3b (security spine) is
complete; next is 3c (a post-quantum-crypto primitive).
