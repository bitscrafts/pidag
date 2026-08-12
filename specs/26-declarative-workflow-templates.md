# pidag — spec-26: Declarative workflow templates — any graph, not one hardcoded chain

- **Project**: `/projects/pidag`
- **Crate**: `pidag`
- **Priority**: HIGH — architectural; unblocks every workflow that is not SDD
- **Status**: PLANNED
- **Depends-On**: spec-25 (`after` edges) and spec-23 (`verify`) as **primitives** the
  templates use. Implement those first; this spec makes them expressible as data.
- **User direction (2026-08-10)**: *"pidag must be flexible, not working with a fixed DAG
  template — it should be able to run any graph"*

---

## Overview

pidag's engine is already general. `src/scheduler/` and `src/core/` contain **no**
SDD-specific knowledge — verified: `grep -rniE "implement-iter|quality-gate|validate-iter"
src/scheduler/ src/core/` returns nothing — and `pidag run <dag.json>` executes any DAG.

The rigidity is entirely in **one generator**, `src/sdd/mod.rs`, and it is severe:

- Node ids are **unrolled string literals** — `"implement-iter1"`, `"implement-iter2"`,
  `"implement-iter3"`, `"quality-gate-1..3"`, `"validate-iter1..3"`. There is no loop.
- **`[sdd] max_iterations` is never read.** It is defined, defaulted, parsed and written
  into the config template (`src/core/config.rs:118,165,235,354,370`) and no code in
  `src/sdd/` consumes it. Setting `max_iterations = 5` silently does nothing.
- Every topology change therefore means editing Rust. spec-14 (gates), spec-23 (`verify`)
  and spec-25 (`after`) each require another hand-edit of the same unrolled literals.

The consequence: pidag can only self-heal in exactly one shape. A research loop, a
refactor loop, a fan-out review, a two-stage plan-then-implement — none are expressible
without a code change, even though the engine could run all of them today.

**This spec turns workflow topology into data.** The SDD chain becomes one template among
several, shipped as a default.

---

## Design

### Layering (the point of the whole thing)

| layer | what it is | changes how often |
|---|---|---|
| **Engine** (`scheduler/`, `core/`) | primitives: `depends_on`, `gate` (14), `after` (25), `verify` (23), retry/failover | rarely |
| **Templates** (`.pidag/workflows/*.toml`) | topology as data: which nodes, which edges, how many iterations | per workflow |
| **Specs** (`specs/NN-*.md`) | the work to do | constantly |

Today layers 1 and 2 are fused. Separating them is the entire deliverable.

### Template format

```toml
# .pidag/workflows/sdd.toml
name        = "sdd"
description = "Spec-driven implement -> quality-gate -> validate recovery loop"
iterations  = 3            # default; overridden by [sdd] max_iterations or --iterations

[[nodes]]
id      = "validate-baseline"
type    = "shell"
command = "{validate_script} {spec_path}"

[[repeat]]                  # expanded once per n in 1..=iterations
  [[repeat.nodes]]
  id     = "implement-iter{n}"
  type   = "llm"
  prompt = "{prompt.implement}"
  models = "{models.worker(n)}"
  # first iteration is unconditional; later ones are fix passes
  depends_on = ["validate-iter{n-1}"]   # dropped when n == 1
  gate       = "validate-iter{n-1}:fail" # dropped when n == 1
  verify     = "git diff --quiet && exit 1 || exit 0"   # spec-23 V3

  [[repeat.nodes]]
  id      = "quality-gate-{n}"
  type    = "shell"
  command = "{quality_gate_script}"
  after   = ["implement-iter{n}"]        # spec-25

  [[repeat.nodes]]
  id      = "validate-iter{n}"
  type    = "shell"
  command = "{validate_script} {spec_path}"
  after   = ["implement-iter{n}", "quality-gate-{n}"]   # arbiter: always runs
```

**Substitution is deliberately tiny** — no general templating language:

- `{n}` → current iteration; `{n-1}` → previous.
- Edges referencing `{n-1}` when `n == 1` are **dropped**, which is what makes iteration 1
  unconditional without a special case.
- `{spec_path}`, `{project_root}`, `{validate_script}`, `{quality_gate_script}` from config.
- `{prompt.<key>}` → a named prompt block; `{models.worker(n)}` → the model chain for
  iteration n (spec-24).

Anything more expressive is a general-purpose config language, and that is a non-goal.

---

## Requirements

### Functional

- **W1 (template loader)**: workflows load from `.pidag/workflows/<name>.toml`, falling
  back to built-ins embedded in the binary. `pidag sdd <spec> --workflow <name>`
  (default `sdd`).
- **W2 (built-in parity)**: the built-in `sdd` template must generate a DAG **equivalent
  to today's hardcoded output**, with spec-25's `after` edges and spec-23's `verify`. A
  golden-file test pins this.
- **W3 (`max_iterations` finally works)**: iteration count resolves as
  `--iterations` > `[sdd] max_iterations` > template `iterations`. Setting 5 produces 5
  iterations. This closes a config key that has lied since it was introduced.
- **W4 (no unrolled literals)**: after this spec, `grep -c 'implement-iter[0-9]' src/sdd/*.rs`
  is **0**. Node ids exist only in templates.
- **W5 (validation before execution)**: an expanded template is validated
  (`dag.validate()` — cycles, dangling refs incl. `after`) and, on failure, reports the
  **template** name, node id and offending edge — not a post-expansion id the author never
  wrote.
