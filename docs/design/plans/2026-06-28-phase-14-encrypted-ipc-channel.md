# Phase 14 — Encrypted IPC channel — Implementation Plan

> **For agentic workers:** Implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Use the ML-KEM shared secret to key a ChaCha20-Poly1305 channel; two components exchange an AEAD-encrypted IPC message; a tampered ciphertext or a caller without a `Session` capability fails.

**Architecture:** A kernel session service: the kernel holds an ML-KEM-derived session key (lazily established from the entropy pool on first use), and offers capability-gated `seal`/`open` syscalls. A small (8-byte) message + 16-byte tag + nonce counter fits in IPC registers.

**Tech Stack:** Rust `no_std` kernel, pure host-tested `kernel-crypto` (now with `chacha20poly1305`), QEMU riscv64, PowerShell two-boot harness.

**Spec:** `docs/design/specs/2026-06-27-phase-14-encrypted-ipc-channel-design.md`

**Spike result (already done):** `chacha20poly1305 = { version = "0.10.1", default-features = false }` builds `no_std`/no-alloc on `riscv64gc-unknown-none-elf`; the in-place detached `seal`/`open` round-trip + tamper + wrong-key were host-verified. The functions are already in `libs/crypto/src/lib.rs` and the dep in its `Cargo.toml` (from the spike).

## Global Constraints

- **Commits:** Conventional Commits, NO AI co-author; author Kathir (signing automated).
- **Audited crypto only** (ADR 0004) — no hand-rolled crypto; **commit the updated `Cargo.lock`** (new deps: `chacha20poly1305`, `chacha20`).
- **`kernel-crypto` is `#![cfg_attr(not(test), no_std)]`, no-alloc.** Host tests: `cargo test -p kernel-crypto`. Arch host tests: `cargo test -p kernel-arch-riscv64`. Build: `./tools/build.ps1`. Boot test: `./tools/test-qemu.ps1`.
- **U-mode task code** uses the `sys_*` wrappers (inline asm); no calls into kernel `.text`/`.rodata`.
- The crypto runs in the **kernel** (kernel session service; threat-model caveat in the spec).

---

## File Structure

- `libs/crypto/src/lib.rs` — `seal`/`open` (done in spike) + proper host tests.
- `arch/riscv64/src/cap.rs` — `Capability::Session` + `has_session` + test.
- `arch/riscv64/src/channel.rs` — **new**: the session key, nonce counter, lazy `ensure_key`, `seal_word`/`open_word`.
- `arch/riscv64/src/lib.rs` — `pub mod channel;`.
- `arch/riscv64/src/syscall.rs` — `Syscall::Seal`/`Open` (a7 = 13/14), decode + dispatch + tests.
- `arch/riscv64/src/sched.rs` — `seal`/`open` syscall handlers (gate on `has_session`, call `channel`).
- `kernel/src/main.rs` — `sys_seal`/`sys_open`/`sys_recv4` wrappers, `CHAN_EP`/cap/plaintext constants, `sealer`/`opener`/`nocap` tasks, boot wiring, stacks, `MAX_TASKS` 22→25.
- `tools/test-qemu.ps1` — encrypted-channel assertions.
- `docs/learning/0032-encrypted-ipc-channel.md`, `docs/learning/README.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md` — docs.

---

## Task 1: `kernel-crypto` seal/open host tests (+ commit the spike)

**Files:**
- Modify: `libs/crypto/src/lib.rs` (tests; the `seal`/`open` impl + dep already exist from the spike)

**Interfaces:** `seal(&[u8;32], &[u8;12], &mut [u8]) -> [u8;16]`; `open(&[u8;32], &[u8;12], &mut [u8], &[u8;16]) -> bool`.

- [ ] **Step 1: Write the tests** (in the `libs/crypto/src/lib.rs` tests module)

