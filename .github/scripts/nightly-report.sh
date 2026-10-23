#!/usr/bin/env bash
# The nightly's report step (P5.3, D-066): each run kind's outcome (nightly-outcome.sh), then its
# issue (nightly-issue.sh). Only the kind a manual dispatch forced is reported as a forced test;
# every other kind in that dispatch reports to its real issue, failed or green (R2-3).
#
# Usage: nightly-report.sh <kind>...
# Environment: for each kind, its upper-case name holds "<result> <started|-> <outcome|->" from
# the job's result and outputs (for example CRASH="failure 1 failed"); FORCE_FAIL, the kind a
# dispatch forced (or none); RUN_URL; gh's own (GH_TOKEN, GH_REPO); GITHUB_STEP_SUMMARY.
# Exit codes: 0 every issue step done; 1 one failed.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
status=0
for kind in "$@"; do
    var=${kind^^}
    read -r result started verdict <<< "${!var}"
    [ "$started" = - ] && started=""
    [ "$verdict" = - ] && verdict=""
    outcome=$("$here/nightly-outcome.sh" "$result" "$started" "$verdict")
    echo "$kind: $outcome ($result)" | tee -a "${GITHUB_STEP_SUMMARY:-/dev/null}"
    [ "$outcome" = skip ] && continue
    forced=()
    [ "${FORCE_FAIL:-none}" = "$kind" ] && forced=(--forced)
    "$here/nightly-issue.sh" "${forced[@]}" "$kind" "$outcome" "$RUN_URL" || status=1
done
exit "$status"
