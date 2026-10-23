#!/usr/bin/env bash
# A nightly job's outcome for the report (P5.3, D-066), from what the report job can see of it:
# its result (success, failure, cancelled, skipped) and its outputs `started` (the run step began)
# and `outcome` (the run step's own verdict: passed, failed or infra).
#
# Usage: nightly-outcome.sh <result> <started> <outcome>
# Prints one of: passed, failed, infra, skip. Exit codes: 0 printed; 2 usage.
#   - the run step's own verdict wins;
#   - a run step that began and gave none didn't finish (a timeout, a hang, a kill): failed, since a
#     hang is a defect until shown otherwise;
#   - a job that never reached its run step failed in setup (checkout, toolchain, services): infra;
#   - a skipped job: skip.
set -euo pipefail

[ $# = 3 ] || { echo "usage: nightly-outcome.sh <result> <started> <outcome>" >&2; exit 2; }
result=$1 started=$2 outcome=$3
case "$outcome" in
    passed|failed|infra) echo "$outcome"; exit 0 ;;
    "") ;;
    *) echo "usage: unknown outcome '$outcome'" >&2; exit 2 ;;
esac
if [ "$result" = skipped ]; then
    echo skip
elif [ -n "$started" ]; then
    echo failed
elif [ "$result" = success ]; then
    # A job that succeeded without running its step has nothing to report: say so loudly.
    echo "error: $result with no run step" >&2
    echo infra
else
    echo infra
fi
