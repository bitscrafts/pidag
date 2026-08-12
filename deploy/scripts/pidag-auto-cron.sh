#!/usr/bin/env bash
# pidag-auto-cron.sh — one autonomous-drive pass, safe to run from cron (30min).
#
# Guardrail: uses `pidag auto --lock` for single-flight. If a previous pass is
# still running (or crashed without releasing its lock), the driver refuses to
# start rather than colliding. A crashed process's lock is auto-reclaimed on the
# next pass (stale-pid detection), so a hung run never blocks the schedule
# forever.
#
# Install:   crontab -e   (or drop a systemd timer)
#   */30 * * * * /projects/pidag/deploy/scripts/pidag-auto-cron.sh >> /var/log/pidag-auto.log 2>&1
#
# Override the pidag binary / workspace via env:
#   PIDAG_BIN  (default: pidag on PATH)
#   WORKSPACE  (default: /projects)
# The `--lock` path is derived from WORKSPACE/.pidag/auto.lock (single global
# lock for the workspace, so no two passes touch any project simultaneously).

set -u

PIDAG_BIN="${PIDAG_BIN:-pidag}"
WORKSPACE="${WORKSPACE:-/projects}"
LOG="${PIDAG_AUTO_LOG:-/var/log/pidag-auto.log}"

echo "== pidag-auto-cron $(date -Is) =="
"$PIDAG_BIN" auto --workspace "$WORKSPACE" --lock "$WORKSPACE/.pidag/auto.lock"

rc=$?
if [ "$rc" -ne 0 ]; then
    echo "pidag auto pass exited rc=$rc (see log tail)."
    tail -n 20 "$LOG" 2>/dev/null || true
fi
exit "$rc"