```rust
    #[test]
    fn seal_open_round_trips() {
        let key = [7u8; 32];
        let nonce = [3u8; 12];
        let mut buf = *b"hi there";
        let tag = seal(&key, &nonce, &mut buf);
        assert_ne!(&buf, b"hi there", "ciphertext differs from plaintext");
        assert!(open(&key, &nonce, &mut buf, &tag));
        assert_eq!(&buf, b"hi there", "plaintext recovered");
    }

    #[test]
    fn open_rejects_a_tampered_ciphertext() {
        let key = [7u8; 32];
        let nonce = [3u8; 12];
        let mut buf = *b"hi there";
        let tag = seal(&key, &nonce, &mut buf);
        buf[0] ^= 1;
        assert!(!open(&key, &nonce, &mut buf, &tag), "a flipped byte fails the tag");
    }

    #[test]
    fn open_rejects_a_wrong_key() {
        let nonce = [3u8; 12];
        let mut buf = *b"hi there";
        let tag = seal(&[7u8; 32], &nonce, &mut buf);
        assert!(!open(&[8u8; 32], &nonce, &mut buf, &tag), "wrong key fails the tag");
    }
```

- [ ] **Step 2: Run**

Run: `cargo test -p kernel-crypto seal` and `... open`
Expected: PASS.

- [ ] **Step 3: Commit** (the dep + impl from the spike, plus the tests, plus `Cargo.lock`)

```bash
git add libs/crypto/src/lib.rs libs/crypto/Cargo.toml Cargo.lock
git commit -m "feat(crypto): ChaCha20-Poly1305 seal/open (in-place AEAD), host-tested"
```

---

## Task 2: `Capability::Session` + `has_session`

**Files:**
- Modify: `arch/riscv64/src/cap.rs`

**Interfaces:** `Capability::Session`; `pub fn has_session(caps: &[Option<Capability>], idx: usize) -> bool`.

- [ ] **Step 1: Write the failing test** (in `cap.rs` tests)

```rust
    #[test]
    fn has_session_checks_the_slot() {
        let caps = [None, Some(Capability::Session), Some(Capability::Endpoint(0))];
        assert!(has_session(&caps, 1));
        assert!(!has_session(&caps, 2), "an Endpoint cap is not a Session cap");
        assert!(!has_session(&caps, 0), "empty slot");
        assert!(!has_session(&caps, 9), "out of range");
    }
```

- [ ] **Step 2: Run to verify it fails**

Run: `cargo test -p kernel-arch-riscv64 has_session`
Expected: FAIL — no `Session` variant.

- [ ] **Step 3: Implement** — add the variant to `enum Capability` (after `Reply`):

```rust
    /// Authority to `seal`/`open` on the kernel's encrypted channel session.
    Session,
```

and the check (near `has_randomness`):

```rust
/// True iff capability `idx` is a `Session` capability (authority to use the
/// encrypted channel's `seal`/`open`). `false` for an empty/out-of-range slot or
/// the wrong type.
pub fn has_session(caps: &[Option<Capability>], idx: usize) -> bool {
    matches!(caps.get(idx), Some(Some(Capability::Session)))
}
```

- [ ] **Step 4: Run to verify it passes**

Run: `cargo test -p kernel-arch-riscv64 has_session`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/cap.rs
git commit -m "feat(cap): Session capability gating the encrypted channel"
```

---

## Task 3: `channel` module — session key + seal/open words

**Files:**
- Create: `arch/riscv64/src/channel.rs`
- Modify: `arch/riscv64/src/lib.rs` (`pub mod channel;`)

**Interfaces:**
- `pub fn seal_word(plain: [u8; 8]) -> ([u8; 8], [u8; 16], u64)` — `(ciphertext, tag, nonce)`.
- `pub fn open_word(ct: [u8; 8], tag: [u8; 16], nonce: u64) -> Option<[u8; 8]>`.

- [ ] **Step 1: Register the module** in `arch/riscv64/src/lib.rs` (after `pub mod entropy;`):

```rust
pub mod channel;
```

- [ ] **Step 2: Create `arch/riscv64/src/channel.rs`**

```rust
//! The encrypted IPC channel (Phase 14): the kernel holds one ChaCha20-Poly1305
//! session keyed by an ML-KEM shared secret, and seals/opens small messages for
//! capability-gated components. A kernel session service — the kernel sees
//! plaintext (it is the TCB); the value is making the post-quantum secret real
//! and establishing the seal/open pattern.

/// The 32-byte session key, lazily established (ML-KEM, pool-seeded) on first use.
static mut SESSION_KEY: Option<[u8; 32]> = None;
/// Per-session nonce counter (never reused under the key).
static mut NONCE: u64 = 0;

