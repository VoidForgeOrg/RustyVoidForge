#!/usr/bin/env bash
# Regression cases for nightly-issue.sh (P5.3, D-066), against a stub gh on PATH that serves a fixed
# set of issues and records every call. Nothing reaches GitHub.
set -euo pipefail

script="$(cd "$(dirname "$0")" && pwd)/nightly-issue.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir "$work/bin"
# The stub: `issue list` prints $STUB_ISSUES filtered by --state like gh (open only with --state
# open; everything without it), so a script that forgets the state would see closed issues.
cat > "$work/bin/gh" <<'EOF'
#!/usr/bin/env bash
# One line per call: a multi-line body is folded onto it.
echo "$*" | tr '\n' ' ' >> "$STUB_CALLS"
echo >> "$STUB_CALLS"
if [ "$1 $2" = "issue list" ]; then
    state=all
    prev=""
    for a in "$@"; do [ "$prev" = --state ] && state=$a; prev=$a; done
    python3 -c 'import json, sys; s = sys.argv[1]
print(json.dumps([{"number": i["number"], "title": i["title"]} for i in json.loads(sys.argv[2]) if s == "all" or i["state"] == s]))' "$state" "$STUB_ISSUES"
fi
EOF
chmod +x "$work/bin/gh"
export PATH="$work/bin:$PATH" STUB_CALLS="$work/calls" GITHUB_SHA=abc123
run_url=https://github.com/o/r/actions/runs/1
failures=0

# case <name> <issues json> <expected calls, one per line, as patterns: grep -E, anchored> -- args
case_() {
    local name=$1 issues=$2 want=$3
    shift 3
    : > "$STUB_CALLS"
    local status=0
    STUB_ISSUES="$issues" "$script" "$@" > /dev/null 2>&1 || status=$?
    local got
    got=$(grep -v '^issue list' "$STUB_CALLS" | sed 's/ --body .*//; s/ *$//' || true)
    if [ "$got" = "$want" ] && [ "$status" = 0 ]; then
        echo "pass: $name"
    else
        echo "FAIL: $name (exit $status)"
        echo "  want: $(echo "$want" | tr '\n' '|')"
        echo "  got:  $(echo "$got" | tr '\n' '|')"
        failures=$((failures + 1))
    fi
}

none='[]'
open_crash='[{"number": 7, "title": "Nightly: crash failed", "state": "open"}]'
closed_crash='[{"number": 3, "title": "Nightly: crash failed", "state": "closed"}]'
open_golden='[{"number": 8, "title": "Nightly: golden failed", "state": "open"}]'
open_infra='[{"number": 9, "title": "Nightly: crash infrastructure not ready", "state": "open"}]'
open_forced='[{"number": 11, "title": "Nightly (forced test): crash failed", "state": "open"}]'
create_crash='label create nightly-failure --color B60205 --force --description A nightly run failed (P5.3, D-066)
issue create --title Nightly: crash failed --label nightly-failure'

case_ "a failure with no open issue opens one" "$none" "$create_crash" crash failed "$run_url"
case_ "a second failure comments on the open one" "$open_crash" "issue comment 7" crash failed "$run_url"
case_ "a closed issue is history: a new one opens" "$closed_crash" "$create_crash" crash failed "$run_url"
case_ "another kind's open issue is not this one" "$open_golden" "$create_crash" crash failed "$run_url"
case_ "infrastructure has its own title" "$open_crash" "label create nightly-failure --color B60205 --force --description A nightly run failed (P5.3, D-066)
issue create --title Nightly: crash infrastructure not ready --label nightly-failure" crash infra "$run_url"
case_ "infrastructure again comments on its own issue" "$open_infra" "issue comment 9" crash infra "$run_url"
case_ "a pass comments green again on each open issue of its kind, never closes" \
    "$(python3 -c 'import json,sys; print(json.dumps(json.loads(sys.argv[1]) + json.loads(sys.argv[2])))' "$open_crash" "$open_infra")" \
    "issue comment 7
