# pidag

A minimal DAG orchestrator for LLM workers. You describe the work as a graph;
pidag executes it deterministically, verifies each step actually did something,
and keeps a durable record you can resume from.

```bash
pidag sdd specs/01-my-feature.md --run --model <model>
```

## Why a static graph

Published failure rates for multi-agent LLM systems run **41–86.7%**, and the
[ICML 2026 orchestrator study](docs/ARCHITECTURE.md#sources) locates the cause: failures
originate in the *orchestrator*, not the individual agents. As a task chain grows, an LLM
orchestrator accumulates tools, history and error feedback until its decisions degrade.

pidag's orchestrator does not reason during a run. The topology is fixed before execution —
planned, expanded from a template, validated — and then executed by a scheduler that makes no
judgements. It cannot degrade under information pressure because it makes no decisions under
pressure.

That is the whole design bet: **put the intelligence in planning and verification, keep the
executor dumb and deterministic.**

## What it does

| | |
|---|---|
| **Topological execution** | bounded concurrency, per-node retry, model fallback chains |
| **Conditional gates** | `gate: "<node>:fail"` — a repair node that runs only when its target failed |
| **Ordering edges** | `after` — waits for a node to reach *any* terminal state, so validators always run |
| **Effect verification** | `verify` / `verify_pre` — a node is `Done` only if it demonstrably changed something |
| **Output interpolation** | `{{node.output}}` — a repair node receives what actually failed |
| **Durable state** | redb vault, checkpoint/resume, full event log, run fencing |
| **Observability** | web UI, mermaid rendering, `pidag describe` |

## The self-healing loop

`pidag sdd` expands a spec into an implement → quality-gate → validate cycle:

```
validate-baseline ─┐
                   ├→ implement-iter1 → quality-gate-1 → validate-iter1 ─┐
                                                                         ├→ implement-iter2 (gated on failure)
                                                                         └→ … up to N iterations
```

If `validate-iter1` passes, the repair iterations are **skipped**. If it fails, the gate fires
and `implement-iter2` runs — receiving the validator's actual output, not a placeholder.

## Verification is the point

The `verify` predicate is what separates this from a retry loop. A node claiming success
while changing nothing is the characteristic LLM failure, and pidag treats it as a failure:

```toml
verify_pre = "( git status --porcelain; git diff; … ) | sha256sum"
verify     = "test \"$( … )\" != \"$PIDAG_VERIFY_PRE\""
```

The predicate measures a **delta across the node**, not a state. Measuring state was a real
defect: on resume the tree is already dirty, so "is the tree dirty" is satisfied before the
node runs — and a node that did nothing verified green. See
[docs/FINDINGS.md](docs/FINDINGS.md).

## Requirements

- **Rust stable** — that is all you need to build, run shell-node DAGs, and use the UI.
- **An agent binary and provider credentials** for LLM nodes. pidag drives
  [pi](https://github.com/bitscrafts/pi_agent_rust) as its worker; set
  `PIDAG_AGENT_BACKEND=pi` and provide the relevant `*_API_KEY`.

Shell-only DAGs need neither, which is what the benchmark and most tests use — they
exercise the scheduler, gates, verification and store with no model calls at all.

## Quick start

```bash
cargo build --release -p pidag
install -m755 target/release/pidag ~/.local/bin/pidag

cd my-project && git init -q .          # the project root must be its own git repo
pidag attach                            # creates .pidag/
pidag split specs/01-feature.md --auto  # only if it has more than 7 exit criteria
pidag sdd specs/01-feature.md --run --model <model>
pidag ui                                # http://localhost:4600
```

Two prerequisites that otherwise waste a session:

- **The project root must be its own git repository.** The verify predicate hashes git
  output, so a directory ignored by an outer repo reports zero changes and every node fails
  verification for unrelated reasons.
- **The deliverables must be absent at the start**, or the baseline validator passes, the
  gate never fires, and the run reports success while demonstrating nothing.

## Status

Working end to end: a spec goes in, working software comes out, verified against the spec's
own exit criteria. See [tests/fixtures/e2e/RUNS.md](tests/fixtures/e2e/RUNS.md) for the
recorded runs and what each one overturned.

Known gaps, and the planned architecture, are in [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md).
The largest is that `verify` runs only shell commands — the next change makes it accept a
**model as critic**, which is the pattern every surviving high-stakes system uses.

## Documentation

| | |
|---|---|
| [docs/ARCHITECTURE.md](docs/ARCHITECTURE.md) | design rationale, feasibility, what to build and what not to |
| [docs/FINDINGS.md](docs/FINDINGS.md) | defects found, and the pattern behind them |
| [specs/](specs/) | the spec-driven development record |
| [CLAUDE.md](CLAUDE.md) | rules for agents working in this repo |

## License

See [LICENSE](LICENSE).