/// Build a 12-byte ChaCha20-Poly1305 nonce from the session counter.
fn nonce_bytes(counter: u64) -> [u8; 12] {
    let mut n = [0u8; 12];
    n[..8].copy_from_slice(&counter.to_le_bytes());
    n
}

/// Return the session key, establishing it on first call: derive an ML-KEM
/// shared secret seeded by the entropy pool. Logged once.
fn ensure_key() -> [u8; 32] {
    // SAFETY: single hart; first caller establishes, then read-only.
    unsafe {
        if let Some(k) = core::ptr::read(core::ptr::addr_of!(SESSION_KEY)) {
            return k;
        }
        let seed = crate::entropy::next_seed();
        let key = kernel_crypto::ml_kem768_agree(seed).unwrap_or([0u8; 32]);
        core::ptr::write(core::ptr::addr_of_mut!(SESSION_KEY), Some(key));
        crate::println!("crypto: channel session established (ML-KEM)");
        key
    }
}

/// Seal an 8-byte plaintext word; returns `(ciphertext, tag, nonce)`.
pub fn seal_word(plain: [u8; 8]) -> ([u8; 8], [u8; 16], u64) {
    let key = ensure_key();
    // SAFETY: single hart; advance the nonce counter.
    let nonce = unsafe {
        let n = core::ptr::read(core::ptr::addr_of!(NONCE));
        core::ptr::write(core::ptr::addr_of_mut!(NONCE), n + 1);
        n
    };
    let mut buf = plain;
    let tag = kernel_crypto::seal(&key, &nonce_bytes(nonce), &mut buf);
    (buf, tag, nonce)
}

/// Open an 8-byte ciphertext word; `Some(plaintext)` iff the tag verifies.
pub fn open_word(ct: [u8; 8], tag: [u8; 16], nonce: u64) -> Option<[u8; 8]> {
    let key = ensure_key();
    let mut buf = ct;
    if kernel_crypto::open(&key, &nonce_bytes(nonce), &mut buf, &tag) {
        Some(buf)
    } else {
        None
    }
}
```

(Confirm the arch crate already deps `kernel-crypto` — it does, via `entropy.rs`.)

- [ ] **Step 3: Build**

Run: `./tools/build.ps1`
Expected: clean build (functions unused until Task 4/5 wire them; `pub` items aren't dead-code-flagged).

- [ ] **Step 4: Commit**

```bash
git add arch/riscv64/src/channel.rs arch/riscv64/src/lib.rs
git commit -m "feat(crypto): channel module — ML-KEM session key + seal_word/open_word"
```

---

## Task 4: `seal`/`open` syscalls (a7 = 13/14)

**Files:**
- Modify: `arch/riscv64/src/syscall.rs` (enum, decode, dispatch, tests)
- Modify: `arch/riscv64/src/sched.rs` (handlers)

**Interfaces:**
- `seal` ABI: `a0` = plaintext word. On success `a0 = 0`, `a1 = ciphertext`, `a2 = tag_lo`, `a3 = tag_hi`, `a4 = nonce`; `a0 = usize::MAX` if the caller lacks `Session`.
- `open` ABI: `a0` = ciphertext, `a1` = tag_lo, `a2` = tag_hi, `a3` = nonce. On success `a0 = 0`, `a1 = plaintext`; `a0 = usize::MAX` on a bad tag or no capability.
- The `Session` cap slot is a fixed convention: `a5` is **not** used — the handler looks for the cap at slot `SESSION_CAP = 0`. (Simpler: the gate is "does the caller hold a `Session` cap at slot 0".)

- [ ] **Step 1: Decode + dispatch + test** — add to `enum Syscall` (after `Revoke`):

```rust
    /// `seal(plaintext)` — AEAD-encrypt a word on the channel (Session-gated).
    Seal,
    /// `open(ciphertext, tag, nonce)` — AEAD-decrypt a word (Session-gated).
    Open,
```

decode arms (after `12 => Syscall::Revoke,`):

```rust
        13 => Syscall::Seal,
        14 => Syscall::Open,
