# pidag — architecture analysis and feasibility

**Date**: 2026-08-12 · **Head**: `a69f454` · **Question**: is pidag the right shape to be
the main implementer for our projects, and what has to change?

---

## 1. The structural argument for pidag, which the failure research supports

Published failure rates for multi-agent LLM systems in production are **41–86.7%**. The
MAST taxonomy (NeurIPS 2025, 1,600+ traces) attributes them **42% specification ambiguity,
37% coordination breakdowns, 21% verification gaps**. ICML 2026 adds the sharpest finding:
**failures originate in the orchestrator, not the individual agents** — as task chains grow,
the orchestrator faces mounting information pressure from more tools, longer history and
richer error feedback, and its decisions degrade.

That last finding is an argument *for* pidag's design, not against it.

In most frameworks the orchestrator is an LLM deciding, at runtime, what to do next. It is
exactly the component that degrades under pressure. **pidag's orchestrator is a static
graph.** The topology is fixed before execution — planned by Claude Code with a human in the
loop, expanded from a template, validated, and then executed by a deterministic scheduler
that makes no judgements at all. It cannot degrade under information pressure because it
does not reason during the run.

So pidag structurally sidesteps the dominant failure mode. That is worth more than it
sounds, and it is the reason the "just use in-context prompting" advice does not settle the
question: prompting has no durable state, no checkpoint, no resume, no event log, and no
verification gate.

**Conclusion: the graph is the right substrate. Keep it.**

## 2. Where our own failures actually landed

Scoring one full session of work against MAST:

| category | MAST share | our instances |
|---|---|---|
| **specification** | 42% | spec-33 O2 premised on a wrong reading of the SDK · spec-32 exit criterion matching 2 of 12 discards · R4 would have deadlocked by locking both ends · I7 put hydration in a module with no store handle · spec-35's premise dead on measurement · U-2 annotated where it needed to narrow |
| **coordination** | 37% | agent reports wrong in detail nearly every time · a tree left uncompiling and reported complete · totals miscounted three times |
| **verification** | 21% | six checks that looked rigorous and tested the wrong surface |

Our distribution matches the published one closely, and **the specification errors were all
mine** — in the specs, the exit criteria, the guards. Not the workers'. This is the ICML
finding reproduced in miniature: the orchestrating layer is where the defects were.

**Consequence for investment**: engine polish is not where the return is. spec-35 is a
performance refactor whose numeric case the baseline already destroyed. The return is in the
**specification and verification layers** — 63% of failure mass by the taxonomy, and ~100%
of ours.

## 3. What is missing, measured against the patterns that survived

Production survivors in 2026 compose to: *supervisor-worker + verifier-critic for
high-stakes work, graph orchestration for observability.* pidag has the graph and the
supervisor-worker split. It is missing the critic.

| capability | pidag today | gap |
|---|---|---|
| graph execution, topo order, concurrency | ✅ | — |
| retry, model fallback chain | ✅ | — |
| conditional gates, ordering edges | ✅ | — |
| checkpoint / resume / event log / UI | ✅ | — |
| effect verification (`verify`) | ⚠️ **shell only** | **cannot use a model as critic** |
| upstream output into a downstream prompt | ✅ (spec-29) | — |
| **critic / reviewer node** | ❌ | the single highest-value gap |
| **parallel ensemble + adjudication** | ❌ | no way to run N models and compare |
| fan-out over a parameter list | ❌ | `research` template hardcodes 3 branches |
| composition (DAG within DAG) | ❌ | audit C-1 |
| budget ceiling | ❌ | `--allow-paid` is a boolean, not a limit |
| work decomposition (`split`) | ⚠️ | divides the checklist, not the work |

## 4. The one change that matters most: `verify` becomes a critic

`Node.verify` already exists as a seam. It runs a **shell command** and gates the node on its
exit status. That is a verifier — just the weakest possible kind. Every published
high-stakes pattern uses a *model* as the critic: the producer emits, a critic reviews, and
output ships only if the critic passes.

The change is small because the wiring is already there:

```
Node.verify: Option<String>              // shell command, today
       ↓
Node.verify: Option<Verify>              // proposed
  enum Verify {
      Shell(String),                     // unchanged, existing DAGs keep working
      Critic { prompt: String, models: Vec<ModelRef> },
      All(Vec<Verify>),                  // shell AND critic must both pass
  }
```

