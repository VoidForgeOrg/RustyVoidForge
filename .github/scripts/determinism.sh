#!/usr/bin/env bash
# The determinism suite (Technical Plan P1.5; decision log D-024), run by `just determinism` and
# the CI job of that name. It runs the suite, then the light two-process double run: one process
# writes the canonical postcard bytes of fixed scenarios, a second process runs them again and
# requires identical bytes. A missing file, or no test matched, fails.
set -euo pipefail

cargo nextest run -p voidforge-testkit --test determinism --locked

target_dir="${CARGO_TARGET_DIR:-target}"
mkdir -p "$target_dir/determinism"
file="$(cd "$target_dir/determinism" && pwd)/double-run.bin"
rm -f "$file"

for mode in write compare; do
    VOIDFORGE_DOUBLE_RUN="$mode" VOIDFORGE_DOUBLE_RUN_FILE="$file" \
        cargo nextest run -p voidforge-testkit --test determinism --locked \
        --run-ignored only -E 'test(=two_process_double_run)'
done
echo "ok: the two processes produced identical bytes ($(wc -c < "$file") bytes)"
