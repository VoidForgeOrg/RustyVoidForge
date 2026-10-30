#!/usr/bin/env python3
"""The nightly simulation's step latency (Technical Plan P5.3; decision log D-064, D-066): serve's
own histograms, read through Grafana's Prometheus datasource with deploy/README.md's PromQL (the
query people will use), for this run's serve only (its `service.instance.id`, which the collector
turns into the `instance` label).

It waits, bounded, for the run's samples to arrive (the collector's export and scrape lag), reads
the raw bucket counts first so a run with no samples fails loudly instead of reading as fast, then
p50 and p99 of a step by kind and of the append, waiting (within the same bound) until every kind
with samples has both: an incomplete read fails. Bounds are checked when given.

A query that fails (refused, an HTTP error, a cut-off or unreadable answer: Grafana's proxy
restarting, as on night 7, slice 53) is retried within the same deadline, then is exit 3. Only
reading is retried; the bounds are compared once, on a complete read.

Usage: nightly-latency.py <instance> <window> [--step-p99-max S] [--append-p99-max S]
  <window>  the PromQL range covering the run, for example 35m
Environment: NIGHTLY_PROMETHEUS (default
  http://localhost:3000/api/datasources/proxy/uid/prometheus), NIGHTLY_SAMPLES_WAIT seconds
  (default 180).
Prints one line, `@nightly-latency <json>`.
Exit codes: 0 read (and within the bounds given); 1 over a bound, or no samples or no quantiles
after the wait;
2 usage; 3 Prometheus not reachable, or its answers unreadable, until the deadline.
"""

import http.client
import json
import os
import sys
import time
import urllib.parse
import urllib.request

BASE = os.environ.get("NIGHTLY_PROMETHEUS",
                      "http://localhost:3000/api/datasources/proxy/uid/prometheus")
WAIT = float(os.environ.get("NIGHTLY_SAMPLES_WAIT", "180"))
STEP = "voidforge_host_step_duration_seconds"
APPEND = "voidforge_host_append_duration_seconds"


class Unreachable(Exception):
    pass


def ask(q):
    url = f"{BASE}/api/v1/query?" + urllib.parse.urlencode({"query": q})
    try:
        with urllib.request.urlopen(url, timeout=15) as resp:
            body = json.load(resp)
        if body.get("status") != "success":
            raise Unreachable(f"query failed: {body}")
        return body["data"]["result"]
    # Every transport and parse failure is the collector's, never a verdict (R1-1): a refused or
    # HTTP-error answer (OSError), a cut-off body or bad status line (HTTPException), and a body
    # that isn't the API's JSON (ValueError, or the wrong shape).
    except (OSError, http.client.HTTPException, ValueError, AttributeError, KeyError, TypeError) as e:
        raise Unreachable(f"{type(e).__name__}: {e}") from e


def query(q, deadline):
    """ask(q), retried every 5 s while the deadline lasts (QA's A1); Unreachable past it."""
    while True:
        try:
            return ask(q)
        except Unreachable:
            if time.monotonic() > deadline:
                raise
            time.sleep(5)


def value(series):
    v = float(series["value"][1])
    return None if v != v else v  # NaN: no samples in the window


def main(argv):
    args = argv[1:]
    if len(args) < 2:
        print(__doc__, file=sys.stderr)
        return 2
    instance, window, rest = args[0], args[1], args[2:]
    bounds = {}
    while rest:
        if len(rest) < 2 or rest[0] not in ("--step-p99-max", "--append-p99-max"):
            print(__doc__, file=sys.stderr)
            return 2
        bounds[rest[0]] = float(rest[1])
        rest = rest[2:]
    sel = f'instance="{instance}"'
    try:
        # The raw counts first: a run whose samples never arrived must not read as fast.
        deadline = time.monotonic() + WAIT
        while True:
            counts = query(f"sum by (kind) (increase({STEP}_count{{{sel}}}[{window}]))", deadline)
            appends = query(f"sum(increase({APPEND}_count{{{sel}}}[{window}]))", deadline)
            steps = {s["metric"].get("kind", "?"): value(s) or 0 for s in counts}
            appended = value(appends[0]) if appends else 0
            if steps.get("command", 0) > 0 and (appended or 0) > 0:
                break
            if time.monotonic() > deadline:
                print(f"@nightly-latency {json.dumps({'instance': instance, 'error': 'no samples', 'steps': steps, 'appends': appended})}")
                print(f"error: no step or append samples for instance {instance} after {WAIT:.0f} s",
                      file=sys.stderr)
                return 1
            time.sleep(5)
        # Then the quantiles, until every kind with samples (always command) and the append has
        # finite p50 and p99: a missing series must not pass a bound by being absent (R2-B2).
        need = {k for k, n in steps.items() if n > 0} | {"command"}
        while True:
            result = {"instance": instance, "window": window, "steps": steps, "appends": appended,
                      "step": {}, "append": {}}
            for q in ("0.5", "0.99"):
                for s in query(f"histogram_quantile({q}, sum by (le, kind) (rate({STEP}_bucket{{{sel}}}[{window}])))", deadline):
                    result["step"].setdefault(s["metric"].get("kind", "?"), {})[f"p{int(float(q) * 100)}"] = value(s)
                r = query(f"histogram_quantile({q}, sum by (le) (rate({APPEND}_bucket{{{sel}}}[{window}])))", deadline)
                result["append"][f"p{int(float(q) * 100)}"] = value(r[0]) if r else None
            missing = sorted(f"step {k}" for k in need
                             if any(result["step"].get(k, {}).get(p) is None for p in ("p50", "p99")))
            missing += [f"append {p}" for p in ("p50", "p99") if result["append"].get(p) is None]
            if not missing:
                break
            if time.monotonic() > deadline:
                result["error"] = "incomplete"
                result["missing"] = missing
                print(f"@nightly-latency {json.dumps(result)}")
                print(f"error: no quantile for {', '.join(missing)} for instance {instance} after "
                      f"{WAIT:.0f} s", file=sys.stderr)
                return 1
            time.sleep(5)
    except Unreachable as e:
        print(f"error: Prometheus isn't reachable at {BASE} (retried for {WAIT:.0f} s): {e}",
              file=sys.stderr)
        return 3
    over = []
    step_max = bounds.get("--step-p99-max")
    if step_max is not None:
        for kind, q in result["step"].items():
            if q.get("p99") is None or q["p99"] > step_max:
                over.append(f"step {kind} p99 {q.get('p99')} s over {step_max} s")
    append_max = bounds.get("--append-p99-max")
    if append_max is not None:
        p99 = result["append"].get("p99")
        if p99 is None or p99 > append_max:
            over.append(f"append p99 {p99} s over {append_max} s")
    result["bounds"] = {k.strip("-").replace("-", "_"): v for k, v in bounds.items()}
    result["over"] = over
    print(f"@nightly-latency {json.dumps(result)}")
    for o in over:
        print(f"error: {o}", file=sys.stderr)
    return 1 if over else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
