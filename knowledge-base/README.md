# Knowledge Base

This is the first cell of the project's **self-healing knowledge organism** (see [ADR 0005](../docs/decisions/0005-self-healing-knowledge-organism.md) and [architecture/self-healing.md](../docs/architecture/self-healing.md)).

It is a structured, **machine- and human-readable** memory of problems the system (and its developers) have encountered, along with the **proven fixes** ("playbooks") for them. The long-term goal: when the OS hits a problem, it **consults this memory first** — diagnosing and repairing itself instead of depending on a human support community.

## How it will be used

1. A problem is detected.
2. The OS searches here for a matching, previously-diagnosed issue.
3. If a fix playbook exists, it is applied **inside the safety cage** — capability-checked, logged, reversible, auditable.
4. New problems and their resolutions are recorded here, so the memory grows.

## Layout

```
knowledge-base/
├── README.md              this file
├── schema/                the format that records follow
│   ├── issue-record.md    the field-by-field schema + a blank template
│   └── example-0001.md    pointer to the filled, real example (entries/KB-0001.md)
└── entries/               individual diagnosed-issue records accumulate here
    ├── KB-0001.md         SSH commit-signing on Windows (also the worked example)
    ├── KB-0002.md         SSH push transport on Windows (twin of KB-0001)
    ├── KB-0003.md         Windows SDK / kernel32.lib linker error
    ├── KB-0004.md         SSH signature verification (allowed_signers)
    └── KB-0005.md         user-space component killed by a fault (first runtime entry)
```

## Status (Phase 0)

Only the **schema and store** are seeded. There is **no runtime diagnosis/healing logic yet** — that arrives in Phase 5. For now, records are written by humans (us), which both validates the format and seeds the organism with genuine early memories.
