# Design docs — specs & plans

This folder is the home for Perenne's **per-change design records**. Every
non-trivial change is developed in the same disciplined cycle, and the artifacts
live here:

```
idea → spec (the what & why) → plan (the how, task-by-task) → build → learning note
        └─ docs/design/specs/   └─ docs/design/plans/         code   └─ docs/learning/
```

- **`specs/`** — one design doc per change: the problem, the chosen approach and
  the alternatives, the architecture, error handling, testing, and scope (what's
  explicitly *out*). Written and agreed **before** any code.
- **`plans/`** — the implementation plan for that spec: bite-sized, testable tasks
  with the actual code and commands, TDD-first, frequent commits.

After the build, a short, honest **[learning note](../learning/)** captures what
was non-obvious.

## Tool-agnostic by design

These records are **independent of any particular tool or assistant.** They were
produced with AI assistance, but the format is just *good engineering hygiene* —
a human contributor or any AI agent follows the same spec → plan → build → note
loop. Nothing here depends on a specific product; the value is the reasoning,
captured durably so "why was it built this way?" is always answerable.

See [`CONTRIBUTING.md`](../../CONTRIBUTING.md) for how to add a change, and
[`docs/roadmap/roadmap.md`](../roadmap/roadmap.md) for how the phases fit together.
