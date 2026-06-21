# 0017 — call/reply IPC: a server that answers

**One-line:** a client can now `call` a server and get a value back — built
from the one-way rendezvous, with the kernel binding each reply to its caller.

## What changed
- New syscalls: `call(ep, request)` (atomically send + block for the reply)
  and `reply(response)` (answer the recorded caller).
- New wait states: `IpcRole::Call` (a caller queued at an endpoint) and
  `TaskState::AwaitingReply` (a caller whose request was picked up, waiting for
  the answer). The server carries a `caller` back-pointer.
- The RTC component is now a real server: `loop { recv; read clock; reply(t) }`.
  The client `call`s it and exits with the returned time — the value crosses
  *back* to the caller, instead of the server reporting via its exit code.

## The ideas worth keeping
1. **`call` is "send + await reply" as one atomic step.** Splitting it into a
   separate send then recv would race — the reply could arrive before the recv.
   One syscall blocks the caller in `AwaitingReply` the moment it sends.
2. **Caller-tracking instead of reply capabilities.** Because we are
   single-hart and servers handle one call at a time, the kernel just records
   `caller` on the server when it receives a Call; `reply` wakes exactly that
   task. A server can only ever answer whoever just called it (secure), with no
   new capability type. seL4's one-shot reply caps buy generality (deferred /
   forwarded / out-of-order replies) we don't need yet.
3. **Two wait phases, two states.** A caller first queues at the endpoint
   (`Blocked{ep, Call}`); once a server picks it up it moves to
   `AwaitingReply` so no second server can re-match it. Only the server's
   `reply` makes it `Ready`.

## Proof
`ipc: 'rtc' blocks on recv` → the client `call`s → `sched: task 'client'
exited (code <nanoseconds>)`: the live clock value returned from the server to
its caller. The server keeps looping, ready for the next request.
