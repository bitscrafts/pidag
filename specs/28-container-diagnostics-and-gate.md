# pidag — spec-28: Container diagnostics tooling and a non-aborting quality gate

- **Project**: `/projects/pidag`
- **Crate**: `pidag` (deploy assets only — no Rust source changes)
- **Priority**: HIGH — process-inspection tooling is absent, which currently **blocks**
  the spec-18 R3 pool-reuse measurement outright
- **Status**: PLANNED
- **Depends-On**: none. Independent of spec-27; may land before or after it.

---

## Execution contexts — READ FIRST

This spec spans **three machines**. Every command below is tagged with where it runs.
Running a step in the wrong place is the most likely way to fail this spec.

**All work in this spec happens on `[LNX]` and `[CTR]`. The Mac checkout is STALE and is
explicitly out of scope — do not edit it, and do not sync from it.**

| tag | machine | path | how you get there |
|---|---|---|---|
| **[LNX]** | podman host `${DEPLOY_HOST}` | `/podman/PROJECTS/pidag-container` (`<REMOTE_DIR>`) | `ssh ${DEPLOY_HOST}`. **Authoritative for `Containerfile`.** Build context. |
| **[CTR]** | container `pidag-runner` | `/projects/pidag` | `podman exec -it pidag-runner /bin/bash` on [LNX]. **Authoritative for pidag source** (git `master`, `52b260e`). |
| ~~[MAC]~~ | workstation | `~/Private/PROJECTS/.../crates/pidag/` | **STALE. Out of scope.** Do not edit, do not `make deploy` from it. |

**Volume mapping** (`deploy/podman-compose.yml`): `[LNX] <REMOTE_DIR>/projects` →
`[CTR] /projects`, and `[LNX] <REMOTE_DIR>/pidag` → `[CTR] /opt/pidag-src` (read-only).
Both live on lnx, so everything below is an **lnx-local** operation. No Mac involvement, no
network sync.

### ⚠ Blocking prerequisite — refresh the build source, lnx-locally

`deploy/Containerfile:28-29` builds the binary from the **build context**:

```dockerfile
COPY pidag/Cargo.toml ./pidag/
COPY pidag/src ./pidag/src/
```

That is `<REMOTE_DIR>/pidag/src` — which is `[CTR] /opt/pidag-src`, a snapshot from
**2026-08-05/06 containing no `src/workflow/` directory at all**. The live source is
`<REMOTE_DIR>/projects/pidag` (= `[CTR] /projects/pidag`, commit `52b260e`, 2026-08-11,
holding specs 23, 25 and 26).

**A rebuild today would compile an Aug-6 `pidag`, omitting three specs, and report
success.** This is `CLAUDE.md` rule 4's stale-binary trap promoted to image level.

Both directories are on lnx, so the fix is a local copy — **no `make deploy`**:

```bash
# [LNX] refresh the build source from the live repo, then verify
cd /podman/PROJECTS/pidag-container
rsync -a --delete \
  --exclude='target/' --exclude='.git/' --exclude='_tmp/' --exclude='.pidag/' \
  projects/pidag/ pidag/
test -d pidag/src/workflow && echo "OK: src/workflow present"
```

**Never run `make deploy`** (and never `make build` if it implies a prior deploy): that
target rsyncs `[MAC]` → `[LNX] <REMOTE_DIR>/pidag/` with `--delete` and would overwrite the
freshly-refreshed tree with the stale Mac copy. Build with `podman-compose build` on lnx
directly.

**Follow-up, not blocking this spec**: `[CTR] /projects/pidag` has **no git remote** and is
unpushed — that history exists in exactly one place, on a volume at 85%. Worth pushing
before any of this. Once lnx is known-good, the Mac should be refreshed *from* lnx, not the
other way round.

---

## Overview

Two small deploy defects have each already cost real session time.

**D1 — the runtime image has no process tooling.** `ps`, `pgrep`, `pstree`, `top`, `free`,
`lsof` and `ss` are all missing (`deploy/Containerfile:57-68` installs no `procps`).
Consequences already observed:

- The 2026-08-10 session diagnosed 1854 zombie processes filling the PID table. Confirming
  the `init: true` fix on 2026-08-11 required hand-rolling an `awk` loop over `/proc/*/stat`
  because `ps -eo stat,comm` — the command written into the handoff as the *first thing to
  run* — does not exist in this image.
