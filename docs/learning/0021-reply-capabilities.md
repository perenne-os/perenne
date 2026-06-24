# 0021 — One-shot reply capabilities

**One-line:** a server can now hold several calls at once and answer them in
any order — each received call mints a one-shot capability to reply to it.

## What changed
- New `Capability::Reply(caller)` + `reply_caller` lookup.
- `recv(ep, reply_slot)` mints a `Reply` cap into the server's chosen cap slot
  when the message is a Call; `reply(reply_slot, msg)` wakes the named caller
  and consumes the cap (one-shot).
- Before, the kernel tracked a single `caller` per server, so a server had to
  reply before its next receive. `caller` now survives only as a transient (it
  carries the caller slot from `call` to the server's `recv` continuation); the
  durable binding is the reply cap — one per outstanding call.

## The ideas worth keeping
1. **One outstanding reply = one capability.** Tracking "who do I owe a reply"
   as a *field* allows exactly one; as a *capability per call*, a server can
   hold many and reply out of order — the deferrer holds a reply cap in slot 1
   and slot 2, then replies to slot 2 first.
2. **Mint on receive, consume on use.** The kernel creates the reply cap when
   the call arrives and clears it when used — so a reply cap is one-shot and a
   double-reply is simply a missing cap.
3. **No staleness guard needed in a blocking kernel.** A caller is
   `AwaitingReply` (blocked — it can't run, exit, or be reused) until replied,
   and the cap is consumed on use, so a `Reply` cap always names a valid waiting
   caller. (A reaping kernel would need a generation.)

## What this does *not* do yet
Forwarding a reply cap to *another* component needs capability delegation
through IPC (moving caps between cap tables) — a separate, larger feature. So a
reply cap stays in the server that minted it.

## Proof
`dclientA` and `dclientB` both call the `deferrer`; it receives both before
replying, then answers B then A. They exit `417` (0x1a1) and `433` (0x1b1) —
distinct values, delivered out of order.
