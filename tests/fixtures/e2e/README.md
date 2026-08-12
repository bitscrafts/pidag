# End-to-end pipeline fixtures

Specs that exercise pidag as a **product** — `spec → split → DAG → implementation`
producing real software — rather than as an engine.

They exist because on 2026-08-12 an audit found that pidag had never been shown to
generate software. The only spec it had ever executed was an 18-line fixture whose
entire deliverable was `touch FIXED.txt`, and `pidag split` had never been run at
all. Every "green bloodtest" until then proved gate *mechanics*, not capability.

## The fixtures

| file | criteria | proves |
|---|---|---|
| `01-csv-stats.md` | 4 | single-spec path, and the **success** branch: validate passes on iteration 1, so iterations 2 and 3 are skipped |
| `02-logtool-split.md` | 15 | the **split** path: >7 criteria auto-splits into `ceil(n/6)` = 3 parts |
| `_tmp/bug-a-bloodtest/01-blood-test.md` | 1 | the **failure** branch: validate fails, the gate fires, the fix node runs |

The three are complementary. The bloodtest alone only ever exercised the failure
path; `01-csv-stats.md` was the first run to exercise the success path.

## Running one

```bash
# 1. The binary must match the tree. `pidag sdd --run` used to resolve `pidag`
#    via PATH (audit S-7, fixed 00c4a77) -- a stale install silently ran old code
#    and produced a meaningless green. It now uses current_exe(), but a rebuild is
#    still needed for the binary to contain your changes.
cargo build --release -p pidag
install -m755 target/release/pidag /root/.local/bin/pidag
install -m755 target/release/pidag /projects/.local/bin/pidag

# 2. Copy the spec into a FRESH directory that is ITS OWN GIT REPO.
mkdir -p _tmp/trial && cd _tmp/trial
git init -q . && git config user.email t@local && git config user.name t
printf '.pidag/\n*.log\n' > .gitignore
mkdir -p specs && cp ../../tests/fixtures/e2e/01-csv-stats.md specs/
git add -A && git commit -qm "spec only"

# 3. Split first if the spec has more than 7 criteria.
pidag split specs/01-csv-stats.md --auto

# 4. Run.
PIDAG_AGENT_BACKEND=pi pidag sdd specs/01-csv-stats.md --run --fresh \
    --model deepseek-v4-flash --project-root .
```

## Two prerequisites that will waste a session if missed

**The project root must be its own git repository.** The `verify_pre`/`verify` pair
added by spec-31 hashes `git status --porcelain` plus content, so a directory that is
*gitignored by an outer repo* reports **zero** changes — every implement node then
fails verify for reasons unrelated to pidag. `_tmp/` is gitignored by the pidag repo,
so `git init` inside the trial directory is mandatory.

**Start from a clean tree with the deliverables absent.** If the artifacts already
exist, `validate-baseline` and `validate-iter1` pass immediately, the gate never
fires, and the run reports success while demonstrating nothing.

## What a good result looks like

Success path (`01-csv-stats.md`):

```
NodeFailed     validate-baseline     <- correct: no code yet
NodeDone       implement-iter1       <- writes the software
NodeDone       quality-gate-1
NodeDone       validate-iter1        <- exit criteria pass
NodeDone       implement-iter2       <- SKIPPED: no NodeDispatched
NodeDone       implement-iter3       <- SKIPPED
```

`implement-iterN` appearing as `NodeDone` with **no preceding `NodeDispatched`** is
the signature of a skip. Verify the software independently afterwards — run the
spec's exit criteria yourself rather than trusting the validator, which is the same
discipline that caught three inert "fixes" during the 2026-08-12 session.

## Known defect in the split path (2026-08-12)

`--auto` on `02-logtool-split.md` distributes criteria **without module coherence**:
part1 receives assertions about `logfilter` and `logreport`, while the files for
those modules are created by parts 2 and 3. Child specs therefore carry undeclared
cross-part dependencies and may not be runnable standalone or in the order produced.
`pidag split` emits no ordering or dependency information between the children.
