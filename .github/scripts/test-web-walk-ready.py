#!/usr/bin/env python3
"""Regression cases for web-walk.py's start (the Phase 4 gate's R2-G1, D-063; the static app, D-066).

The walk must only ever talk to the web app it started. Each case points the walk at an ephemeral
port and starts a stand-in child, a small Python server that binds 127.0.0.1 and prints vite
preview's receipt line. Where another app already holds the port (R2's repros, on 127.0.0.1 and on
::1 only), start_web must refuse and that app must receive zero requests: nothing at all is sent to
it, not even a bad one.

Usage: test-web-walk-ready.py   (run from the repository root; VERBOSE=1 shows each refusal)
Exit codes: 0 every case passed; 1 a case failed; 2 the cases couldn't run.
"""
import importlib.util
import os
import socket
import socketserver
import sys
import tempfile
import threading
import urllib.request

CHILD = r'''
import http.server, sys, time
port, delay, receipt = int(sys.argv[1]), float(sys.argv[2]), sys.argv[3]
time.sleep(delay)

class H(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        self.send_response(200)
        self.end_headers()
        self.wfile.write(b"child")

    def log_message(self, *a):
        pass

server = http.server.HTTPServer(("127.0.0.1", port), H)
print(receipt.replace("PORT", str(port)), flush=True)
server.serve_forever()
'''

RECEIPT = "  \x1b[32m➜\x1b[39m  \x1b[1mLocal\x1b[22m:   \x1b[36mhttp://127.0.0.1:PORT/\x1b[39m"


class StandIn(socketserver.ThreadingTCPServer):
    """Another app on the port: it records each connection that sent anything, and answers 200."""
    allow_reuse_address = True
    daemon_threads = True

    def __init__(self, host, family=socket.AF_INET):
        self.address_family = family
        self.requests_seen = []
        super().__init__((host, 0) if family == socket.AF_INET else (host, 0, 0, 0), Handler)
        threading.Thread(target=self.serve_forever, daemon=True).start()

    @property
    def port(self):
        return self.server_address[1]


class Handler(socketserver.BaseRequestHandler):
    def handle(self):
        self.request.settimeout(2)
        try:
            data = self.request.recv(4096)
        except OSError:
            return
        if data:
            self.server.requests_seen.append(data.split(b"\r\n", 1)[0])
            self.request.sendall(b"HTTP/1.0 200 OK\r\nContent-Length: 8\r\n\r\nstand-in")


def free_port():
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


def ipv6():
    try:
        with socket.socket(socket.AF_INET6) as s:
            s.bind(("::1", 0))
        return True
    except OSError:
        return False


def load_walk():
    spec = importlib.util.spec_from_file_location("web_walk", ".github/scripts/web-walk.py")
    walk = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(walk)
    walk.READY_SECONDS = 3
    return walk


failures = 0
work = tempfile.mkdtemp()
child_py = os.path.join(work, "child.py")
with open(child_py, "w") as f:
    f.write(CHILD)


def start(walk, port, delay=0.3, receipt=RECEIPT):
    """start_web with the stand-in child; returns (the process or None, the refusal or None)."""
    log = open(os.path.join(work, "web-%d.log" % port), "w")
    try:
        argv = [sys.executable, child_py, str(port), str(delay), receipt]
        return walk.start_web(argv, work, log, port), None
    except RuntimeError as e:
        return None, str(e)


def expect(name, condition, detail=""):
    global failures
    if condition:
        print("pass: " + name)
    else:
        failures += 1
        print("FAIL: %s: %s" % (name, detail))


def refused(name, walk, port, stand_in, words, **kw):
    """A case where start_web must refuse, sending the stand-in nothing."""
    proc, refusal = start(walk, port, **kw)
    if proc is not None:
        walk.stop(proc)
    if os.environ.get("VERBOSE") and refusal:
        print("    " + refusal)
    expect(name + ": refused", proc is None and refusal is not None and words in refusal, repr(refusal))
    if stand_in is not None:
        expect(name + ": the other app received zero requests", not stand_in.requests_seen,
               repr(stand_in.requests_seen))


def main():
    walk = load_walk()

    # 1. R2's repro: another app answers on 127.0.0.1.
    other = StandIn("127.0.0.1")
    refused("another app on 127.0.0.1", walk, other.port, other, "already listens on localhost")
    other.shutdown()

    # 2. The same on ::1 only: `localhost` may resolve there first.
    if ipv6():
        other = StandIn("::1", socket.AF_INET6)
        refused("another app on ::1 only", walk, other.port, other, "already listens on localhost")
        other.shutdown()
    else:
        print("skip: another app on ::1 only (no IPv6 loopback here)")

    # 3. A child that says it serves somewhere else isn't the walk's app.
    refused("a receipt naming another address", walk, free_port(), None, "didn't say",
            receipt="  Local:   http://localhost:1/")

    # 4. A child that exits before serving is refused.
    refused("a child that exits first", walk, free_port(), None, "didn't say", delay="x")

    # 5. The real path: a free port, then the receipt; the first request reaches the walk's own child.
    port = free_port()
    proc, refusal = start(walk, port)
    try:
        expect("a free port: ready on the child's receipt", proc is not None and proc.poll() is None, repr(refusal))
        if proc is not None:
            body = urllib.request.urlopen("http://127.0.0.1:%d/" % port, timeout=5).read()
            expect("a free port: the first request reaches the walk's own child", body == b"child", repr(body))
    finally:
        if proc is not None:
            walk.stop(proc)

    if failures:
        print("error: %d web-walk start case(s) failed" % failures, file=sys.stderr)
        sys.exit(1)
    print("ok: web-walk.py starts only on its own child's receipt and refuses a taken port")


if __name__ == "__main__":
    if not os.path.isfile(".github/scripts/web-walk.py"):
        print("error: run from the repository root", file=sys.stderr)
        sys.exit(2)
    main()
