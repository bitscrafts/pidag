# pidag — Project-Specific Agent Instructions

`pidag` is the minimal DAG orchestrator for the
[claude-pi-delegation](../../docs/claude-pi-delegation/README.md) research
topic. It is the free-tier alternative to `loop-engineer`: Claude Code plans
and drives `pi` workers through declarative DAGs.

## Before doing anything in this crate

1. **Read `docs/ARCHITECTURE.md`** — the design rationale, what is feasible, and what is
   deliberately not being built. **`docs/FINDINGS.md`** records defects already found and
   the pattern behind them; re-deriving those is an anti-pattern.
2. **Search agent-memory** for prior pidag work before designing anything:
   ```bash
   curl -s "http://host.containers.internal:7420/v1/memory/claude-pi-delegation/search?q=pidag&k=5" \
     | jq '.results[] | {key, snippet: (.content[:200])}'
   ```
   Re-deriving known results is an anti-pattern.

## Workspace rules (apply here)

- **Rust-first, surgical changes** — touch only what the task requires.
  Match existing style. Don't refactor adjacent code.
- **Spec-driven (SDD)** — every non-trivial change starts with a spec in
  `specs/<feature>.md` (see `specs/92-configurable-models.md` for the format:
  Overview, Requirements, Architecture, TDD Contract, Exit Criteria,
  Guardrails, Files to Modify).
- **Tests under `_tmp/`** — any test that writes files uses `_tmp/...` as
  its base directory, never `/tmp/`. Gitignored.
- **Quality gate before commit**:
  - `cargo test -p pidag` — all pass
  - `cargo clippy -p pidag -- -D warnings` — clean
- **No emojis in commits; no AI-tool mentions in commits.** Conventional
  prefixes: `feat(pidag):`, `fix(pidag):`, `docs(pidag):`, `refactor(pidag):`.

## Memory update is mandatory at task end

Per the workspace `CLAUDE.md`, a task is NOT complete until the
`store_insight` call returns. The canonical endpoints
(`http://host.containers.internal:7420` REST from inside the dev container; `http://${DEPLOY_HOST_NAME}:7420` does NOT resolve here) and the enforcement checklist live in the parent
workspace `CLAUDE.md` — follow them.

Store pidag findings under topic `claude-pi-delegation`:
- `<topic>/fix/<slug>` for bug fixes
- `<topic>/experiment/<slug>` for experimental results
- `<topic>/phase<N>/<slug>` for phase completions
Specs go to `workspace/specs/pidag-<feature>` (global scope).

## Reference

- **Research docs**: `docs/claude-pi-delegation/` (SSOT hub: `README.md`)
- **Specs**: `specs/` (this folder)
- **Architecture**: `docs/ARCHITECTURE.md` — read first
- **Parent rules**: `~/Private/PROJECTS/20260601-on-research/CLAUDE.md`

## Hard rules for delegated agents (violations happened; these are non-negotiable)

1. **NEVER modify `/projects/_upstream/`.** It is a read-only reference checkout of
   `pi_agent_rust`, on the user's own fork and active branches. Read it freely; never edit,
   never commit there. If pidag needs a capability the SDK does not expose, work around it
   on pidag's side (e.g. probe liveness with a cheap `get_state()` RPC rather than adding an
   `is_alive()` to the SDK). An upstream change was committed onto a live user branch on
   2026-08-10 and had to be reverted.
2. **NEVER add a `Co-Authored-By:` trailer**, and never mention Claude, Anthropic, or any AI
   tool in a commit message. Agent harnesses append this by default; suppress it. Eleven
   commits had to be rewritten with `filter-branch` on 2026-08-10.
3. **Never `rm -rf` a `.pidag/` directory.** It is the only record of a run. Move it aside:
   `mv .pidag .pidag.prev-$(date +%H%M%S)`.
4. **Install built binaries to BOTH** `/root/.local/bin/pidag` and
   `/projects/.local/bin/pidag`. `/root/.local/bin` precedes `/projects/.local/bin` on PATH
   and will shadow it — an agent once "verified" a fix against a stale binary this way.
5. **Quality gate is `cargo clippy -p pidag -- -D warnings`**, not `--all-targets` (which has
   13 pre-existing test-file errors, out of scope — see `specs/SPEC-AUDIT-2026-08-10.md` P8).
6. **Report raw output, never summed totals.** Paste every `^test result:` line; the
   architect does the arithmetic. Totals have been misreported by summing truncated output.
7. **NEVER modify any file under `specs/`.** Specs are architect-owned — they are the
   contract being implemented, not an artefact of the implementation. A workhorse that can
   edit the spec can make any failure vanish by rewriting the requirement, which is the
   same failure mode as editing a test to fit the code. If a spec looks wrong, incomplete
   or self-contradictory, **stop and report it**; the architect amends it. Those reports
   are valuable — on spec-27 they surfaced both a missing `[[repeat]]` TOML fix and a
   silent prompt-parity regression.
8. **NO WORKHORSE MAY EVER `git commit`.** Leave all work in the working tree. Only the
   architect commits, and only **after** verifying the diff against the spec by reading it.
   Correspondingly, the architect **commits before dispatching** any workhorse, so the
   agent's changes are always reviewable as an isolated diff against a known-good point
   and can be reverted independently.

## Build discipline (learned 2026-08-11)

**Run ONE command, not four.** `bash deploy/scripts/quality-gate.sh .` already runs
`fmt --check`, `check`, `clippy -D warnings` and `test` in a single invocation. Enumerating
separate `cargo build` / `cargo test` / `cargo clippy` / `cargo build --release` steps
recompiles across profiles and wastes minutes per verification round. Only build `--release`
when a binary actually needs installing.

**Linking is the bottleneck, not compilation.** Since `pi_agent_rust` is linked in, each of
the ~33 test binaries is large, and *parallel* linking fails with
`linking with cc failed: exit status: 1` even with ample RAM and disk. Two fixes are in
place:
- `[profile.dev]`/`[profile.test]` use `debug = "line-tables-only"` — cut `target/debug`
  from **26G to ~8.5G** while keeping usable backtraces.
- The gate runs `cargo test -j 2`; bounded link parallelism is what makes the suite
  complete (`-j 1` also links cleanly, the default does not).

Verified green after both: **33 binaries, 396 passed, 0 failed, 1 ignored.**
