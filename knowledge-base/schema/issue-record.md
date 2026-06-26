# Issue Record Schema

Every entry in `knowledge-base/entries/` is a Markdown file with a **YAML frontmatter** block (machine-readable) followed by free-form notes (human-readable). This keeps records easy for both a future rule engine and a human to read.

## Fields

| Field | Type | Meaning |
|-------|------|---------|
| `id` | string | Unique identifier, e.g. `KB-0001`. |
| `title` | string | Short, specific summary of the problem. |
| `status` | enum | `open` · `diagnosed` · `fixed` · `wont-fix`. |
| `severity` | enum | `low` · `medium` · `high` · `critical`. |
| `component` | string | Subsystem affected, e.g. `boot`, `memory`, `hal`, `dev-environment`. |
| `symptoms` | list of strings | Observable signs — what is seen when it happens. |
| `diagnosis` | string | The root cause, once understood. |
| `playbook` | list of strings | Ordered, **reversible** steps to fix it. |
| `match-cause` | string | *(optional)* Machine-matchable token tying this issue to a runtime fault class the kernel can diagnose. Vocabulary: `page-fault` (a load/store/instruction page fault). Absent ⇒ not runtime-matchable. |
| `verification` | string | How to confirm the fix actually worked. |
| `created` | date | ISO date the record was created (`YYYY-MM-DD`). |
| `updated` | date | ISO date last updated. |
| `references` | list of strings | Links to ADRs, docs, or commits. |

## Conventions

- **Playbook steps must be reversible** (or state explicitly how to undo them), per the safety cage.
- `match-cause` is the only field the in-kernel self-healer matches on at runtime; it must be one of the documented tokens, added together with a kernel `Cause→token` arm.
- Keep `symptoms` concrete and matchable — they are how the system will recognize a recurrence.
- One issue per file; filename matches the `id` (e.g. `entries/KB-0001.md`).

## Blank template

```markdown
---
id: KB-NNNN
title: ""
status: open          # open | diagnosed | fixed | wont-fix
severity: medium      # low | medium | high | critical
component: ""
match-cause: ""       # optional: page-fault (a runtime fault class the kernel diagnoses)
symptoms:
  - ""
diagnosis: ""
playbook:
  - ""
verification: ""
created: YYYY-MM-DD
updated: YYYY-MM-DD
references:
  - ""
---

## Notes

<free-form detail, history, dead-ends, links>
```
