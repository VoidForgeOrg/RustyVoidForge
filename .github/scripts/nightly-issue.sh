#!/usr/bin/env bash
# The nightly's issue on failure (Technical Plan P5.3; decision log D-066). One open issue per run
# kind and outcome, labelled nightly-failure: a failure comments on the open one with the same
# title, or opens it; a pass comments "green again" on any open one for its kind and never closes
# it (people read the gate's week). Run by the nightly's `report` job, never for a pull request.
#
# Usage: nightly-issue.sh [--forced] <kind> <outcome> <run-url>
#   kind     sim, crash or golden
#   outcome  passed; failed (a check failed: a defect); infra (exit 3: not ready, no verdict)
#   --forced a forced failure (workflow_dispatch's force_fail): its own titles, "Nightly (forced
#            test): …", so a real failure never lands on a test issue, and the reverse
# Environment: gh's (GH_TOKEN, GH_REPO); GITHUB_SHA, the commit (default: git's HEAD).
# Exit codes: 0 done; 1 gh failed; 2 usage.
set -euo pipefail

usage() { echo "usage: nightly-issue.sh [--forced] <sim|crash|golden> <passed|failed|infra> <run-url>" >&2; exit 2; }
prefix="Nightly"
if [ "${1-}" = --forced ]; then
    prefix="Nightly (forced test)"
    shift
fi
[ $# = 3 ] || usage
kind=$1 outcome=$2 run=$3
case "$kind" in sim|crash|golden) ;; *) usage ;; esac
case "$outcome" in passed|failed|infra) ;; *) usage ;; esac
[ -n "$run" ] || usage

label=nightly-failure
failed_title="$prefix: $kind failed"
infra_title="$prefix: $kind infrastructure not ready"
sha="${GITHUB_SHA:-$(git rev-parse HEAD)}"
when="$(date -u +%Y-%m-%dT%H:%MZ)"

# The open issue whose title is exactly $1, or nothing. Only open issues: a closed one is history.
open_issue() {
    gh issue list --state open --label "$label" --limit 200 --json number,title \
        | python3 -c 'import json, sys; t = sys.argv[1]; print(next((str(i["number"]) for i in json.load(sys.stdin) if i["title"] == t), ""))' "$1"
}

if [ "$outcome" = passed ]; then
    for title in "$failed_title" "$infra_title"; do
        number=$(open_issue "$title")
        if [ -n "$number" ]; then
            gh issue comment "$number" --body "Green again on $run ($when, $sha). Not closed: closing is a person's call."
            echo "commented green again on #$number ($title)"
        fi
    done
    exit 0
fi

if [ "$outcome" = failed ]; then
    title=$failed_title
    meaning="A check failed: a defect until shown otherwise. Rerunning to green is not a fix."
else
    title=$infra_title
    meaning="Infrastructure wasn't ready (exit 3): a service or the database stage didn't start, so the night has no verdict on the code."
fi
detail="$meaning

- Run: $run
- Commit: $sha
- When: $when"
number=$(open_issue "$title")
if [ -n "$number" ]; then
    gh issue comment "$number" --body "Again. $detail"
    echo "commented on #$number ($title)"
else
    gh label create "$label" --color B60205 --force \
        --description "A nightly run failed (P5.3, D-066)" > /dev/null
    gh issue create --title "$title" --label "$label" --body "$detail

The workflow comments here on each later failure of this kind, and \"green again\" when it passes; it never closes this issue."
fi