```

dispatch arms (after `Syscall::Revoke`):

```rust
        Syscall::Seal => {
            crate::sched::seal(frame);
            Outcome::Resume
        }
        Syscall::Open => {
            crate::sched::open(frame);
            Outcome::Resume
        }
```

decode test (with the others):

```rust
        assert_eq!(decode_syscall(13), Syscall::Seal);
        assert_eq!(decode_syscall(14), Syscall::Open);
```

- [ ] **Step 2: Handlers in `sched.rs`** (near `getrandom`, which is the gating model)

```rust
/// The `seal` syscall: AEAD-encrypt the plaintext word in `a0` on the channel,
/// gated by a `Session` capability at slot `SESSION_CAP`. On success `a0 = 0`,
/// `a1` = ciphertext, `a2`/`a3` = tag, `a4` = nonce; `a0 = usize::MAX` if the
/// caller lacks the capability.
#[cfg(target_arch = "riscv64")]
pub fn seal(frame: &mut crate::trap::TrapFrame) {
    const SESSION_CAP: usize = 0;
    let ok = SCHED.with(|s| {
        crate::cap::has_session(&s.tasks[s.current].as_ref().unwrap().caps, SESSION_CAP)
    });
    if !ok {
        let name = SCHED.with(|s| s.tasks[s.current].as_ref().unwrap().name);
        crate::println!("crypto: '{name}' seal refused (no Session capability)");
        frame.regs[9] = usize::MAX;
        return;
    }
    let plain = frame.regs[9].to_le_bytes();
    let (ct, tag, nonce) = crate::channel::seal_word(plain);
    frame.regs[9] = 0;
    frame.regs[10] = usize::from_le_bytes(ct);
    frame.regs[11] = usize::from_le_bytes(tag[..8].try_into().unwrap());
    frame.regs[12] = usize::from_le_bytes(tag[8..].try_into().unwrap());
    frame.regs[13] = nonce as usize;
}

/// The `open` syscall: AEAD-decrypt the ciphertext word in `a0` (tag in `a1`/`a2`,
/// nonce in `a3`), gated by a `Session` capability at slot `SESSION_CAP`. On
/// success `a0 = 0`, `a1` = plaintext; `a0 = usize::MAX` on a bad tag or no cap.
#[cfg(target_arch = "riscv64")]
pub fn open(frame: &mut crate::trap::TrapFrame) {
    const SESSION_CAP: usize = 0;
    let ok = SCHED.with(|s| {
        crate::cap::has_session(&s.tasks[s.current].as_ref().unwrap().caps, SESSION_CAP)
    });
    if !ok {
        frame.regs[9] = usize::MAX;
        return;
    }
    let ct = frame.regs[9].to_le_bytes();
    let mut tag = [0u8; 16];
    tag[..8].copy_from_slice(&frame.regs[10].to_le_bytes());
    tag[8..].copy_from_slice(&frame.regs[11].to_le_bytes());
    let nonce = frame.regs[12] as u64;
    match crate::channel::open_word(ct, tag, nonce) {
        Some(plain) => {
            frame.regs[9] = 0;
            frame.regs[10] = usize::from_le_bytes(plain);
        }
        None => frame.regs[9] = usize::MAX,
    }
}
```

(`usize` is 8 bytes on rv64, so a plaintext/ciphertext "word" is `[u8; 8]`. `SESSION_CAP = 0` matches the boot grant slot.)

- [ ] **Step 3: Build**

Run: `./tools/build.ps1`
Expected: clean build.

- [ ] **Step 4: Run the decode test**

Run: `cargo test -p kernel-arch-riscv64 decodes`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add arch/riscv64/src/syscall.rs arch/riscv64/src/sched.rs
git commit -m "feat(crypto): seal/open syscalls (a7=13/14), Session-gated"
```

---

## Task 5: Wrappers + the sealer/opener/nocap demo

**Files:**
- Modify: `kernel/src/main.rs`
- Modify: `arch/riscv64/src/sched.rs` (`MAX_TASKS` 22→25 + initializer)

- [ ] **Step 1: Add the wrappers** (`kernel/src/main.rs`, near `sys_send`)