- **Handoff task #2 is currently impossible.** Measuring whether `PiBackend`'s client pool
  reuses processes is specified as "sample live `pi --mode rpc` PIDs during a bloodtest
  run", and the documented method uses `pgrep -f "mode rpc"`. Without `procps` there is no
  way to run it. spec-18 R3 cannot be verified until this lands.
- pidag's own unspecced child-process leak (children not `wait()`ed on kill/timeout) is a
  process-tree problem being investigated without `pstree`.

**D2 — the quality gate aborts on the first failing binary.**
`deploy/scripts/quality-gate.sh:53` runs `cargo test -j 2` with no `--no-fail-fast`. On
2026-08-11 this reported 10 test binaries / 96 passed / 2 failed and stopped — hiding the
other 23 binaries, including `tests/workflow_tests.rs`, the suite belonging to the very spec
under verification. The failure set looked small and local when it was neither. A gate that
truncates its own evidence is how spec-26 was accepted while broken.

---

## Requirements

### Functional

- **R1 (process tooling)**: the runtime image provides `ps`, `pgrep`, `pkill`, `top`, `free`
  (package `procps`), `pstree`, `killall` (`psmisc`), and `lsof`.
- **R2 (network tooling)**: the runtime image provides `ss` and `ip` (`iproute2`), for
  inspecting the UI on `:4601` and the RPC/MCP listeners.
- **R3 (general tooling)**: the runtime image provides `file`, `tree`, `bc`, `unzip`.
- **R4 (gate completeness)**: `cargo test` in `deploy/scripts/quality-gate.sh` runs with
  `--no-fail-fast`, so every test binary executes and the complete failure set is reported
  in one pass.
- **R5 (gate still fails loudly)**: adding `--no-fail-fast` must **not** change the script's
  exit status contract. A non-zero `cargo test` exit still fails the gate.
- **R6 (a separate, cache-friendly layer)**: the packages go in their **own** `RUN`,
  inserted immediately **after** the existing runtime `apt-get` block (i.e. after
  `deploy/Containerfile:68`) and before the `uv` install. It must use
  `--no-install-recommends` and carry its own `rm -rf /var/lib/apt/lists/*` in the same
  `RUN`, so no apt cache is committed to the layer.
  **Do NOT append to the existing `RUN` at line 55.** That block also runs
  `rustup component add clippy rustfmt` and `cargo install redbcli`; editing its command
  string invalidates the layer and forces a `redbcli` source recompile on every subsequent
  tooling change. A separate later layer leaves it cached.
- **R7 (builder stage untouched)**: only the **runtime** stage
  (`deploy/Containerfile:52+`) gains packages. The builder stage is not modified.

### Non-Functional

- **N1**: No Rust source changes. `git diff --stat` must show only `deploy/`.
- **N2**: `strace` is deliberately **excluded** — it is useless without
  `--cap-add=SYS_PTRACE`, which is a separate security decision for
  `deploy/podman-compose.yml` and is out of scope. Record it as a known gap; do not add the
  capability.
- **N3**: Image size growth is expected and acceptable, but must be measured and reported
  (see Exit Criteria) rather than assumed negligible.

---

## Architecture

No architecture to speak of — this is two edits to deploy assets. The one decision worth
recording:

**Packages go in a new `RUN` layer, deliberately not appended to the existing one.** The
runtime stage (`deploy/Containerfile:55-68`) fuses four expensive things into one `RUN`:
`rustup component add clippy rustfmt`, `cargo install redbcli` (a from-source compile),
`apt-get install`, and the cache cleanup. Appending a package name to that list changes the
layer's command string, invalidating it — so adding `ps` would rebuild `redbcli` from
source, and would do so again on every future tooling adjustment.

A separate `RUN` inserted after it costs one extra layer and nothing else: because the
`rm -rf /var/lib/apt/lists/*` runs *within* that same `RUN`, no apt cache is committed, so
the "extra cache layer" objection does not apply. Diagnostic tooling changes on a different
cadence than the toolchain and belongs in its own layer.

**Package-to-tool mapping** (Debian trixie), for the implementer's reference:

