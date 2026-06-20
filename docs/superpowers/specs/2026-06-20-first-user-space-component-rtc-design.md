# Kernel — Design: First user-space component (RTC time server)

- **Date:** 2026-06-20
- **Status:** Draft — awaiting review
- **Scope of this document:** the first realization of
  [ADR 0007](../decisions/0007-extensibility-user-space-components.md) — move
  a real driver *out* of the kernel into an unprivileged, capability-holding
  user-space component reached via IPC. The concrete component is an
  **RTC (real-time clock) time server**. Fully QEMU-testable; no board.

---

## 0. Where this sits

Phases 0–3 built the security spine (U-mode, isolation, capabilities + IPC,
PQC); Phase 4a/4b made the kernel device-tree-driven with its own UART.
[ADR 0007](../decisions/0007-extensibility-user-space-components.md) commits
the project to a **minimal trusted core extended by capability-holding
user-space components**. This is the first such component — the proof that
"a driver/feature is a component, not kernel code."

It is deliberately built almost entirely by **composing** what already
exists: per-address-space isolation (3b-ii), capabilities + synchronous IPC
(3b-iii), and the `print` syscall (3a). It also lays the substrate for the
self-healing organism ([ADR 0005](../decisions/0005-self-healing-knowledge-organism.md)),
which will itself run as such a caged component (Phase 5).

## 1. Goal

An **RTC server** runs as an unprivileged U-mode component that exclusively
owns the goldfish real-time clock: its MMIO page is mapped **only** into that
component's address space. Other components ask it for the time over a
**capability-checked endpoint**; the server reads the device and prints the
value. The kernel itself never touches the RTC — the driver lives entirely
in user space, bounded by what it was granted.

**You learn (kept brief):** that a device driver can be an ordinary
unprivileged task whose only "privilege" is a device's MMIO mapped into its
address space and an IPC endpoint to receive requests on — and that
isolation (only this component sees the device) plus capabilities (only
endpoint-holders can call it) together bound its authority, so adding such a
component never grows or endangers the trusted core.

**Done when** `./tools/test-qemu.ps1` observes, alongside every existing
milestone (2a/2b, `console: ns16550a …`, `dt: 192 MiB RAM`,
`pqc: ML-KEM-768 …`, `tick: 2`):

1. **A user-space driver served a real device over IPC** — a line
   `rtc: 0x<hex>` printed by the RTC server, where the value is the goldfish
   clock it read after a client's capability-checked request (a large,
   non-zero nanosecond count — proof it read live hardware, not a constant).
2. **Capability enforcement** — a `rogue` component that lacks the endpoint
   capability is refused (`ipc: 'rogue' send rejected (no capability)`).

And off the bare target:

3. **Host unit test** — `parse()` on the committed real QEMU DTB returns the
   RTC base (in addition to the existing RAM/timebase/UART asserts).

## 2. Non-goals (deferred)

- **Call/reply (RPC) IPC** — the server *prints* the value rather than
  replying to the client, so one-way `send`/`recv` suffices. A
  reply-carrying IPC (and a reply capability) is future work.
- **Byte-buffer messages** — unchanged from 3b-iii (register-only); the
  server formats and prints locally, so no buffer transfer is needed.
- **A first-class, transferable Device capability token** — here a
  component's device authority is *granted at boot by mapping the device's
  MMIO into that component's address space* (analogous to `grant_cap` for
  endpoints). A real `Capability::Device` that can be checked and delegated
  ties to capability delegation (deferred since 3b-iii) and is future work.
- **Writing the RTC / alarms** — read-only access (least authority).
- **Other devices / virtio / a driver framework** — one device, by hand,
  first. virtio-rng (real crypto entropy) and others come once the model is
  established.
- **Moving the console out of the kernel** — the kernel needs its own
  console for boot/panic/trap diagnostics; that stays in-kernel.
- **Dynamic, page-walk pointer validation** for `print` — the static
  `.user_data` bounds guard (3a) is unchanged; the server prints its own
  stack buffer, which lies in `.user_data`.

## 3. Design

### 3.1 Components

| Component | Location | Responsibility |
|-----------|----------|----------------|
| RTC discovery | `arch/riscv64/src/dt.rs` | Extend `MachineInfo` with `rtc_base: usize`; find the `goldfish`-compatible node's `reg` base (the existing `compatible`-match-committed-at-`END_NODE` path, generalized to a second device). Host-tested. |
| Per-component device mapping | `arch/riscv64/src/mem/mod.rs` | Rename `build_user_space`'s second region from `data` to `device` (semantics: an R-U page mapped into *this component only*). **No mechanism change** — the 3b-ii per-component R-U mapping already does exactly this; we pass a device MMIO page through it. |
| `rtc_server` | `kernel/src/main.rs` (`.user_text`) | The driver-as-component (§3.3). |
| `rtc_client`, `rogue` | `kernel/src/main.rs` (`.user_text`) | A client that requests the time; a rogue that lacks the cap (enforcement proof). |
| Boot wiring | `kernel/src/main.rs` | Discover `rtc_base`; spawn the server (RTC mapped R-U into it + endpoint cap), client (endpoint cap), rogue (none), idle. Replaces the 3b-iii toy server/client/rogue triad. |

### 3.2 Granting a component a device (no new mechanism)

