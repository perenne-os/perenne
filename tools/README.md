# tools/

Helper scripts for common development actions. Run them from the repository root.

## Scripts

| Script | What it does |
|--------|--------------|
| `build.ps1` | Builds the Cargo workspace (host) and runs the unit tests. |
| `run-qemu.ps1` | Boots QEMU's RISC-V virtual machine with the built-in OpenSBI firmware. |
| `check-references.ps1` | Validates doc cross-references: every `KB-####` id, every root-relative `docs/`/`knowledge-base/` path, and every Markdown link target (`.md`) mentioned must actually resolve. Skips historical snapshots under `docs/superpowers/`. |

## Usage

```powershell
./tools/build.ps1
./tools/run-qemu.ps1
./tools/check-references.ps1
```

## Notes

- **`run-qemu.ps1` currently boots firmware only.** It loads QEMU's built-in
  OpenSBI firmware to prove the RISC-V virtual machine works on your machine.
  Our own kernel does not exist yet — loading it (via `-kernel`) arrives in
  **Phase 1**.
- **Exit QEMU** with `Ctrl-A` then `X`.
- These are PowerShell scripts (Windows). A more Unix-style workflow (WSL2,
  shell scripts) is an option later; see `docs/learning/0001-dev-environment.md`.