```rust
    /// seal syscall (a7 = 13): a0 = plaintext word. Returns (status, ciphertext,
    /// tag_lo, tag_hi, nonce); status `usize::MAX` if the caller lacks Session.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability.
    #[inline(always)]
    unsafe fn sys_seal(plain: usize) -> (usize, usize, usize, usize, usize) {
        let status; let ct; let tl; let th; let nonce;
        core::arch::asm!(
            "ecall",
            in("a7") 13usize,
            inout("a0") plain => status,
            out("a1") ct, out("a2") tl, out("a3") th, out("a4") nonce,
            options(nostack),
        );
        (status, ct, tl, th, nonce)
    }

    /// open syscall (a7 = 14): a0 = ciphertext, a1 = tag_lo, a2 = tag_hi, a3 =
    /// nonce. Returns (status, plaintext); status `usize::MAX` on a bad tag/no cap.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and the tag.
    #[inline(always)]
    unsafe fn sys_open(ct: usize, tag_lo: usize, tag_hi: usize, nonce: usize) -> (usize, usize) {
        let status; let plain;
        core::arch::asm!(
            "ecall",
            in("a7") 14usize,
            inout("a0") ct => status,
            inout("a1") tag_lo => plain,
            in("a2") tag_hi, in("a3") nonce,
            options(nostack),
        );
        (status, plain)
    }

    /// recv syscall capturing the 3 data words (a7 = 5): a0 = endpoint cap, a1 =
    /// reply slot. Returns (badge, w0, w1, w2) — the message's badge + data.
    ///
    /// # Safety
    /// Always sound; the kernel validates the capability and may block us.
    #[inline(always)]
    unsafe fn sys_recv4(cap: usize, reply_slot: usize) -> (usize, usize, usize, usize) {
        let badge; let w0; let w1; let w2;
        core::arch::asm!(
            "ecall",
            in("a7") 5usize,
            inout("a0") cap => badge,
            inout("a1") reply_slot => w0,
            out("a2") w1, out("a3") w2,
            options(nostack),
        );
        (badge, w0, w1, w2)
    }
```

(Check whether a `sys_send4(cap, w0, w1, w2, w3)` wrapper already exists — the
entropy component uses a 4-word send. If it does, reuse it; if not, add one
mirroring `sys_send` but with `in("a1") w1, in("a2") w2, in("a3") w3` and
`in("a4") w4`. The sealer sends `{badge = nonce, data = [ct, tag_lo, tag_hi]}`.)

- [ ] **Step 2: Add the channel constants** (near the other EP constants)

```rust
    /// The encrypted-channel endpoint (Phase 14): sealer -> opener.
    const CHAN_EP: usize = 8;
    const CHAN_CAP: usize = 1;        // Endpoint(CHAN_EP) slot (Session is slot 0)
    const CHAN_REPLY_SLOT: usize = 2; // opener's recv reply slot (unused for Send)
    /// The known 8-byte plaintext the sealer encrypts (a recognizable word).
    const CHAN_PLAINTEXT: usize = 0x5345_4352_4554_3231; // "12TERCES" le-ish
    /// opener's exit code when the message verified AND a tamper was rejected.
    const CHAN_OK_CODE: usize = 14;
    /// nocap's exit code when its seal was refused.
    const NOCAP_CODE: usize = 15;
```

- [ ] **Step 3: Add the `sealer`, `opener`, `nocap` tasks** (near `tenant_task`)

```rust
    /// `sealer` (Phase 14): holds a Session cap (slot 0) + Endpoint(CHAN_EP). It
    /// seals a known plaintext and sends {nonce, ciphertext, tag} to the opener.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn sealer_task() -> ! {
        unsafe {
            let (_s, ct, tl, th, nonce) = sys_seal(CHAN_PLAINTEXT);
            let _ = sys_send4(CHAN_CAP, nonce, ct, tl, th);
            sys_exit(0)
        }
    }

    /// `opener` (Phase 14): holds a Session cap (slot 0) + Endpoint(CHAN_EP). It
    /// receives the sealed message, opens it (verifying the plaintext), then
    /// confirms a flipped ciphertext is rejected. Exits CHAN_OK_CODE iff both.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn opener_task() -> ! {
        unsafe {
            let (nonce, ct, tl, th) = sys_recv4(CHAN_CAP, CHAN_REPLY_SLOT);
            let (s, plain) = sys_open(ct, tl, th, nonce);
            let verified = s == 0 && plain == CHAN_PLAINTEXT;
            let (s2, _) = sys_open(ct ^ 1, tl, th, nonce); // tamper
            let tamper_rejected = s2 == usize::MAX;
            if verified && tamper_rejected {
                sys_exit(CHAN_OK_CODE)
            } else {
                sys_exit(99)
            }
        }
    }

    /// `nocap` (Phase 14): holds NO Session cap — its seal is refused.
    #[no_mangle]
    #[link_section = ".user_text"]
    extern "C" fn nocap_task() -> ! {
        unsafe {
            let (s, ..) = sys_seal(CHAN_PLAINTEXT);
            if s == usize::MAX { sys_exit(NOCAP_CODE) } else { sys_exit(98) }
        }
    }
```

