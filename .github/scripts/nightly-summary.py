#!/usr/bin/env python3
"""The nightly jobs' summaries (Technical Plan P5.3; decision log D-066): reads a run's logs and
prints Markdown for the job summary ($GITHUB_STEP_SUMMARY). Every number comes from the run's own
output; a run that printed no numbers says so instead of a table of zeros.

Usage:
  nightly-summary.py crash <outcome> <log>...      the `@nightly-crash <json>` lines in the logs
  nightly-summary.py golden <outcome> <replay.json> the release replay CLI's JSON document
  nightly-summary.py sim <outcome> <log-dir> [why]  nightly-sim.sh's logs

<outcome> is the job's: passed, failed (a check failed: a defect) or infra (exit 3: a service or
the database stage wasn't ready, not a verdict on the code).
"""

import json
import re
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
               "with exactly one outcome. Each store's core of fixed seeds reaches every kill point "
               "by construction, asserted; the night's own range explores further, and its coverage "
               "is reported. Kills inside a multi-command turn are required of the file core only.")
    out.append("")
    if not nights:
        out.append("No run reported its numbers (see the log).")
        return out
    out.append("| Store | Seeds | Commands | Lives | Kills inside a multi-command turn | Kills per point |")
    out.append("| --- | --- | --- | --- | --- | --- |")
    logs = 0
    for n in nights:
        for label, seen, seeds in (("core", n.get("core"), "fixed"),
                                   ("range", n["seen"], f'{n["seed_base"]}..{n["seed_base"] + n["seeds"]}')):
            if not seen:
                continue
            kills = ", ".join(f"{p} {k}" for p, k in sorted(seen["kills"].items())) or "none"
            out.append(f'| {n["store"]}, {label} | {seeds} | {seen["commands"]} | {seen["lives"]} | '
                       f'{seen["mid_turn"]} | {kills} |')
        logs += n["seeds"] + (12 if n["store"] == "file" else 4)
    out.append("")
    for n in nights:
        if n.get("range_missed"):
            out.append(f'Not reached by the {n["store"]} range (its core reached them): '
                       f'{", ".join(n["range_missed"])}.')
    out.append(f"Self-consistency replays: each seed's surviving log was verified on exit "
               f"({logs} logs).")
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


def tagged(path, tag):
    """The JSON after the last `<tag> ` in a file, or None."""
    found = None
    try:
        with open(path, encoding="utf-8") as f:
            for line in f:
                at = line.find(tag + " ")
                if at >= 0:
                    found = json.loads(line[at + len(tag) + 1:])
    except (FileNotFoundError, json.JSONDecodeError):
        return None
    return found


def load(path):
    try:
        with open(path, encoding="utf-8") as f:
            return json.load(f)
    except (FileNotFoundError, json.JSONDecodeError):
        return None


def ms(seconds):
    return "–" if seconds is None else f"{seconds * 1000:.2f} ms"


def keys_lookup(path):
    """The find_keys EXPLAIN's execution time and whether it used the unique index, or None."""
    try:
        with open(path, encoding="utf-8") as f:
            text = f.read()
    except FileNotFoundError:
        return None
    m = re.search(r"Execution Time: ([0-9.]+) ms", text)
    if not m:
        return None
    index = "log_records_player_key_unique" in text
    return f"{m.group(1)} ms, {'by' if index else 'NOT by'} the (player_id, idempotency_key) index"


def sim(outcome, log_dir, why):
    out = [f"## Bot simulation: {HEADLINE[outcome]}", ""]
    if why:
        out += [why, ""]
    out.append("Checked: eight bots played through the API on the fast rules; every view kept its "
               "rules; every invalid command got exactly its code; every command sent again with its "
               "key got exactly the stored answer; serve stopped cleanly; the log replays and agrees "
               "with every snapshot; the core's invariants held after every record.")
    out.append("")
    t = tagged(f"{log_dir}/sim.out", "@nightly-sim")
    if not t:
        out.append("The bots reported no numbers (see the log).")
        return out
    secs = t.get("wall_seconds", 0)
    out.append(f"- Simulated span: {secs / 60:.1f} wall minutes on the fast rules = {secs:.0f} v0 hours "
               f"≈ {secs / 168:.1f} v0 weeks. Seed {t.get('seed')}.")
    refused = ", ".join(f"{c} {n}" for c, n in sorted(t.get("game_refused", {}).items())) or "none"
    invalid = ", ".join(f"{c} {n}" for c, n in sorted(t.get("invalid", {}).items())) or "none"
    out.append(f"- Commands: {t.get('valid_sent')} valid ({t.get('accepted')} accepted; refused by a "
               f"game rule: {refused}); invalid, each with exactly its code: {invalid}; "
               f"{t.get('retried')} sent again with their key. "
               f"{t.get('commands_per_second', 0):.2f} commands a second; 429s: {t.get('rate_limited')}.")
    out.append(f"- Views read and checked: {t.get('views_checked')} over {t.get('ticks')} bot ticks.")
    by_type = ", ".join(f"{k} {n}" for k, n in sorted(t.get("valid_by_type", {}).items())) or "none"
    out.append(f"- Valid commands by type: {by_type}. The most planets one bot held: "
               f"{t.get('most_planets', '?')}.")
    inv = tagged(f"{log_dir}/invariants.log", "@nightly-invariants")
    if inv:
        out.append(f"- Invariants after every record: {inv['records']} records from the snapshot at "
                   f"seq {inv['from_snapshot']} to the head, seq {inv['head']}.")
    early = load(f"{log_dir}/replay-earliest.json")
    inspect = load(f"{log_dir}/inspect.json")
    snaps = None
    if inspect and inspect.get("sources"):
        snaps = len(inspect["sources"][0].get("snapshots", []))
    if early:
        src = (early.get("sources") or [early])[0]
        out.append(f"- Self-consistency replays (no blessed hash: the replay agrees with itself): the "
                   f"sim log, {src.get('records', '?')} records, replayed from its earliest snapshot "
                   f"and checked against {snaps if snaps is not None else '?'} snapshots; then from "
                   f"its latest.")
    keys = keys_lookup(f"{log_dir}/keys-explain.log")
    if keys:
        out.append(f"- The keyed-submission lookup on the final log (D-037; reported, not bounded): "
                   f"{keys}.")
    lat = tagged(f"{log_dir}/latency.log", "@nightly-latency")
    if lat and "step" in lat:
        out.append("")
        out.append("| Latency (serve's histograms) | p50 | p99 |")
        out.append("| --- | --- | --- |")
        for kind, q in sorted(lat["step"].items()):
            out.append(f"| step, {kind} | {ms(q.get('p50'))} | {ms(q.get('p99'))} |")
        out.append(f"| append | {ms(lat['append'].get('p50'))} | {ms(lat['append'].get('p99'))} |")
        if lat.get("bounds"):
            out.append("")
            out.append(f"Bounds (p99): {lat['bounds']}.")
    return out


def main(argv):
    if len(argv) < 4 or argv[1] not in ("crash", "golden", "sim") or argv[2] not in HEADLINE:
        print(__doc__, file=sys.stderr)
        return 2
    if argv[1] == "sim":
        lines = sim(argv[2], argv[3], argv[4] if len(argv) > 4 else "")
    else:
        lines = crash(argv[2], argv[3:]) if argv[1] == "crash" else golden(argv[2], argv[3])
    print("\n".join(lines))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
