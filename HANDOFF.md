# HANDOFF

The implementation diary. Read it before starting; update it before finishing.
A task is not complete until this file reflects the work.

## Status

Specs 36-42 landed and CI-green. The `ARCHITECTURE.md` plan is complete: critic,
`for_each`/quorum ensembles, budget ceilings, and `split` narrowing. Specs 41-42
fixed section-parsing defects that were silently dropping a third of every spec.
The `deploy/orchestration/` bundle now lets Claude Code delegate implementation
to `pi` instead of Claude subagents.

## Last session

- **Did**: built the portable pi-orchestration bundle — directives, skills for
  both layers, harness, installer, templates.
- **Outcome**: installer verified into a throwaway prefix; harness syntax-checked
  and its no-model roles exercised. `pi` auto-load behaviour verified in both
  directions.
- **Left uncommitted**: nothing.

## Open issues

- **The critic has never faced a live model.** Specs 37-39 are proven in wiring
  only, against scripted workers and a `pi` shim. Six provider keys are present —
  this was never blocked, it simply was not done.
- **The orchestration bundle has not been run end-to-end on a real spec.** Its
  safety properties are verified; the loop is not.
- Specs 36-41 use shell-block Exit Criteria rather than the `- [ ]` house format,
  so pidag cannot split or validate them. Left as-is deliberately; the rule is
  now in CLAUDE.md for new specs.

## Next steps

1. Validate the critic against a live model — the assumption everything in specs
   37-39 rests on. Small, cheap, unblocked.
2. Run one real spec through the orchestration bundle for a measured token
   comparison against the ~1.9M-token Claude-subagent baseline.
3. Composition (DAG within DAG), audit C-1 — the largest remaining capability gap.

## Do not repeat

- Do not assume an agent's claim about its environment. Three separate agents
  reported "no model credentials in this container"; it was false, and the claim
  reached three documents before anyone checked.
- `~/.pi/agent/system.md` is NOT auto-loaded by pi, and its first commandment
  orders a git commit before editing — which this project forbids. Do not append
  it. `CLAUDE.md` auto-loads; `DIRECTIVES.md` is passed explicitly.
- spec-35 (index identity): its performance premise died on measurement. Deferred
  deliberately, not forgotten.
