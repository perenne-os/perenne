# Self-Healing: The Knowledge Organism

This is the soul of the project ([priority #2](../vision/north-star.md)): instead of depending on a human community for support, the OS builds and consults its **own** growing memory of problems and proven fixes. Decision recorded in [ADR 0005](../decisions/0005-self-healing-knowledge-organism.md).

## The idea

When most operating systems hit a problem, a human searches forums, finds someone who solved it, and applies the fix. Our OS aims to internalize that loop:

1. **Detect** a problem (a service crashed, a check failed, a resource is exhausted).
2. **Consult itself first** — look in the knowledge base for a matching, previously-diagnosed issue.
3. **Apply a known fix** (a "playbook") if one exists — inside the safety cage.
4. **Record** new problems and their resolutions so the organism's memory grows.

Over time, an experience one machine has becomes knowledge every machine can use.

## Where it lives

- The memory lives in [`knowledge-base/`](../../knowledge-base/): a structured, **machine- and human-readable** store of issue records and fix playbooks, with the format defined in `knowledge-base/schema/`.
- The healing *logic* runs as **isolated user-space services** — never in the privileged kernel (consistent with the [security model](security-model.md)).

## The safety cage (non-negotiable)

Every automated action is **capability-checked, logged, reversible, and auditable**. The healer can never gain unchecked power; otherwise it would become the system's biggest vulnerability. It proposes and applies fixes within this cage, and a human can always review and undo what it did.

## Evolution path (deterministic core first)

We build this conservatively, in the order that preserves trust:

1. **Now (Phase 0):** seed the *schema* and the *store*. No runtime logic yet.
2. **Deterministic rules + knowledge base (Phase 5):** explicit, explainable matching of symptoms to playbooks. Fully auditable.
3. **AI advisor, later:** an isolated model that *suggests* diagnoses and fixes for a human or the rule engine to approve. It **advises**; it never silently acts.

The deterministic, explainable core is **never** replaced by a black box. AI augments it from inside the cage — it does not become the authority. This is how "self-healing" stays compatible with "trusted and secure."
