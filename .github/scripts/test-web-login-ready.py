#!/usr/bin/env python3
"""Regression cases for web-login.py's start (Phase 4 gate, R2-G1 and R1-G2).

The walk must only ever talk to the voidforge-web it started. Each case points the walk's WEB at an
ephemeral port and starts a stand-in child, a small Python server that waits, then binds and prints
voidforge-web's receipt line, as the real one does after the issuer's discovery. Where another app
already holds the port (R2's repro), start_web must refuse and that app must receive zero requests:
nothing at all is sent to it, not even a bad one.

Usage: test-web-login-ready.py   (run from the repository root; VERBOSE=1 shows each refusal)
Exit codes: 0 every case passed; 1 a case failed; 2 the cases couldn't run.
"""
import importlib.util
import os
import queue
import socket
import socketserver
import subprocess
import sys
import tempfile
import threading
import time
import types
import urllib.request

CHILD = r'''
import http.server, signal, socket, sys, time
bind, delay, receipt = sys.argv[sys.argv.index("--bind") + 1], float(sys.argv[1]), sys.argv[2]
host, port = bind.rsplit(":", 1)
time.sleep(delay)  # the issuer's discovery, before the bind
try:
    server = http.server.HTTPServer((host, int(port)), type("H", (http.server.BaseHTTPRequestHandler,), {
        "do_GET": lambda self: (self.send_response(200), self.end_headers(), self.wfile.write(b"child")),
        "log_message": lambda self, *a: None}))
except OSError as e:
    print("voidforge-web: listening on %s: %s" % (bind, e), file=sys.stderr)
    sys.exit(1)
signal.signal(signal.SIGTERM, lambda *a: sys.exit(0))
print(receipt.replace("BIND", bind), flush=True)
if sys.argv[3:4] == ["exit"]:
    sys.exit(0)  # serving, then gone at once
server.serve_forever()
'''


class StandIn(socketserver.ThreadingTCPServer):
    """Another app on the port: it records each connection that sent anything, and answers 200."""
    allow_reuse_address = True
    daemon_threads = True

    def __init__(self, host, family=socket.AF_INET, port=0):
        self.address_family = family
        self.requests_seen = []
        super().__init__((host, port) if family == socket.AF_INET else (host, port, 0, 0), Handler)
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
    spec = importlib.util.spec_from_file_location("web_login", ".github/scripts/web-login.py")
    walk = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(walk)
    walk.READY_SECONDS = 3
    return walk


failures = 0
work = tempfile.mkdtemp()
child = os.path.join(work, "child.py")
with open(child, "w") as f:
    f.write(CHILD)
RECEIPT = "voidforge-web: serving on http://BIND"


def start(walk, port, delay=0.5, receipt=RECEIPT, then=()):
    """start_web with the stand-in child; returns (the process or None, the refusal or None)."""
    walk.WEB = "http://localhost:%d" % port
    log = open(os.path.join(work, "web-%d.log" % port), "w")
    try:
        argv = [sys.executable, child, str(delay), receipt, *then]
        return walk.start_web(argv, dict(os.environ), log), None
    except RuntimeError as e:
        return None, str(e)


def stop(proc):
    if proc is not None and proc.poll() is None:
        proc.terminate()
        proc.wait(timeout=10)


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
    stop(proc)
    if os.environ.get("VERBOSE") and refusal:
        print("    " + refusal)
    expect(name + ": refused", proc is None and refusal is not None and words in refusal, repr(refusal))
    if stand_in is not None:
        expect(name + ": the other app received zero requests", not stand_in.requests_seen,
               repr(stand_in.requests_seen))


def main():
    walk = load_walk()

    # 1. R2's repro: another app answers on 127.0.0.1 while the child is still in discovery.
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

    # 3. The check-then-spawn window: the port is free at the check and taken before the child's
    # bind. The child's bind fails, it exits, and the walk refuses on its missing receipt.
    port = free_port()
    taken = []
    check = walk.listeners

    def then_taken():
        found = check()
        taken.append(StandIn("127.0.0.1", port=port))
        return found

    walk.listeners = then_taken
    refused("the port taken after the check", walk, port, None, "didn't say")
    walk.listeners = check
    expect("the port taken after the check: the walk checked the port before starting", taken)
    expect("the port taken after the check: the other app received zero requests",
           taken and not taken[0].requests_seen, repr(taken and taken[0].requests_seen))
    for server in taken:
        server.shutdown()

    # 4. A child that says it serves somewhere else isn't the walk's web.
    refused("a receipt naming another address", walk, free_port(), None, "didn't say",
            receipt="voidforge-web: serving on http://127.0.0.1:1")

    # 5. A child that exits before serving is refused, with its log's tail.
    refused("a child that exits first", walk, free_port(), None, "exit 1", delay="x")

    # 5b. A child that says it's serving and then exits isn't serving: the walk reads the receipt
    # slowly here (a pipe read held 0.5 s), so the child is surely gone when it's judged.
    class Slow(queue.Queue):
        def get(self, *args, **kwargs):
            item = super().get(*args, **kwargs)
            time.sleep(0.5)
            return item

    walk.queue = types.SimpleNamespace(Queue=Slow, Empty=queue.Empty)
    refused("a child gone after its receipt", walk, free_port(), None, "didn't say", then=["exit"])
    walk.queue = queue

    # 6. The real path: a free port, a slow discovery, then the receipt; the first request reaches
    # the walk's own child.
    port = free_port()
    started = time.monotonic()
    proc, refusal = start(walk, port, delay=1.5)
    try:
        expect("a free port: ready on the child's receipt", proc is not None and proc.poll() is None, repr(refusal))
        expect("a free port: not ready before the child bound", time.monotonic() - started >= 1.5)
        if proc is not None:
            body = urllib.request.urlopen(walk.WEB + "/", timeout=5).read()
            expect("a free port: the first request reaches the walk's own child", body == b"child", repr(body))
    finally:
        stop(proc)

    if failures:
        print("error: %d web-login start case(s) failed" % failures, file=sys.stderr)
        sys.exit(1)
    print("ok: web-login.py starts only on its own child's receipt and refuses a taken port")


if __name__ == "__main__":
    if not os.path.isfile(".github/scripts/web-login.py"):
        print("error: run from the repository root", file=sys.stderr)
        sys.exit(2)
    main()