| package | provides | why it is needed here |
|---|---|---|
| `procps` | `ps`, `pgrep`, `pkill`, `top`, `free` | zombie/PID-table diagnosis; spec-18 R3 pool sampling |
| `psmisc` | `pstree`, `killall`, `fuser` | seeing the `pidag → pi → sh` tree for the child-process leak |
| `lsof` | `lsof` | which process holds the redb lock / leaked fds |
| `iproute2` | `ss`, `ip` | UI and RPC listener inspection |
| `file`, `tree`, `bc`, `unzip` | — | general diagnosis ergonomics |

---

## TDD Contract

This spec has no unit tests — it changes an image and a shell script. Verification is by
the executable checks below, run **inside a container built from the modified
Containerfile**. Each row is a named check the implementer must run and report output for.

| id | check | given | expects |
|----|-------|-------|---------|
| T1 | `command -v ps pgrep pkill top free` | rebuilt runtime image | all five resolve to a path; exit 0 |
| T2 | `command -v pstree killall lsof` | rebuilt runtime image | all three resolve; exit 0 |
| T3 | `command -v ss ip file tree bc unzip` | rebuilt runtime image | all six resolve; exit 0 |
| T4 | `ps -eo stat,comm \| grep -c '^Z'` | rebuilt runtime image | runs without error and prints a count (the handoff's documented first command works) |
| T5 | `grep -n 'no-fail-fast' deploy/scripts/quality-gate.sh` | edited script | matches the `cargo test` line |
| T6 | `bash deploy/scripts/quality-gate.sh .` on a tree with a known-failing test | spec-27 not yet applied, or a deliberately broken test | **33+** `^test result:` lines appear, not 10; gate exits non-zero (R5) |
| T7 | `podman images` before/after | rebuilt image | size delta recorded and reported as a number |

---

## Exit Criteria

Each block is tagged with the machine it runs on. `$DEPLOY` is the `deploy/` directory of
whichever checkout that machine holds.

```bash
# ---------- [LNX] ssh ${DEPLOY_HOST} ----------
cd /podman/PROJECTS/pidag-container

# PREREQUISITE — build source refreshed from the live repo
test -d pidag/src/workflow
diff -rq --exclude=target --exclude=.git --exclude=_tmp --exclude=.pidag \
     projects/pidag/src pidag/src

# R1-R3 — packages declared in the build-context Containerfile (the authoritative one)
grep -n 'procps'   Containerfile
grep -n 'psmisc'   Containerfile
grep -n 'lsof'     Containerfile
grep -n 'iproute2' Containerfile

# R6/R7 — own layer, cleanup intact, redbcli layer untouched
[ "$(grep -c 'apt-get install' Containerfile)" = "3" ]   # builder, runtime, diagnostics
[ "$(grep -c 'rm -rf /var/lib/apt/lists/\*' Containerfile)" = "3" ]
grep -n 'cargo install redbcli' Containerfile            # must be unchanged
! grep -n 'strace' Containerfile                          # N2

# T7 — image size before/after
podman images --format '{{.Repository}}:{{.Tag}} {{.Size}}' | grep pidag-runner

# Build WITHOUT make deploy (which would restore the stale Mac tree)
podman-compose build
```

```bash
# ---------- [CTR] podman exec -it pidag-runner /bin/bash ----------
# R4 — the gate the container actually executes (live repo copy)
grep -q 'no-fail-fast' /projects/pidag/deploy/scripts/quality-gate.sh

# T1-T4 — tooling present after rebuild
command -v ps pgrep pkill top free
command -v pstree killall lsof
command -v ss ip file tree bc unzip
ps -eo stat,comm | grep -c '^Z'

# T6 — the gate reports the complete failure set
cd /projects/pidag && bash deploy/scripts/quality-gate.sh . 2>&1 | grep -cE '^test result:'
```

**Prose criteria**:

0. **The source refresh happened before the build, and is reported.** State that
   `<REMOTE_DIR>/pidag/src/workflow/` exists after the rsync and that
   `diff -rq projects/pidag/src pidag/src` is clean. A rebuild performed without this is a
   spec failure even if every other check passes — the image would ship an Aug-6 `pidag`
   missing specs 23, 25 and 26, and would report success.
0b. **`make deploy` was not run at any point.** Confirm explicitly. It would restore the
   stale Mac tree over the refreshed one.
1. A container built from the modified `deploy/Containerfile` resolves every tool in T1-T3.
   Report the raw `command -v` output, not a summary claim.
2. `bash deploy/scripts/quality-gate.sh .` on a tree with at least one failing test emits
   **every** test binary's `^test result:` line — 33 or more — and still exits non-zero.
   Paste every line raw; do not sum them.
3. The image size delta is reported as an explicit before/after number in MB.
4. `git diff --stat` shows changes only under `deploy/`.

---

## Guardrails

- **G1 — never modify `/projects/_upstream/`.** Read-only reference on the user's own fork
  and active branch.
- **G2 — no `Co-Authored-By:` trailer; no mention of Claude, Anthropic or any AI tool in a
  commit message.** Harnesses append this by default; suppress it.
- **G2b — never run `make deploy`, and never edit the Mac checkout.** The Mac tree is stale
  (Aug-6). `make deploy` rsyncs Mac → `<REMOTE_DIR>/pidag/` with `--delete` and would
  silently undo the source refresh, producing an image without specs 23/25/26. Build with
  `podman-compose build` on lnx. If the Mac is ever reconciled, it must be refreshed
  **from** lnx, not the reverse.
- **G3 — do not touch the builder stage** (`deploy/Containerfile:16-51`). Adding diagnostic
  tools there slows every build and helps nothing (R7).
- **G4 — do not add `strace`, and do not add `SYS_PTRACE` to
  `deploy/podman-compose.yml`.** That is a separate security decision (N2).
- **G5 — do not change `init: true` or `pids_limit: 4096`** in `deploy/podman-compose.yml`.
  They fixed the 2026-08-10 PID exhaustion and are verified working (`pids.current` 160,
  0 zombies, PID 1 = `podman-init`).
- **G6 — do not "fix" tests.** If `--no-fail-fast` reveals additional failures, that is the
  *point* of this spec. **Report them; do not repair them here.** They belong to spec-27 or
  a new spec. Silently fixing revealed failures inside a deploy change hides the very
  signal being bought.
- **G7 — do not remove or reorder existing packages** in the runtime `apt-get install`
  list. Append only.
- **G8 — do not `rm -rf` any `.pidag/` directory** while testing the gate.
  `mv .pidag .pidag.prev-$(date +%H%M%S)`.
- **G9 — report raw output, never summed totals.**

### Error handling expectations

- If the image build fails on a package name, report the raw `apt-get` error and the exact
  failing package. Do not silently drop a package to make the build pass — a missing
  `procps` is the whole reason this spec exists.
- If `--no-fail-fast` makes the suite exceed the harness timeout, report the timeout rather
  than reverting the flag; the fix is bounded parallelism, not less evidence.

---

## Files to Modify

| ctx | File | Change |
|---|------|--------|
| **[LNX]** | `<REMOTE_DIR>/pidag/` | refresh from `<REMOTE_DIR>/projects/pidag/` (lnx-local rsync, prerequisite above) |
| **[LNX]** | `<REMOTE_DIR>/Containerfile` | insert a **new** `RUN` after the runtime `apt-get` block installing `procps psmisc lsof iproute2 file tree bc unzip` with `--no-install-recommends` and its own list cleanup. Leave the `rustup`/`redbcli` block byte-identical (R6). **This is the authoritative copy.** |
| **[CTR]** | `/projects/pidag/deploy/Containerfile` | same edit, committed to the live repo so the change is under version control |
| **[CTR]** | `/projects/pidag/deploy/scripts/quality-gate.sh` | `--no-fail-fast` — **already applied 2026-08-11**; this is the copy the container executes |

**Note the asymmetry**: the `--no-fail-fast` fix went live the moment the `[CTR]` copy was
edited, because the gate script is read from the mounted volume at run time. The
`Containerfile` change does nothing until the image is rebuilt — which is gated on the
source-refresh prerequisite above. The Mac copy of both files is stale and is left alone.

**Explicitly not modified**: `deploy/podman-compose.yml` (G4, G5), any Rust source, any
test.

## Memory

Store on completion: `workspace/specs/pidag-28-container-diagnostics-and-gate`,
`claude-pi-delegation/fix/20260811-procps-missing-blocks-pool-measurement`.
