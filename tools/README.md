# tools/

Helper scripts for common development actions. Run them from the repository root.

## Scripts

| Script | What it does |
|--------|--------------|
| `build.ps1` | Builds the Cargo workspace + unit tests (host), then cross-builds the kernel for `riscv64gc-unknown-none-elf`. |
| `run-qemu.ps1` | Cross-builds **our kernel** and boots it under QEMU (riscv64 `virt`, OpenSBI firmware). |
| `test-qemu.ps1` | Non-interactive boot smoke test: boots the kernel headless and asserts the greeting appears on the serial console. Exit code 0 = pass. |
| `check-references.ps1` | Validates doc cross-references: every `KB-####` id, every root-relative `docs/`/`knowledge-base/` path, and every Markdown link target (`.md`) mentioned must actually resolve. Skips historical snapshots under `docs/superpowers/`. |

## Usage

```powershell
./tools/build.ps1
./tools/run-qemu.ps1
./tools/test-qemu.ps1
./tools/check-references.ps1
```

`cargo qemu` (an alias defined in `.cargo/config.toml`) is the one-liner
equivalent of `run-qemu.ps1`.

## Notes

- **`run-qemu.ps1` boots our own kernel** (since Phase 1). Expect the OpenSBI
  banner followed by:

  ```
  hello world from Perenne - Phase 1 (hart 0)
  (kernel is idle; exit QEMU with Ctrl-A then X)
  ```

- **Exit QEMU** with `Ctrl-A` then `X`.
- These are PowerShell scripts (Windows). A more Unix-style workflow (WSL2,
  shell scripts) is an option later; see `docs/learning/0001-dev-environment.md`.
