# Contributing

This is an early-stage, solo, open-source project built deliberately and securely. Contributions and curiosity are welcome — please read the [vision](docs/vision/north-star.md) and [principles](docs/vision/principles.md) first, since they guide every decision.

## Setup

See [`docs/learning/0001-dev-environment.md`](docs/learning/0001-dev-environment.md) for the full story. In short:

- Install [Rust via rustup](https://rustup.rs) — the pinned toolchain installs automatically from `rust-toolchain.toml`.
- Install [QEMU](https://www.qemu.org) and ensure `qemu-system-riscv64` is on your PATH.
- On Windows: install the MSVC toolchain **with the Windows SDK** (needed to link host test binaries).

## Build and run

```powershell
./tools/build.ps1     # builds the workspace + tests, then cross-builds the kernel
./tools/run-qemu.ps1  # boots OUR kernel under QEMU; exit with Ctrl-A then X
./tools/test-qemu.ps1 # non-interactive boot smoke test (exit code 0 = pass)
```

Every change must keep the **build green** (`./tools/build.ps1` passes), keep the **kernel booting** (`./tools/test-qemu.ps1` passes), and keep the docs accurate.

## Commits

- Use [Conventional Commits](https://www.conventionalcommits.org) (`feat:`, `fix:`, `docs:`, `chore:`, …).
- Commits are **signed**. No `Co-Authored-By` trailers.
- Keep commits small and focused; commit frequently.

## Decisions

Any significant decision gets an **Architecture Decision Record** in [`docs/decisions/`](docs/decisions/) — context, decision, consequences. Don't rewrite old ADRs; supersede them with a new one.

## Diagnosability

When you hit and solve a real problem with the toolchain or the system, consider recording it in [`knowledge-base/`](knowledge-base/) using the [issue-record schema](knowledge-base/schema/issue-record.md). Capturing problems is core to this project's purpose, not a chore.
