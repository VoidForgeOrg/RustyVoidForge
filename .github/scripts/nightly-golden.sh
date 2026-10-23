#!/usr/bin/env bash
# The nightly's golden replays (Technical Plan P5.3; decision log D-040, D-041, D-066): the three
# golden checks every PR runs in debug, here in a release build (no overflow checks, other codegen):
# every golden replays exactly, every golden is what a fresh recording gives, and every golden
# verifies through the replay binary. Then the release binary's report for the summary. It never
# blesses or writes a golden: tests/golden must be unchanged afterwards. The CI nightly's `golden`
# job and `just nightly-golden` both run this script.
#
# Environment: NIGHTLY_LOG_DIR (default target/nightly/golden); GITHUB_STEP_SUMMARY, the job
# summary to append to (default: stdout).
# Exit codes: 0 passed; 1 a check failed (a defect, or the build failed).
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
log_dir="${NIGHTLY_LOG_DIR:-target/nightly/golden}"
mkdir -p "$log_dir"
rm -f "$log_dir/golden.log" "$log_dir/replay.json"
say() { echo "nightly-golden: $*" >&2; }
status=0

cargo nextest run --cargo-profile release -p voidforge-host --test golden --locked \
    -E 'test(=every_golden_replays_exactly) | test(=the_goldens_are_what_a_fresh_recording_gives)' \
    2>&1 | tee "$log_dir/golden.log"
[ "${PIPESTATUS[0]}" = 0 ] || status=1
cargo nextest run --cargo-profile release -p voidforge-replay --test cli --locked \
    -E 'test(=every_golden_verifies_through_the_binary)' 2>&1 | tee -a "$log_dir/golden.log"
[ "${PIPESTATUS[0]}" = 0 ] || status=1
# The release binary's own report: the numbers for the summary, and a fourth look at the set.
cargo run -q --release -p voidforge-replay --locked -- golden tests/golden --json \
    > "$log_dir/replay.json" 2>> "$log_dir/golden.log" || status=1

changed=$(git status --porcelain -- tests/golden)
if [ -n "$changed" ]; then
    say "tests/golden changed during the run (the nightly never blesses): $changed"
    status=1
fi

outcome=passed
[ "$status" = 0 ] || outcome=failed
# Locally the summary goes to stdout itself: reopening /dev/stdout would write at its own offset
# in a redirected log, under the shell's later lines.
if [ -n "${GITHUB_STEP_SUMMARY-}" ]; then
    python3 "$here/nightly-summary.py" golden "$outcome" "$log_dir/replay.json" >> "$GITHUB_STEP_SUMMARY"
else
    python3 "$here/nightly-summary.py" golden "$outcome" "$log_dir/replay.json"
fi
say "$outcome"
exit "$status"
