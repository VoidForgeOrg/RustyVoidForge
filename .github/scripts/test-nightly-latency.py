#!/usr/bin/env python3
"""Regression cases for nightly-latency.py (P5.3, D-066), against a stub Prometheus: a read within
its bounds passes; a p99 over its bound fails while the p50 is under it; a run with no samples,
or with samples but a missing quantile series, fails instead of reading as fast; an unreachable
Prometheus is exit 3 (infrastructure). Slice 53: every kind of failed query (an HTTP error, a
cut-off body, HTML, a bad status line, the wrong JSON) is exit 3 when it lasts to the deadline,
and a read that recovers before it passes, or fails on a bound as it would have (A1, R1-1).

Usage: test-nightly-latency.py
Exit codes: 0 every case passed; 1 one failed.
"""

import http.server
import json
import os
import subprocess
import sys
import threading
import time
import urllib.parse

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(HERE, "nightly-latency.py")

# What the stub answers: raw counts, and p50 and p99 in seconds per series.
SCENARIO = {}
# The next FAULT["left"] requests get FAULT["kind"] instead.
FAULT = {"kind": None, "left": 0}
FAULTS = ["502", "truncated", "html", "badstatus", "shape"]


def fault(handler, kind):
    handler.close_connection = True
    if kind == "badstatus":
        handler.wfile.write(b"garbage\r\n\r\n")
        return
    code, body, length = {
        "502": (502, b"Bad Gateway", None),
        "truncated": (200, b'{"status": "succ', 100),
        "html": (200, b"<html><body>Grafana</body></html>", None),
        "shape": (200, b'{"status": "success"}', None),
    }[kind]
    handler.send_response(code)
    handler.send_header("Content-Length", str(length or len(body)))
    handler.end_headers()
    handler.wfile.write(body)


def vector(pairs):
    return [{"metric": metric, "value": [0, str(v)]} for metric, v in pairs]


class Stub(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        if FAULT["left"] > 0:
            FAULT["left"] -= 1
            return fault(self, FAULT["kind"])
        q = urllib.parse.parse_qs(urllib.parse.urlparse(self.path).query)["query"][0]
        s = SCENARIO
        if "_count" in q:
            if "step" in q:
                result = vector([({"kind": k}, n) for k, n in s["steps"].items()])
            else:
                result = vector([({}, s["appends"])])
        else:
            p = "p50" if "histogram_quantile(0.5," in q else "p99"
            if "step" in q:
                result = vector([({"kind": k}, v[p]) for k, v in s["step"].items()])
            else:
                result = vector([({}, s["append"][p])])
        body = json.dumps({"status": "success", "data": {"result": result}}).encode()
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_):
        pass


def read(base, bounds, wait="0"):
    env = dict(os.environ, NIGHTLY_PROMETHEUS=base, NIGHTLY_SAMPLES_WAIT=wait)
    p = subprocess.run([sys.executable, SCRIPT, "run1", "35m", *bounds], env=env,
                       capture_output=True, text=True, timeout=60)
    return p.returncode, p.stdout + p.stderr


GOOD = {
    "steps": {"command": 900, "advance": 400}, "appends": 1200,
    "step": {"command": {"p50": 0.00005, "p99": 0.0001}, "advance": {"p50": 0.00005, "p99": 0.0001}},
    "append": {"p50": 0.001, "p99": 0.005},
}
BOUNDS = ["--step-p99-max", "0.05", "--append-p99-max", "0.25"]


def with_(**edits):
    s = json.loads(json.dumps(GOOD))
    for path, v in edits.items():
        node = s
        keys = path.split("__")
        for k in keys[:-1]:
            node = node[k]
        node[keys[-1]] = v
    return s


CASES = [
    ("within the bounds passes", GOOD, BOUNDS, 0, "@nightly-latency"),
    ("no bounds given reads and passes", GOOD, [], 0, "@nightly-latency"),
    ("a step p99 over its bound fails, though its p50 is under",
     with_(step__command__p99=0.06), BOUNDS, 1, "step command p99"),
    ("an append p99 over its bound fails, though its p50 is under",
     with_(append__p99=0.3), BOUNDS, 1, "append p99"),
    ("no command samples fails, not fast", with_(steps={"advance": 400}), BOUNDS, 1, "no step or append samples"),
    ("no appends fails, not fast", with_(appends=0), BOUNDS, 1, "no step or append samples"),
    ("a NaN p99 under a bound fails", with_(step__command__p99="NaN"), BOUNDS, 1, "no quantile for step command"),
    # R2-B2: counts arrived but a quantile series didn't; absent must not pass.
    ("no step quantiles at all fails", with_(step={}), BOUNDS, 1, "no quantile for step advance, step command"),
    ("a kind with samples but no quantile fails", with_(step={"advance": GOOD["step"]["advance"]}), BOUNDS, 1,
     "no quantile for step command"),
    ("a missing quantile fails without bounds too", with_(step={}), [], 1, "no quantile for step"),
    ("no append quantile fails", with_(append={"p50": "NaN", "p99": "NaN"}), BOUNDS, 1, "append p50, append p99"),
]


def main():
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Stub)
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{server.server_address[1]}"
    bad = 0
    for name, scenario, bounds, want_code, want_text in CASES:
        SCENARIO.clear()
        SCENARIO.update(scenario)
        code, out = read(base, bounds)
        if code == want_code and want_text in out:
            print(f"pass: {name}")
        else:
            print(f"FAIL: {name}: expected exit {want_code} with {want_text!r}, got {code}:\n{out}")
            bad += 1
    # A failed query, lasting to the deadline: infrastructure, never a verdict (R1-1).
    # (case, scenario, bounds, fault, how many, wait, exit, text, seconds at least, at most)
    faults = [(f"a {k} answer to the deadline is exit 3", GOOD, BOUNDS, k, 10**6, "0", 3, "isn't reachable",
               0, 30) for k in FAULTS]
    faults += [
        ("a 502 is retried until the deadline, then exit 3", GOOD, BOUNDS, "502", 10**6, "6", 3,
         "retried for 6 s", 6, 30),
        # Recovered before the deadline: the read goes on (A1).
        *[(f"a {k} answer, then a good one, passes", GOOD, BOUNDS, k, 1, "30", 0, "@nightly-latency", 4, 30)
          for k in FAULTS],
        # A hiccup changes nothing about the verdict: a p99 over its bound still fails.
        ("a 502, then a p99 over its bound, fails on the bound", with_(step__command__p99=0.06), BOUNDS,
         "502", 1, "30", 1, "step command p99", 4, 30),
    ]
    for name, scenario, bounds, kind, left, wait, want_code, want_text, least, most in faults:
        SCENARIO.clear()
        SCENARIO.update(scenario)
        FAULT.update(kind=kind, left=left)
        t = time.monotonic()
        code, out = read(base, bounds, wait)
        took = time.monotonic() - t
        FAULT.update(left=0)
        if code == want_code and want_text in out and least <= took <= most:
            print(f"pass: {name}")
        else:
            print(f"FAIL: {name}: expected exit {want_code} with {want_text!r} in {least}-{most} s, "
                  f"got {code} in {took:.1f} s:\n{out}")
            bad += 1
    server.shutdown()
    # Nothing listening: infrastructure, not a verdict.
    code, out = read("http://127.0.0.1:9", BOUNDS)
    if code == 3:
        print("pass: an unreachable Prometheus is exit 3")
    else:
        print(f"FAIL: an unreachable Prometheus is exit 3: got {code}:\n{out}")
        bad += 1
    if bad == 0:
        print("ok: every nightly-latency case passed")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
