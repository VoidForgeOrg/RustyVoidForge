#!/usr/bin/env python3
"""Serve the built web app and run Playwright's walk against it, talking only to the app it started
(the Phase 4 gate's R2-G1, D-063, carried to the static app, D-066).

  1. refuse (exit 2) when anything accepts on localhost:PORT, on any address `localhost` resolves to,
     before starting anything: that app would otherwise get alice's login;
  2. start `vite preview` bound to 127.0.0.1:PORT with a strict port, as this script's own child;
  3. be ready only on the child's own "Local: http://127.0.0.1:PORT/" line, read from its pipe while
     it lives;
  4. run the walk (web/e2e), whose Chromium maps `localhost` to 127.0.0.1, so every request reaches
     the child while the URL, dex's redirect and the origin stay `localhost`;
  5. stop the child, and exit with the walk's code.

Usage: web-walk.py --dist DIR --api URL --issuer URL --output DIR --log FILE
Exit codes: the walk's; 2 the app couldn't be served (the port is taken, or no receipt).
"""
import argparse
import os
import queue
import re
import socket
import subprocess
import sys
import threading
import time

PORT = 8081
READY_SECONDS = 30
ANSI = re.compile(r"\x1b\[[0-9;]*m")


def listeners(port=None):
    """The addresses `localhost` resolves to where something accepts on the port. Connecting only:
    nothing is ever sent."""
    port = PORT if port is None else port
    found = []
    for family, kind, proto, _, addr in socket.getaddrinfo("localhost", port, type=socket.SOCK_STREAM):
        with socket.socket(family, kind, proto) as s:
            s.settimeout(0.5)
            if s.connect_ex(addr) == 0:
                found.append(addr[0])
    return sorted(set(found))


def start_web(argv, cwd, log, port=None):
    """Starts the child once the port is free, and returns it once it says it serves on
    127.0.0.1:PORT; raises RuntimeError otherwise, with the child stopped."""
    port = PORT if port is None else port
    taken = listeners(port)
    if taken:
        raise RuntimeError("something already listens on localhost:%d (%s); stop it, then run the walk"
                           % (port, ", ".join(taken)))
    child = subprocess.Popen(argv, cwd=cwd, stdout=subprocess.PIPE, stderr=log, universal_newlines=True)
    lines = queue.Queue()

    def pump():
        for line in child.stdout:
            log.write(line)
            log.flush()
            lines.put(line)
        lines.put(None)

    threading.Thread(target=pump, daemon=True).start()
    receipt = re.compile(r"Local:\s+http://127\.0\.0\.1:%d/\s*$" % port)
    until = time.monotonic() + READY_SECONDS
    while time.monotonic() < until:
        try:
            line = lines.get(timeout=max(0.0, until - time.monotonic()))
        except queue.Empty:
            break
        if line is None:
            break
        if receipt.search(ANSI.sub("", line)) and child.poll() is None:
            return child
    stop(child)
    raise RuntimeError("the web app didn't say it serves on http://127.0.0.1:%d/ (exit %s); see %s"
                       % (port, child.returncode, log.name))


def stop(child):
    if child.poll() is None:
        child.terminate()
        try:
            child.wait(timeout=10)
        except subprocess.TimeoutExpired:
            child.kill()
            child.wait()


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--dist", required=True)
    p.add_argument("--api", required=True)
    p.add_argument("--issuer", required=True)
    p.add_argument("--output", required=True)
    p.add_argument("--log", required=True)
    args = p.parse_args()
    web = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "web")
    preview = ["node_modules/.bin/vite", "preview", "--host", "127.0.0.1", "--port", str(PORT), "--strictPort",
               "--outDir", os.path.abspath(args.dist)]
    with open(args.log, "w") as log:
        try:
            child = start_web(preview, web, log)
        except RuntimeError as e:
            print("web-walk: " + str(e), file=sys.stderr)
            return 2
        try:
            env = dict(os.environ, WALK_API=args.api, WALK_ISSUER=args.issuer)
            return subprocess.call(["npm", "run", "walk", "--", "--output", os.path.abspath(args.output)],
                                   cwd=web, env=env)
        finally:
            stop(child)


if __name__ == "__main__":
    sys.exit(main())
