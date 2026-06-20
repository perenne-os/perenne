# 0013 — The first user-space component: an RTC driver (ADR 0007)

**One-line:** a real driver now lives *outside* the kernel — an unprivileged
component that owns the clock and is reached over capability-checked IPC.

## What changed
- `dt::parse` also discovers the goldfish RTC base. A new U-mode `rtc_server`
  component has that device's MMIO mapped R-U into *its* address space only
  (the per-component mapping from 3b-ii — `build_user_space`'s second region,
  renamed `data → device`).
- A `client` (holding the endpoint capability) requests the time; the server
  reads the clock and reports the value; a `rogue` (no capability) is refused.
  The kernel never touches the RTC.

## The point (ADR 0007)
A driver is just a task whose only authority is (a) a device's MMIO mapped
into its address space and (b) an IPC endpoint to receive requests on.
Isolation means only this component sees the device; capabilities mean only
endpoint-holders can call it. So adding a driver/feature neither grows nor
endangers the trusted core — it shrinks it. This is the substrate the
self-healer (Phase 5) will reuse.

## Built almost entirely by composition
No real new kernel mechanism: 3b-ii isolation maps the device into one
component, 3b-iii capabilities/IPC gate the calls, and the value comes back
through the existing `exit` path. The "device capability" is, for now, simply
the boot-time mapping.

## The gotcha that shaped the code (worth remembering)
A U-mode task must not reach into the kernel: that means **no `.rodata`
reads** (kernel `.rodata` isn't U-accessible) and — subtly — **no calls to
compiler/`core` routines that live in kernel `.text`**. In a *debug* build
only `#[inline(always)]` is guaranteed inlined, so `core::ptr::read_volatile`
(`#[inline]`), `ptr::write`, and the `memset` a `[0u8; N]` emits all become
*calls* into kernel `.text` → `InstructionPageFault` when the task fetches
them. So the server reads the RTC with **inline asm** and avoids any buffer
formatting by **reporting the clock as its exit code**, which the *kernel*
formats and prints. (We watched the kernel cleanly *contain* the faulting
server while everything else kept running before getting this right — the
isolation works.)

## Proof
`ipc: 'rtc' blocks on recv`, the client delivers a request, then
`sched: task 'rtc' exited (code <a large nanosecond count>)` — the live clock
the server read on a capability-checked request — and the `rogue` refused.
All in QEMU, no board.