A `Critic` verify dispatches a *worker*, not a subprocess: it receives the node's output via
the spec-29 interpolation already built, and returns pass/fail plus a reason that flows into
the repair node's prompt. The retry/fallback machinery, the event log, the artifact store and
the gate semantics all apply unchanged.

**This is what closes the 21% verification gap, and it composes with the 42% specification
gap**: a critic that reads the spec's Exit Criteria and judges the diff against them catches
exactly the class of error the shell `verify` cannot — "the code compiles and the tests pass,
but it does not do what the spec asked."

That is precisely the failure mode this codebase produced six times in one session.

## 5. The second change: ensemble adjudication

Your instinct — run more than one model in parallel to review and fix — is the pattern the
research supports, and pidag's graph makes it expressible **today** without engine changes:

```
implement → ┌ critic-a (model 1) ┐
            ├ critic-b (model 2) ┤ → adjudicate → gate → repair
            └ critic-c (model 3) ┘
```

Three critic nodes, `after` edges into an adjudicator, a gate on its verdict. Every primitive
exists. What is missing is only:

- **`for_each` fan-out** so the width is data, not copy-paste (audit C-2), and
- a **`quorum` helper** so the adjudicator does not need a model call for a simple vote.

Nodes voting is cheaper and more reliable than one model self-reviewing, and it is the
cheapest available answer to "who checks the checker".

## 6. Feasibility: what to build, and what not to

### Build — high return, small surface

| change | effort | closes |
|---|---|---|
| `Verify::Critic` — model as verifier | **2–3 d** | verification gap (21%) |
| `for_each` fan-out over a parameter list | 1 d | C-2, enables ensembles |
| quorum/adjudication helper | 1 d | ensemble without an extra model call |
| budget ceiling (`--max-spend`, abort on breach) | 0.5 d | the $50k-at-scale caution |
| `split`: narrow Architecture per child | 0.5 d | U-2 remainder |
| spec `Status` fields + CI + remote | 1 d | the specs currently lie; the gate is manual |

**≈ 6–7 days** to a genuinely capable implementer.

### Do not build

- **An LLM orchestrator that decides the graph at runtime.** This is precisely the component
  ICML 2026 identifies as the origin of failure. pidag's static graph is an asset; making it
  dynamic would import the dominant failure mode deliberately.
- **Swarm / many-agent topologies.** The guidance is explicit: teams deploying swarms on
  tasks a three-node supervisor handles are spending budget on infrastructure the use case
  does not need. Add patterns against *measured* failures, not anticipated ones.
- **spec-35 (index identity).** Its performance premise did not survive the baseline —
  scaling is linear at N ≤ 500. Only its structural case remains, and that is cosmetic
  next to a missing critic.
- **Chasing the last 30% automatically.** Edge cases, security, platform integration — the
  research is consistent that this needs human judgement. Design pidag to *surface* that
  boundary (a critic that says "I cannot verify this criterion") rather than paper over it.

## 7. What pidag can and cannot be

**Can**: a reliable executor of *pre-planned* work with durable state, real verification, and
a full audit trail. Multi-day tasks that survive interruption. Repetitive structured work —
migrations, refactors, spec-conformance passes — across many files. Work where "prove it
did what was asked" matters more than raw speed.

**Cannot, and should not try to be**: an autonomous engineer. The plan has to come from
somewhere, and every piece of evidence — published and our own — says the planning layer is
where failure concentrates. pidag's job is to execute a plan faithfully and prove it. Ours is
to write plans worth executing.

The honest reframing of this session: we spent it hardening the executor and found the
executor was not the weak part. **The specs were.** That is where the next six days belong.

---

## Sources

- MAST taxonomy and production failure rates — *Why Multi-Agent LLM Systems Fail*, Augment Code, 2026
- *Can You Trust the Orchestrator? Entropy Dynamics Reveal Multi-Agent System Vulnerabilities*, ICML 2026
- *In-Context Prompting Obsoletes Agent Orchestration for Procedural Tasks*, arXiv 2604.27891
- *Multi-Agent in Production in 2026: What Actually Survived*, M. Lanham
- *The 70% Problem: Hard truths about AI-assisted coding*, A. Osmani