- [ ] **Step 4: Spawn the demo** (in `kmain`, after the lease/tenant block). Spawn `opener` before `sealer` so the opener is recv-blocked when the sealer sends:

```rust
        // Phase 14 — encrypted IPC channel. The sealer seals a known message and
        // sends the ciphertext to the opener, which verifies it (and that a
        // tampered ciphertext is rejected). `nocap` proves the Session gate.
        let opu = ustack(core::ptr::addr_of!(US_OPENER) as usize);
        let opener = sched::spawn_user("opener", opener_task, opu.1,
            core::ptr::addr_of!(KS_OPENER) as usize + TASK_STACK,
            mem::build_user_space(opu, NO_DEVICE));
        sched::grant_cap(opener, 0, Capability::Session);
        sched::grant_cap(opener, CHAN_CAP, Capability::Endpoint(CHAN_EP));

        let seu = ustack(core::ptr::addr_of!(US_SEALER) as usize);
        let sealer = sched::spawn_user("sealer", sealer_task, seu.1,
            core::ptr::addr_of!(KS_SEALER) as usize + TASK_STACK,
            mem::build_user_space(seu, NO_DEVICE));
        sched::grant_cap(sealer, 0, Capability::Session);
        sched::grant_cap(sealer, CHAN_CAP, Capability::Endpoint(CHAN_EP));

        let ncu = ustack(core::ptr::addr_of!(US_NOCAP) as usize);
        let nocap = sched::spawn_user("nocap", nocap_task, ncu.1,
            core::ptr::addr_of!(KS_NOCAP) as usize + TASK_STACK,
            mem::build_user_space(ncu, NO_DEVICE));
        // nocap gets NO Session cap.
```

- [ ] **Step 5: Declare the stacks** (`KS_OPENER`/`KS_SEALER`/`KS_NOCAP` + `US_*`, copy the `US_BROKER` form).

- [ ] **Step 6: Raise `MAX_TASKS` 22 → 25** in `sched.rs` (+ three `None` in the `Scheduler::new` initializer).

- [ ] **Step 7: Build + verify `sys_send4`**

Run: `./tools/build.ps1`
Expected: clean build. If `sys_send4` is undefined, add the wrapper (Task 5 Step 1 note).

- [ ] **Step 8: Commit**

```bash
git add kernel/src/main.rs arch/riscv64/src/sched.rs
git commit -m "feat(crypto): sys_seal/open/recv4 wrappers + sealer/opener/nocap demo"
```

---

## Task 6: Encrypted-channel assertions in the boot test

**Files:**
- Modify: `tools/test-qemu.ps1`

**Interfaces:** boot-1 markers `crypto: channel session established (ML-KEM)`, `sched: task 'opener' exited (code 14)`, `crypto: 'nocap' seal refused (no Session capability)`, `sched: task 'nocap' exited (code 15)`.

- [ ] **Step 1: Add the boot-1 assertions** to `$mustMatch1`:

```powershell
    "crypto: channel session established \(ML-KEM\)",
    "sched: task 'opener' exited \(code 14\)",
    "crypto: 'nocap' seal refused \(no Session capability\)",
    "sched: task 'nocap' exited \(code 15\)",
```

- [ ] **Step 2: Update the PASS banner** — append: `; and Phase 14 the encrypted IPC channel: the ML-KEM shared secret keys a ChaCha20-Poly1305 session - a 'sealer' encrypts a message and a 'opener' decrypts and verifies it over IPC (a tampered ciphertext is rejected, and a component without the Session capability is refused) - the post-quantum secret is finally put to work.`

