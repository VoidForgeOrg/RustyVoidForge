#!/usr/bin/env python3
"""Prove the telemetry bootstrap by effect (Technical Plan P0.6, decision log D-016): each binary's
stub span travels through the local OTLP collector into Tempo and is found through Grafana.
`just trace-check` runs this after `just up`.

For each of voidforge-server, voidforge-bots and voidforge-replay:
  export   run it with OTEL_EXPORTER_OTLP_ENDPOINT=http://localhost:4318; it must exit 0 and
           print trace_id=<32 hex digits>
  find     fetch that trace through Grafana's Tempo datasource; it must hold a stub_startup span
           from service.name voidforge-<binary> with attribute binary=voidforge-<binary>
Then the lifecycle cases, with voidforge-bots:
  offline      without the variable: exit 0, a local log line, no trace ID (nothing exported)
  unreachable  with an endpoint where nothing listens: exit 1 and a delivery error on stderr

Usage: verify_trace.py   (run from the repository root, with the local stack up)
Exit codes: 0 all checks passed, 1 a check failed, 2 could not check (cargo or Grafana missing).
"""
import base64
import json
import os
import re
import socket
import subprocess
import sys
import time
import urllib.error
import urllib.request

BINARIES = ["voidforge-server", "voidforge-bots", "voidforge-replay"]
ENDPOINT = "http://localhost:4318"
GRAFANA_TRACE = "http://localhost:3000/api/datasources/proxy/uid/tempo/api/v2/traces/"
FIND_TIMEOUT_S = 30

failures = []


def ok(message):
    print("ok:   " + message)


def fail(message):
    failures.append(message)
    print("FAIL: " + message)


def run_binary(binary, endpoint):
    env = dict(os.environ)
    env.pop("OTEL_EXPORTER_OTLP_ENDPOINT", None)
    if endpoint:
        env["OTEL_EXPORTER_OTLP_ENDPOINT"] = endpoint
    try:
        return subprocess.run(["cargo", "run", "--quiet", "--locked", "-p", binary], env=env,
                              stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True)
    except OSError as exc:
        print("error: cannot run cargo (%s); could not check" % exc, file=sys.stderr)
        sys.exit(2)


def find_stub_span(binary, trace_id):
    """Poll Grafana's Tempo datasource until the trace holds the binary's stub span.

    Tempo can answer 404, or 200 with an empty trace, until ingestion completes, so only the
    expected span itself counts. Returns (span, seconds) or (None, last answer)."""
    started = time.time()
    last = None
    while time.time() - started < FIND_TIMEOUT_S:
        try:
            with urllib.request.urlopen(GRAFANA_TRACE + trace_id, timeout=10) as resp:
                spans = stub_spans(json.load(resp), trace_id)
            last = "HTTP 200 with spans %s" % spans
            for span in spans:
                if span[0] == binary and span[1] == "stub_startup" and span[2].get("binary") == binary:
                    return span, time.time() - started
        except urllib.error.HTTPError as exc:
            last = "HTTP %s" % exc.code
        except (urllib.error.URLError, OSError) as exc:
            print("error: Grafana is not reachable on localhost:3000 (%s); run `just up`" % exc, file=sys.stderr)
            sys.exit(2)
        time.sleep(1)
    return None, last


def stub_spans(trace, trace_id):
    """(service.name, span name, attributes) for spans of `trace` with the given trace ID."""
    found = []
    root = trace.get("trace", trace)
    for resource_spans in root.get("resourceSpans", root.get("batches", [])):
        attributes = resource_spans.get("resource", {}).get("attributes", [])
        service = next((list(a["value"].values())[0] for a in attributes if a["key"] == "service.name"), None)
        for scope_spans in resource_spans.get("scopeSpans", []):
            for span in scope_spans.get("spans", []):
                if base64.b64decode(span["traceId"]).hex() != trace_id:
                    continue
                span_attributes = {a["key"]: list(a["value"].values())[0] for a in span.get("attributes", [])}
                found.append((service, span["name"], span_attributes))
    return found


def check_export(binary):
    proc = run_binary(binary, ENDPOINT)
    match = re.search(r"^trace_id=([0-9a-f]{32})$", proc.stdout, re.MULTILINE)
    if proc.returncode != 0 or not match:
        fail("%s: export run exited %d without a trace ID: %s" % (binary, proc.returncode, proc.stderr.strip()[-300:]))
        return
    trace_id = match.group(1)
    ok("%s: exported and exited 0, trace_id=%s" % (binary, trace_id))
    span, detail = find_stub_span(binary, trace_id)
    if span:
        ok("%s: found through Grafana's Tempo datasource after %.1fs: span stub_startup, service.name %s"
           % (binary, detail, binary))
    else:
        fail("%s: stub span of trace %s not found through Grafana within %ds (last: %s)"
             % (binary, trace_id, FIND_TIMEOUT_S, detail))


def check_offline():
    proc = run_binary("voidforge-bots", None)
    if proc.returncode == 0 and "trace_id=" not in proc.stdout and "stub started" in proc.stderr:
        ok("offline: voidforge-bots without an endpoint exits 0, logs locally and exports nothing")
    else:
        fail("offline: exit %d, stdout %r, stderr %r" % (proc.returncode, proc.stdout, proc.stderr[-300:]))


def check_unreachable():
    listener = socket.socket()
    listener.bind(("127.0.0.1", 0))
    port = listener.getsockname()[1]
    listener.close()
    started = time.time()
    proc = run_binary("voidforge-bots", "http://127.0.0.1:%d" % port)
    elapsed = time.time() - started
    if proc.returncode == 1 and "could not be delivered" in proc.stderr:
        ok("unreachable: voidforge-bots exits 1 after %.1fs with: %s" % (elapsed, proc.stderr.strip().splitlines()[-1]))
    else:
        fail("unreachable: exit %d after %.1fs, stderr %r" % (proc.returncode, elapsed, proc.stderr[-300:]))


def main():
    if not os.path.isfile("deploy/compose.yaml"):
        print("error: run from the repository root; could not check", file=sys.stderr)
        return 2
    for binary in BINARIES:
        check_export(binary)
    check_offline()
    check_unreachable()
    if failures:
        print("error: %d telemetry check(s) failed" % len(failures), file=sys.stderr)
        return 1
    print("ok: every binary's stub span reached Grafana's trace backend; offline and failure behavior hold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