issue comment 9" crash passed "$run_url"
case_ "a pass with no open issue does nothing" "$open_golden" "" crash passed "$run_url"
case_ "a forced failure has its own title" "$open_crash" "label create nightly-failure --color B60205 --force --description A nightly run failed (P5.3, D-066)
issue create --title Nightly (forced test): crash failed --label nightly-failure" --forced crash failed "$run_url"
case_ "a second forced failure comments on the forced issue" "$open_forced" "issue comment 11" --forced crash failed "$run_url"
case_ "a real failure never lands on a forced issue" "$open_forced" "$create_crash" crash failed "$run_url"

# The body names the run and the commit.
: > "$STUB_CALLS"
STUB_ISSUES="$open_crash" "$script" crash failed "$run_url" > /dev/null
if grep -q "issue comment 7 --body .*Run: $run_url .*Commit: abc123" "$STUB_CALLS"; then
    echo "pass: the comment names the run and the commit"
else
    echo "FAIL: the comment names the run and the commit: $(cat "$STUB_CALLS")"
    failures=$((failures + 1))
fi

for bad in "nope failed $run_url" "crash broken $run_url" "crash failed"; do
    status=0
    # shellcheck disable=SC2086
    STUB_ISSUES='[]' "$script" $bad > /dev/null 2>&1 || status=$?
    if [ "$status" = 2 ]; then echo "pass: usage refused ($bad)"; else echo "FAIL: usage ($bad) exit $status"; failures=$((failures + 1)); fi
done

# nightly-report.sh: a dispatch that forces one kind reports only that kind as a forced test (R2-3).
report="$(cd "$(dirname "$0")" && pwd)/nightly-report.sh"
report_case() {
    local name=$1 issues=$2 want=$3
    shift 3
    : > "$STUB_CALLS"
    local status=0
    env "$@" STUB_ISSUES="$issues" RUN_URL="$run_url" GITHUB_STEP_SUMMARY=/dev/null \
        "$report" crash golden > /dev/null 2>&1 || status=$?
    local got
    got=$(grep -E '^issue (create|comment)' "$STUB_CALLS" | sed 's/ --body .*//; s/ --label .*//; s/ *$//' || true)
    if [ "$got" = "$want" ] && [ "$status" = 0 ]; then
        echo "pass: $name"
    else
        echo "FAIL: $name (exit $status)"
        echo "  want: $(echo "$want" | tr '\n' '|')"
        echo "  got:  $(echo "$got" | tr '\n' '|')"
        failures=$((failures + 1))
    fi
}
report_case "forcing crash leaves golden's real failure on its real issue" "$none" \
    "issue create --title Nightly (forced test): crash failed
issue create --title Nightly: golden failed" \
    FORCE_FAIL=crash CRASH="failure 1 failed" GOLDEN="failure 1 failed"
report_case "forcing crash still says green again on golden's real issue" "$open_golden" \
    "issue create --title Nightly (forced test): crash failed
issue comment 8" \
    FORCE_FAIL=crash CRASH="failure 1 failed" GOLDEN="success 1 passed"
report_case "a dispatch that forces nothing reports every kind for real" "$none" \
    "issue create --title Nightly: crash failed
issue create --title Nightly: golden infrastructure not ready" \
    FORCE_FAIL=none CRASH="failure 1 failed" GOLDEN="failure - -"
report_case "a skipped kind reports nothing" "$none" "" \
    FORCE_FAIL=none CRASH="skipped - -" GOLDEN="success 1 passed"

# nightly-outcome.sh: what the report makes of each job.
outcome="$(cd "$(dirname "$0")" && pwd)/nightly-outcome.sh"
while read -r result started verdict want; do
    [ "$started" = - ] && started=""
    [ "$verdict" = - ] && verdict=""
    got=$("$outcome" "$result" "$started" "$verdict" 2> /dev/null)
    if [ "$got" = "$want" ]; then
        echo "pass: outcome of $result/${started:-unstarted}/${verdict:-none} is $want"
    else
        echo "FAIL: outcome of $result/${started:-unstarted}/${verdict:-none}: want $want, got $got"
        failures=$((failures + 1))
    fi
done <<'CASES'
success 1 passed passed
failure 1 failed failed
failure 1 infra infra
failure 1 - failed
cancelled 1 - failed
failure - - infra
cancelled - - infra
skipped - - skip
CASES

if [ "$failures" != 0 ]; then
    echo "$failures case(s) failed"
    exit 1
fi
echo "ok: every nightly-issue case passed"
