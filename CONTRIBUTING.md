# Contributing to Perenne

Thanks for your interest in Perenne — a from-scratch, security-first, self-healing
microkernel in Rust. This guide covers how to set up, how we work, and the bar a
change should meet. It's short on ceremony and long on "here's exactly how."

> **Philosophy first.** Perenne grows in **small, finishable steps**, and
> **correctness and security come before speed and features** (see
> [`docs/vision/`](docs/vision/)). A tiny, well-tested, well-explained change is
> exactly what we want. A big, clever, undocumented one is not.

## Ground rules (the short version)

1. **Every non-trivial change starts with a design, not code.** Write a short
   spec, then a plan, *then* build. See [The workflow](#the-workflow).
2. **Pure logic is host-tested; the system is boot-tested.** Both must be green.
3. **Explain the *why*.** Durable reasoning (ADRs, specs, learning notes) is as
   valued as the code.
4. **Match the surrounding code.** Its naming, comment density, and idioms are the
   house style.

## Development setup

**Prerequisites**
- [Rust](https://rustup.rs) — the pinned toolchain installs automatically from
  `rust-toolchain.toml`.
- [QEMU](https://www.qemu.org) with `qemu-system-riscv64` on your `PATH`.
- **Windows:** the MSVC toolchain **with the Windows SDK** (needed to link the host
  test binaries). Linux/macOS work too; the helper scripts are PowerShell
  (`pwsh` is cross-platform). See [`docs/learning/0001-dev-environment.md`](docs/learning/0001-dev-environment.md).

**The commands you'll use**
```powershell
./tools/build.ps1              # host build + unit tests, then the riscv64 cross-build
./tools/run-qemu.ps1           # boot Perenne interactively (exit QEMU: Ctrl-A then X)
./tools/test-qemu.ps1          # non-interactive boot smoke test (exit 0 = pass)
./tools/check-references.ps1   # verify doc cross-references resolve
```

## The workflow

Perenne is built in **phases**, each a single coherent change developed in one
loop. Follow the same loop for any non-trivial contribution — this is
**tool-agnostic** (do it by hand or with an AI assistant; the format is just good
hygiene):

```
idea → spec → plan → build (TDD) → learning note → PR
       │      │       │
       │      │       └─ code + tests, small commits
       │      └─ docs/design/plans/YYYY-MM-DD-<name>.md   (bite-sized, testable tasks)
       └─ docs/design/specs/YYYY-MM-DD-<name>-design.md   (what & why, approach, scope)
```

1. **Spec** (`docs/design/specs/`): the problem, the approach you chose *and the
   alternatives*, the architecture, error handling, testing, and **what's out of
   scope**. Agree on this before writing code. (For anything user-facing or with a
   real design fork, open an issue/discussion first.)
2. **Plan** (`docs/design/plans/`): the implementation broken into small, testable
   tasks — each with its test, the minimal code, and a commit.
3. **Build**: work the plan task-by-task, **test-first** where there's pure logic
   (see below). Commit frequently.
4. **Learning note** (`docs/learning/`): a short, honest "what was non-obvious"
   after it works.

See [`docs/design/README.md`](docs/design/README.md) for the full shape, and any
recent phase under `docs/design/` for a concrete worked example.

## Testing — the two gates

Perenne has a deliberate split, and **both must pass**:

- **Pure logic → host unit tests.** Wire formats, parsers, checksums, the KB/FS
  logic, crypto wrappers, line discipline — anything that doesn't need hardware —
  lives in a `libs/` crate and is tested with `cargo test` on your host. Write these
  **test-first** (red → green → commit).
- **Kernel/device integration → the boot smoke test.** Behavior that only shows on
  the real (QEMU) system is proven by `./tools/test-qemu.ps1`, which boots the kernel
  headless and asserts the expected serial output. When you add a milestone, add its
  assertion line to that test.

Before opening a PR:
```powershell
cargo test                     # all host tests green
./tools/test-qemu.ps1          # boot smoke passes (exit 0)
./tools/check-references.ps1   # docs resolve
```

## Code conventions

- **Rust, memory-safe by default.** `unsafe` is allowed only where it must be (MMIO,
  assembly, raw device memory) and **every `unsafe` block carries a `// SAFETY:`
  comment** justifying it.
- **Match the neighbours.** Naming, comment style, and structure should read like the
  file you're editing. Prefer small, focused files.
- **U-mode component code is special.** Code that runs in a user-space component lives
  in `#[link_section = ".user_text"]` and **must not** call kernel `.text` or use
  iterators (`for … in`, ranges) — in debug builds those can become calls into kernel
  code the component can't fetch. Use `#[inline(always)]` helpers and manual `while`
  loops. (If this is new to you, read learning notes 0034–0035.)
- **Keep the trusted core small.** New drivers/features belong in **unprivileged
  user-space components** holding only the capabilities they need
  ([ADR 0007](docs/decisions/0007-extensibility-user-space-components.md)), not in the
  kernel.

## Commits & pull requests

- **[Conventional Commits](https://www.conventionalcommits.org):** `feat(net): …`,
  `fix(cap): …`, `test: …`, `docs: …`, `refactor: …`, `chore: …`. Keep the subject
  imperative and specific.
- **Small, buildable commits.** Each commit should build and (ideally) keep the tests
  green. Frequent small commits over one giant one.
- **Sign your commits** if you can, and **use no AI co-author trailer.** Using an AI
  assistant is welcome — but commits are authored by *you*, in your name, with no tool
  attribution line.
- **PRs:** describe *what and why*, link the spec/plan (or issue), and confirm
  `cargo test` + `./tools/test-qemu.ps1` + `./tools/check-references.ps1` pass. Keep a
  PR to one coherent change.

## When to write an ADR

If your change makes a decision that's **hard to reverse** or that future
contributors will ask "why did we do it this way?" about (a dependency, an
architectural boundary, a protocol/format choice), add an
[Architecture Decision Record](docs/decisions/) — context, the choice, the
consequences. Don't rewrite old ADRs; supersede them with a new one. That's how the
*reasoning* stays durable.

## Reporting issues & the knowledge base

- **Bugs / ideas:** open a GitHub issue with what you expected, what happened, and how
  to reproduce (the exact command + serial output helps).
- **Record real problems you solve.** Capturing issues+fixes is this project's whole
  purpose, not a chore. When you hit and fix a genuine toolchain/system problem,
  consider adding it to [`knowledge-base/`](knowledge-base/) using the
  [issue-record schema](knowledge-base/schema/issue-record.md). That growing memory is
  dogfooded — the self-healing organism reads part of it at boot — so keep entries
  accurate to the schema.

## Code of conduct

Be kind, be rigorous, assume good faith. Critique ideas, not people. Disagreements
are resolved with evidence (a test, a spec, a benchmark), not volume.

## License

By contributing, you agree that your contributions are licensed under the project's
[Apache License 2.0](LICENSE).