`build_user_space(stack, device)` builds a U-mode task's address space:
kernel sections (global) + shared `.user_text` (R-X-U) + the task's stack
(RW-U) + one extra **R-U page** mapped *only* in this tree. 3b-ii used that
extra page for a read-only data page; here we pass the **RTC's MMIO page**
(`rtc_base`, one 4 KiB page) as that R-U region. Because per-task trees are
private (3b-ii), the device is visible to the server and **no one else** —
that exclusivity *is* the device "capability." The kernel decides which
component gets which device by choosing what to map at spawn (just as it
chooses which endpoint caps to `grant_cap`). Read-only (R-U, no W/X) is
least authority: the server can read the clock, not reprogram it.

### 3.3 The RTC server

A U-mode component in `.user_text` that owns the goldfish RTC. The goldfish
RTC exposes `TIME_LOW` at offset 0 and `TIME_HIGH` at offset 4; reading
`TIME_LOW` latches the high word, so the sequence is: read `TIME_LOW`, read
`TIME_HIGH`, combine to a `u64` nanosecond count. The server loops:

```
loop:
    recv(endpoint)                  # block until a client requests
    low  = read_volatile(rtc_base + 0)
    high = read_volatile(rtc_base + 4)
    let t = ((high as u64) << 32) | low as u64
    format "rtc: 0x{t:016x}\n" into a stack buffer
    sys_print(buf, len)             # print the value (no reply IPC needed)
```

Formatting is a small hand-rolled `u64`→hex loop into a stack array (no
`fmt` in U-mode). `sys_print`'s pointer is the stack buffer, which is in
`.user_data` (user stacks live there), so the static confused-deputy guard
accepts it and the SUM-window copy reads it. The server never returns; after
serving it blocks on `recv` again.

### 3.4 The client, the rogue, and boot wiring

`rtc_client` holds the endpoint capability and `sys_send`s one "report"
request (badge value is a fixed command; the server treats any request as
"report the time"), then `exit`s. `rogue` holds **no** endpoint cap; its
`sys_send` fails the capability check and is logged/refused (it then exits) —
re-proving enforcement against the real server's endpoint.

`kmain`, after the existing setup (DTB parse, console switch, `mem::init`,
`pqc_demo`), discovers `rtc_base` (from the same `MachineInfo`) and spawns:
the RTC server first (so it `recv`s and blocks before the client sends),
with `build_user_space(server_stack, (rtc_base, rtc_base + PAGE))` and an
endpoint cap; then the client (endpoint cap) and rogue (none); then idle.
Spawn order puts the server in slot 0. The 3b-iii toy `server`/`client`/
`rogue` demo is replaced by this real component (which covers the same
IPC + enforcement ground, now with an actual driver).

### 3.5 Error handling summary

| Failure | Behavior |
|---------|----------|
| `rogue` sends without the endpoint cap | `ipc: 'rogue' send rejected (no capability)`; rogue exits (3b-iii path, unchanged). |
| RTC node absent from the device tree | `parse`/`from_ptr` panic (QEMU always provides it; a real board without goldfish-RTC is a 4c concern). |
| Server reads the device | Read-only loads to a page mapped R-U in its own AS; cannot fault for other tasks (they don't map it). |
| `print` bounds | Static `.user_data` guard, unchanged; the server prints its own stack buffer. |
| Any other task touches the RTC address | Not mapped in its AS → U-mode fault → contained (3b-i/ii path). |

## 4. Testing

- **Host unit test** (`arch/riscv64`): extend the DTB test — `parse(fixture)`
  also returns the RTC base (the goldfish `rtc@…` node's `reg`); the existing
  RAM/timebase/UART asserts and the bad-magic/truncated cases stay.
- **QEMU smoke test** (`tools/test-qemu.ps1`, extended; written first,
  failing): add `rtc: 0x` (with several hex digits, e.g. `rtc: 0x[0-9a-f]{8,}`)
  and the `rogue` rejection line; drop the replaced 3b-iii toy
  `server`/`client`/`rogue` lines; keep every other milestone.

## 5. Deliverables

1. `dt.rs`: `MachineInfo.rtc_base` + goldfish-RTC discovery; host test updated.
2. `mem`: `build_user_space`'s second region renamed `data → device`
   (R-U, per-component), doc updated; no behavior change.
3. `kmain`: the RTC server, client, and rogue components; boot wiring that
   maps the RTC into the server only and grants the endpoint caps; the toy
   3b-iii triad removed.
4. Extended QEMU smoke test + host unit test, all green.
5. Short learning note `docs/learning/0013-first-user-space-component.md`.
6. Roadmap: a new entry for the user-space-component thread (realizing
   ADR 0007), this milestone marked done.
7. Glossary: user-space driver / server component, device capability
   (granted by mapping) — only genuinely new terms.

## 6. Open questions (for later)

- **Call/reply IPC + reply capabilities:** so a server can return a value to
  the caller (the RTC server would *reply* with the time instead of printing
  it) — the basis for real RPC services.
- **A first-class Device capability:** a checkable, delegable token for
  device authority, replacing "granted by mapping at boot."
- **A component/manifest format and a driver framework:** once a second and
  third component exist, factor the common shape (endpoint loop, device
  mapping) — designed then, not now (YAGNI).
- **virtio-rng as the next component:** real entropy for crypto (retiring
  Phase 3c's fixed seed) — the security-differentiator payoff.
- **The self-healer as a component (Phase 5):** the knowledge organism built
  on this exact pattern — a caged, capability-holding user-space component.