- **W6 (`pidag workflows`)**: list available workflows and `pidag workflows show <name>`
  renders the expanded DAG (reusing `pidag describe`'s mermaid output) **without running
  it**, so a topology can be inspected before it costs tokens.
- **W7 (ship more than one)**: at least two built-ins, to prove the abstraction is real
  rather than SDD with extra steps:
  - `sdd` — today's implement/quality-gate/validate loop.
  - `research` — a fan-out/fan-in shape: N parallel investigation nodes → one synthesis
    node → one validate. No iteration loop, no quality gate. If the format cannot express
    this, the design has failed.
- **W8 (arbitrary graphs stay first-class)**: `pidag run <dag.json>` is unchanged. A
  hand-written DAG remains fully supported and needs no template.

### Non-Functional

- **N1**: Existing DAG JSON files and `pidag run` behave identically. No existing test
  changes.
- **N2**: No engine changes. If expressing a template requires touching
  `src/scheduler/`, stop and escalate — that means a primitive is missing and belongs in
  its own spec, not smuggled in here.
- **N3**: Template parse errors never produce a partially-built DAG.

---

## TDD Contract

| id | test | given | expects |
|----|------|-------|---------|
| W1 | `test_loads_project_template_over_builtin` | `.pidag/workflows/sdd.toml` present | project file wins over built-in |
| W2 | `test_builtin_sdd_matches_golden` | built-in `sdd`, 3 iterations | expanded DAG equals the golden file (ids, deps, gates, `after`, `verify`) |
| W3a | `test_iterations_from_config` | `[sdd] max_iterations = 5` | 5 implement nodes, ids `implement-iter1..5` |
| W3b | `test_iterations_cli_overrides_config` | config 3, `--iterations 2` | 2 iterations |
| W4 | `test_no_hardcoded_node_ids_in_src` | source scan | no `implement-iter<N>` literal in `src/sdd/` |
| W5a | `test_first_iteration_drops_prev_edges` | n=1 | `implement-iter1` has no `depends_on`/`gate` referencing iter0 |
| W5b | `test_template_error_names_template_and_node` | template with `after = ["ghost"]` | error names workflow, node id, and `ghost` |
| W6 | `test_workflows_show_renders_without_running` | `pidag workflows show sdd` | mermaid emitted; no node dispatched |
| W7 | `test_research_template_expands` | built-in `research`, fan-out 3 | 3 parallel nodes → synthesis → validate; no gates, no iteration loop |
| W8 | `test_hand_written_dag_unchanged` | existing `dag.json` via `pidag run` | byte-identical behaviour (N1) |

**W7 is the falsifiability test.** A format that can only express the SDD chain has not
made pidag flexible — it has just moved the hardcoding into a file.

---

## Exit Criteria

```bash
cd /projects/pidag
test -f src/workflow/mod.rs
test -f src/workflow/templates/sdd.toml
test -f src/workflow/templates/research.toml
[ "$(grep -c 'implement-iter[0-9]' src/sdd/*.rs)" = "0" ]     # W4
grep -q "max_iterations" src/sdd/mod.rs                        # W3, finally consumed
# NOTE (2026-08-11, spec-27 R7): this criterion originally named src/workflow/mod.rs and
# failed. The CRITERION was wrong, not the code — resolution is CLI > config > template > 3
# at src/sdd/mod.rs:183-190, exactly as W3 requires. Corrected in place.
pidag workflows | grep -q research
pidag workflows show sdd | grep -q "implement-iter3"
cargo test --no-fail-fast 2>&1 | grep -E "^test result:"
cargo clippy -p pidag -- -D warnings
cargo fmt --check
```

**Prose criterion**: a user adds `.pidag/workflows/myloop.toml` describing a graph pidag
has never seen, runs `pidag sdd spec.md --workflow myloop`, and it executes — **with no
change to pidag's source**. Until that holds, the workflow is still hardcoded, only in a
new place.

---

## Guardrails

- **Do NOT** invent a general-purpose templating language. The substitution set in Design
  is the whole surface; anything beyond it needs its own spec.
- **Do NOT** put workflow knowledge into the engine (N2). The scheduler must stay ignorant
  of what a "quality gate" is.
- **Do NOT** change `pidag run`'s DAG JSON format (W8/N1). Templates *produce* that format;
  they do not replace it.
- **Do NOT** drop the built-in fallback — pidag must work in a project with no
  `.pidag/workflows/` directory.
- **Do NOT** implement this before spec-25 and spec-23. The templates reference `after`
  and `verify`; writing them first would bake in shapes the engine cannot honour.
- No new dependencies (`toml` and `serde` are already present).

---

## Files to Modify

| File | Change |
|------|--------|
| `src/workflow/mod.rs` | **NEW** — template model, loader, expansion, validation |
| `src/workflow/templates/sdd.toml` | **NEW** — built-in, parity with today's chain |
| `src/workflow/templates/research.toml` | **NEW** — fan-out/fan-in, proves generality (W7) |
| `src/sdd/mod.rs` | Reduced to: parse spec sections → hand them to the workflow expander. All unrolled literals deleted |
| `src/cli/sdd.rs` | `--workflow <name>`, `--iterations <n>` |
| `src/cli/workflows.rs` | **NEW** — `pidag workflows [show <name>]` |
| `src/bin/pidag.rs` | Register the `workflows` subcommand |
| `src/core/config.rs` | `[sdd] max_iterations` now genuinely consumed (W3) |
| `tests/workflow_tests.rs` | **NEW** — W1-W8 + golden file |

---

## Why this is the right shape

Three specs in a row (14 gates, 23 verify, 25 after) each ended with the same chore:
hand-edit the same unrolled node literals in `src/sdd/mod.rs`. That is the signal that
topology belongs in data, not code.

The engine is already general — this spec stops one generator from pretending it is the
only workflow pidag can run, and makes `max_iterations` mean what it has always claimed.

## Memory

Store on completion: `pidag/specs/26-declarative-workflow-templates`,
`claude-pi-delegation/decision/20260810-topology-is-data-not-code`.
