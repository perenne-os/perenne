# 0001 — Development Environment

What we set up in Phase 0 to be able to build and run a kernel, and why.

## Rust (via rustup), pinned to nightly

- We use **rustup**, Rust's toolchain manager, and pin the exact toolchain in `rust-toolchain.toml` so every machine builds identically.
- We pin **nightly** (not stable) because Phase 1 needs `-Z build-std` — the ability to compile Rust's `core`/`alloc` libraries for our own bare-metal target. That feature is nightly-only.
- The pin also lists the `riscv64gc-unknown-none-elf` target and components (`rust-src`, `rustfmt`, `clippy`, `llvm-tools`). Running any `cargo` command in the repo makes rustup install all of it automatically.

## QEMU

- **QEMU** emulates a whole RISC-V computer so we can run our kernel without real hardware.
- The check `qemu-system-riscv64 -machine virt -nographic -bios default` boots QEMU's built-in **OpenSBI** firmware and prints its banner. That proves the virtual machine + firmware work *before our kernel exists*. Exit with **Ctrl-A** then **X**.

## Windows specifics (what bit us, so future-you doesn't have to wonder)

- We develop **natively on Windows** with PowerShell. (A more Unix-style workflow via **WSL2** is a valid alternative if we ever want it; nothing in the project prevents switching.)
- **MSVC + Windows SDK:** Rust's default Windows toolchain links with Microsoft's `link.exe`, which needs the **Windows SDK** libraries (e.g. `kernel32.lib`). Installing only the MSVC *compiler* without the SDK causes `LINK : fatal error LNK1181: cannot open input file 'kernel32.lib'` when building test executables. Fix: install the Windows SDK. (Knowledge base: `knowledge-base/entries/KB-0003.md`.)
- **SSH commit signing:** git was configured to sign commits with an SSH key. Git for Windows ships its own `ssh-keygen` that can't reach the Windows `ssh-agent`, so commits prompted for a passphrase and failed non-interactively. Fix: run `ssh-agent` (Automatic service) with the key added, and point git at the system OpenSSH via `git config --global gpg.ssh.program "C:/Windows/System32/OpenSSH/ssh-keygen.exe"`. (Knowledge base: `knowledge-base/entries/KB-0001.md`.)
- **SSH push (`git@github.com: Permission denied (publickey)`):** the *same* root cause for the *transport*. Git's bundled `ssh` couldn't reach the agent, so it never presented the registered key. Fix: `git config --global core.sshCommand "C:/Windows/System32/OpenSSH/ssh.exe"`. (Knowledge base: `knowledge-base/entries/KB-0002.md`.) **The general lesson on Windows: redirect git to the system OpenSSH for both signing and transport.**

## Why this matters

Even before the kernel does anything, a *reproducible, working* environment is the real Phase 0 deliverable. Every problem we hit here is the kind of thing the project's self-healing knowledge base will eventually capture for itself.