- [ ] **Step 3: Run the full boot test**

Run: `./tools/test-qemu.ps1`
Expected: `BOOT TEST PASS: …; and Phase 14 the encrypted IPC channel: …`.

Debugging aids:
- If `opener` exits 99 → either the open failed (key mismatch — confirm both call `ensure_key` so the key is established once and shared) or the message words were mis-shuffled (check the `sys_send4` arg order vs `sys_recv4`'s returned `(badge, w0, w1, w2)`).
- If `scheduler full` → raise `MAX_TASKS` (Task 5 Step 6); 25 should fit (24 tasks).
- If the established line is missing → `ensure_key` never ran (no seal/open happened); confirm the sealer holds the Session cap at slot 0.

- [ ] **Step 4: Commit**

```bash
git add tools/test-qemu.ps1
git commit -m "test: encrypted IPC channel (ML-KEM + AEAD) demo (Phase 14)"
```

---

## Task 7: Documentation

**Files:**
- Create: `docs/learning/0032-encrypted-ipc-channel.md`
- Modify: `docs/learning/README.md`, `docs/roadmap/roadmap.md`, `docs/glossary.md`

- [ ] **Step 1: Learning note** `docs/learning/0032-encrypted-ipc-channel.md` — short. Cover: what changed (the ML-KEM secret now keys a ChaCha20-Poly1305 AEAD channel; seal/open syscalls; a Session capability gates it); the idea worth keeping (the crypto stack is finally *applied* — entropy → ML-KEM key → AEAD message; AEAD gives confidentiality *and* integrity, so a tampered ciphertext is rejected by the tag); the honest threat model (kernel session service — the kernel sees plaintext; the value is the real crypto + the pattern, not E2E-from-the-kernel, which needs U-mode crypto); the spike (the audited crate builds no_std/no-alloc on the target); what's next (U-mode crypto for true E2E, applying the channel to disk/network). Follow `0010` (PQC) / `0018` (entropy) in style.

- [ ] **Step 2: Index** in `docs/learning/README.md` (`0032` line).

- [ ] **Step 3: Roadmap** — replace `## Phase 14+ — Breadth` with a completed `## Phase 14 — Encrypted IPC channel (done — 2026-06-28)` (goal / you-learn / done-when citing note 0032), and re-add a `## Phase 15+ — Breadth` placeholder.

- [ ] **Step 4: Glossary** — add **AEAD (authenticated encryption)** and **Encrypted channel / session key** near the crypto terms.

- [ ] **Step 5: Cross-reference check**

Run: `./tools/check-references.ps1`
Expected: passes.

- [ ] **Step 6: Commit**

```bash
git add docs/learning/0032-encrypted-ipc-channel.md docs/learning/README.md docs/roadmap/roadmap.md docs/glossary.md
git commit -m "docs: Phase 14 encrypted IPC channel — learning note 0032, roadmap, glossary"
```

---

## Self-Review (completed during planning)

- **Spec coverage:** seal/open + dep → Task 1 (spike-validated); `Session` cap → Task 2; channel/session key → Task 3; seal/open syscalls → Task 4; wrappers + demo → Task 5; proof → Task 6; docs → Task 7. All spec sections map to a task.
- **Type consistency:** `seal(&[u8;32],&[u8;12],&mut[u8])->[u8;16]` / `open(...)->bool` (Tasks 1, 3); `has_session(caps, idx)->bool` (Tasks 2, 4); `channel::seal_word([u8;8])->([u8;8],[u8;16],u64)` / `open_word(...)->Option<[u8;8]>` (Tasks 3, 4); the seal/open syscall register ABI is consistent between the handlers (Task 4) and the wrappers (Task 5); `SESSION_CAP = 0` matches the boot grant slot; `CHAN_EP = 8` is unique (EP0..LEASE_EP=7).
- **Open verification during execution:** confirm `sys_send4` exists (else add it — Task 5 Step 1); confirm `usize`/word packing (`[u8;8]` ↔ `usize` on rv64) round-trips through the ABI (Task 6 debugging); confirm `MAX_TASKS = 25` fits (Task 5 Step 6).
```
