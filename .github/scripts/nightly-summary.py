#!/usr/bin/env python3
"""The nightly jobs' summaries (Technical Plan P5.3; decision log D-066): reads a run's logs and
prints Markdown for the job summary ($GITHUB_STEP_SUMMARY). Every number comes from the run's own
output; a run that printed no numbers says so instead of a table of zeros.

Usage:
  nightly-summary.py crash <outcome> <log>...      the `@nightly-crash <json>` lines in the logs
  nightly-summary.py golden <outcome> <replay.json> the release replay CLI's JSON document

<outcome> is the job's: passed, failed (a check failed: a defect) or infra (exit 3: a service or
the database stage wasn't ready, not a verdict on the code).
"""

import json
import sys

HEADLINE = {
    "passed": "passed",
    "failed": "**FAILED**: a check failed",
    "infra": "**INFRASTRUCTURE**: not ready (exit 3), no verdict on the code",
}


def crash(outcome, logs):
    nights = []
    for path in logs:
        try:
            with open(path, encoding="utf-8") as f:
                for line in f:
                    at = line.find("@nightly-crash ")
                    if at >= 0:
                        nights.append(json.loads(line[at + len("@nightly-crash "):]))
        except FileNotFoundError:
            pass
    out = [f"## Crash and retry: {HEADLINE[outcome]}", ""]
    out.append("Checked: the host killed at seeded points; every command retried with its key ends "
               "with exactly one outcome; every kill point was reached. Kills inside a "
               "multi-command turn are counted, not required.")
    out.append("")
    if not nights:
        out.append("No run reported its numbers (see the log).")
        return out
    out.append("| Store | Seeds | Commands | Lives | Kills inside a multi-command turn | Kills per point |")
    out.append("| --- | --- | --- | --- | --- | --- |")
    for n in nights:
        seen = n["seen"]
        seeds = f'{n["seed_base"]}..{n["seed_base"] + n["seeds"]}'
        kills = ", ".join(f"{p} {k}" for p, k in sorted(seen["kills"].items()))
        out.append(f'| {n["store"]} | {seeds} | {seen["commands"]} | {seen["lives"]} | '
                   f'{seen["mid_turn"]} | {kills} |')
    out.append("")
    out.append(f"Self-consistency replays: each seed's surviving log was verified on exit "
               f"({sum(n['seeds'] for n in nights)} logs).")
    return out


def golden(outcome, path):
    out = [f"## Golden replays: {HEADLINE[outcome]}", ""]
    out.append("Checked, in a release build: every golden replays exactly, every golden is what a "
               "fresh recording gives, and every golden verifies through the replay binary. "
               "Nothing was blessed or written.")
    out.append("")
    try:
        with open(path, encoding="utf-8") as f:
            doc = json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        out.append("The replay CLI's report is missing (see the log).")
        return out
    sources = doc.get("sources", [])
    records = sum(s.get("records", 0) for s in sources)
    out.append(f"Goldens: {len(sources)}, blessed hashes, release build: {doc.get('result')} "
               f"({records} records).")
    out.append("")
    out.append("| Golden | Result | Records | End hash |")
    out.append("| --- | --- | --- | --- |")
    for s in sources:
        out.append(f'| {s["source"]} | {s["result"]} | {s.get("records", "")} | '
                   f'`{s.get("end_hash", "")[:16]}` |')
    return out


def main(argv):
    if len(argv) < 4 or argv[1] not in ("crash", "golden") or argv[2] not in HEADLINE:
        print(__doc__, file=sys.stderr)
        return 2
    lines = crash(argv[2], argv[3:]) if argv[1] == "crash" else golden(argv[2], argv[3])
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
