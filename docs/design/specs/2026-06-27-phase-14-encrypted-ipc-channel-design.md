# Phase 14 — Encrypted IPC channel: putting the post-quantum secret to work (design)

**Status:** approved 2026-06-27 (user authorized completing the phase end-to-end)
**Priority served:** #1 (security). Closes the unused-crypto loose end: ML-KEM
(3c) produces a shared secret that, until now, is only ever computed in a
round-trip demo and never applied.

## The gap

Phase 3c integrated ML-KEM and the kernel entropy pool seeds it with real
hardware entropy — but the 32-byte shared secret it agrees is thrown away (a
demo). Phase 14 uses it: the secret keys a **ChaCha20-Poly1305 AEAD** session,
and two isolated components exchange an **encrypted IPC message** through it. One
seals a message under the session key; the kernel relays the ciphertext; the
other opens it (the AEAD tag verifying integrity). A tampered ciphertext or a
caller lacking authority fails.

## Implementation revision (2026-06-28, during build)

Two design details changed when they met reality; the shipped code + learning
note 0032 are authoritative. (1) **The session key is established once at boot in
`kmain` (on the 512 KiB boot stack), not lazily in the `seal`/`open` syscall
path** — ML-KEM keygen is far too stack-hungry to run on a task's 16 KiB trap
stack (it overflowed and corrupted the return address into a fatal jump). The
syscalls then only run the light AEAD. (2) Consequently the key is derived from a
**fixed seed** at boot rather than the entropy pool (the pool isn't seeded until
the entropy component runs, post-boot); **pool-seeding the channel key is
deferred** (it needs a large-stack establishment after the pool is ready). The
demo plaintext is also kept ≤ 32 bits so U-mode materializes it inline rather
than via a `.rodata` constant-pool load (the recurring U-mode codegen rule).

## Honest threat model (read this first)

The crypto runs in the **kernel** — U-mode components cannot run it (the
debug-codegen constraint: U-mode must not call kernel `.text`/`.rodata`). So this
is a **kernel session service**: the kernel, already the Trusted Computing Base,
sees plaintext. The value is (a) making the crypto stack *real* end to end
(entropy → ML-KEM session key → AEAD message), and (b) establishing the
`seal`/`open` + session pattern as the foundation for future **at-rest** (disk)
and **on-the-wire** (network) encryption. It is **not** end-to-end secrecy *from*
the kernel — that requires U-mode crypto (a much larger effort), deferred.

## Scope (YAGNI)

- One kernel-held session, keyed by one pool-seeded ML-KEM agreement.
- One AEAD message exchanged over IPC between two components.
- `seal`/`open` are **capability-gated** by a `Session` capability (consistent
  with `Randomness` gating `getrandom`).
- Deferred: U-mode (end-to-end) crypto; a real two-party ML-KEM handshake (vs the
  existing self-contained `ml_kem768_agree`); multi-message nonce/rekey
  management; applying the secret to disk or network.

## Architecture & components

### Pure crypto (`libs/crypto`, host-tested)

- Add the audited **`chacha20poly1305`** crate (RustCrypto, `no_std`, no-alloc
  via the in-place *detached* AEAD API) — an audited dependency per
  [ADR 0004](../decisions/0004-post-quantum-crypto.md); commit `Cargo.lock`.
  **Integration is the spike-worthy risk** (like 3c): verify a `no_std`/no-alloc
  build on the `riscv64gc-unknown-none-elf` target *before* writing the plan.
- `seal(key: &[u8; 32], nonce: &[u8; 12], buf: &mut [u8]) -> [u8; 16]` —
  encrypt `buf` in place, return the 16-byte tag.
- `open(key: &[u8; 32], nonce: &[u8; 12], buf: &mut [u8], tag: &[u8; 16]) -> bool`
  — decrypt `buf` in place iff the tag verifies; `false` (and `buf` left
  undefined/unused) on a bad tag.
- The session key is `ml_kem768_agree(seed)` (already present) seeded by
  `EntropyPool::next_seed()`.
- Host tests: `seal` then `open` round-trips the plaintext; a flipped ciphertext
  byte makes `open` return `false`; a wrong key makes `open` return `false`.

### Kernel (`arch/riscv64`, `kernel/src/main.rs`)

- At boot: derive the channel **session key** = `ml_kem768_agree(pool_next_seed())`,
  stored in a kernel static; a per-session **nonce counter** (starts at 0).
- **`Capability::Session`** — gates the crypto syscalls (pure `has_session`
  check, mirroring `has_randomness`).
- **`seal` syscall** (a7 = 13), capability-gated: input an 8-byte plaintext word;
  output `(ciphertext_word, tag_lo, tag_hi, nonce)` in registers. Increments the
  nonce counter.
- **`open` syscall** (a7 = 14), capability-gated: input `(ciphertext_word, tag_lo,
  tag_hi, nonce)`; output the plaintext word and a status (0 = tag ok,
  `usize::MAX` = bad tag or no capability).

### Demo cast (`kernel/src/main.rs`)

Two U-mode components on a shared endpoint `CHAN_EP`:

- **`sealer`** (holds `Session` + `Endpoint(CHAN_EP)`): `seal`s a known 8-byte
  plaintext, then `send`s `{badge = nonce, data = [ciphertext, tag_lo, tag_hi]}`
  to `opener`.
- **`opener`** (holds `Session` + `Endpoint(CHAN_EP)`): `recv`s the message,
  `open`s it (nonce from the badge), verifies the plaintext equals the known
  value, and exits with a proof code. It then demonstrates a negative: `open` of
  a **flipped** ciphertext returns the bad-tag status.
- A `Session`-less component's `seal` is refused (the authorization guard).

The small message (8-byte plaintext → 8-byte ciphertext + 16-byte tag + a nonce
counter) fits in IPC registers (`badge` + `data[3]`), so no shared buffer is
needed.

## Data flow (the proof)

`crypto: channel session established (ML-KEM, pool-seeded)` → `sealer` seals →
ciphertext crosses IPC → `opener` opens, the tag verifies, plaintext matches →
`sched: task 'opener' exited (code <proof>)`. Then
`crypto: tampered ciphertext rejected (tag mismatch)` and a no-capability
refusal line.

## Error handling

| Situation | Behavior |
|---|---|
| `seal`/`open` without a `Session` capability | `usize::MAX`, logged (authorization guard). |
| `open` with a bad tag (tamper / wrong key) | bad-tag status, no plaintext released. |
| Nonce | a per-session counter, transmitted with the message, never reused under the key (single message here; multi-message nonce/rekey deferred). |

## Testing

- Host (`libs/crypto`): `seal`/`open` round-trip; flipped-ciphertext `open`
  fails; wrong-key `open` fails.
- Boot test: assert the session-established line, the `opener` verified-decrypt
  exit code, the tampered-ciphertext-rejected line, and the no-capability
  refusal.

## What this proves / what's next

The post-quantum secret is finally *used*: a real entropy → ML-KEM → AEAD channel
carries an encrypted message between isolated components, capability-gated. The
`seal`/`open` + session primitives are the foundation for at-rest (disk) and
on-the-wire (network) encryption. Deferred: U-mode end-to-end crypto (remove the
kernel from the confidentiality TCB), a two-party ML-KEM handshake, multi-message
nonce/rekey, and applying the channel to storage or networking.
