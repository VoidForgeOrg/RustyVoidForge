#!/usr/bin/env python3
"""Serve the built web app and run Playwright's walk against it, talking only to the app it started
(the Phase 4 gate's R2-G1, D-063, carried to the static app, D-066).

  1. refuse (exit 2) when anything accepts on localhost:PORT, on any address `localhost` resolves to,
     before starting anything: that app would otherwise get alice's login;
  2. start `vite preview` bound to 127.0.0.1:PORT with a strict port, as this script's own child,
     or with --image the web image (D-065, D-066) as `docker run --rm -i`, which relays its stop's
     SIGTERM and returns the container's exit code;
  3. be ready only on the child's own line naming http://127.0.0.1:PORT, read from its pipe while it
     lives (vite's "Local: ...", static-web-server's "listening on ..."); for the image, then wait,
     only connecting, until the port accepts here (colima forwards host networking on macOS);
  4. run the walk (web/e2e), whose Chromium maps `localhost` to 127.0.0.1, so every request reaches
     the child while the URL, dex's redirect and the origin stay `localhost`;
  5. stop the child, and exit with the walk's code.

Usage: web-walk.py (--dist DIR | --image ID --name NAME) --api URL --issuer URL --output DIR --log FILE
       web-walk.py --check-port PORT   (only the pre-check, for another port e2e.sh is about to use)
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
FORWARD_SECONDS = 10
ANSI = re.compile(r"\x1b\[[0-9;]*m")
# The line each server prints once it listens; PORT is filled in.
VITE = r"Local:\s+http://127\.0\.0\.1:PORT/\s*$"
STATIC_WEB_SERVER = r"listening on http://127\.0\.0\.1:PORT\s*$"


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


def refusal(port):
    """Why the port can't be used, or None when nothing listens on it."""
    taken = listeners(port)
    if not taken:
        return None
    return "something already listens on localhost:%d (%s); stop it, then run again" % (port, ", ".join(taken))


def start_web(argv, cwd, log, port=None, receipt=VITE, forwarded=False):
    """Starts the child once the port is free, and returns it once it says it serves on
    127.0.0.1:PORT (and, forwarded, once the port accepts here); raises RuntimeError otherwise, with
    the child stopped."""
    port = PORT if port is None else port
    taken = refusal(port)
    if taken:
        raise RuntimeError(taken)
    # Both streams: static-web-server logs its receipt on stderr, vite on stdout.
    child = subprocess.Popen(argv, cwd=cwd, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
                             universal_newlines=True)
    lines = queue.Queue()

    def pump():
        for line in child.stdout:
            log.write(line)
            log.flush()
            lines.put(line)
        lines.put(None)

    threading.Thread(target=pump, daemon=True).start()
    said = re.compile(receipt.replace("PORT", str(port)))
    until = time.monotonic() + READY_SECONDS
    while time.monotonic() < until:
        try:
            line = lines.get(timeout=max(0.0, until - time.monotonic()))
        except queue.Empty:
            break
        if line is None:
            break
        if said.search(ANSI.sub("", line)) and child.poll() is None:
            if not forwarded or accepts(port, child):
                return child
            break
    stop(child)
    raise RuntimeError("the web app didn't say it serves on http://127.0.0.1:%d/ (exit %s); see %s"
                       % (port, child.returncode, log.name))


def accepts(port, child):
    """Waits, connecting only, until 127.0.0.1:PORT accepts here while the child lives."""
    until = time.monotonic() + FORWARD_SECONDS
    while time.monotonic() < until and child.poll() is None:
        with socket.socket() as s:
            s.settimeout(0.5)
            if s.connect_ex(("127.0.0.1", port)) == 0:
                return True
        time.sleep(0.2)
    return False


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
    where = p.add_mutually_exclusive_group(required=True)
    where.add_argument("--dist")
    where.add_argument("--image")
    where.add_argument("--check-port", type=int)
    p.add_argument("--name", help="the walk's web container's name, with --image")
    p.add_argument("--api")
    p.add_argument("--issuer")
    p.add_argument("--output")
    p.add_argument("--log")
    args = p.parse_args()
    if args.check_port is not None:
        taken = refusal(args.check_port)
        if taken:
            print("web-walk: " + taken, file=sys.stderr)
            return 2
        return 0
    if args.image and not args.name:
        p.error("--image needs --name, so a container a crash leaves behind can be found and removed")
    missing = [f"--{a}" for a in ("api", "issuer", "output", "log") if getattr(args, a) is None]
    if missing:
        p.error("the walk needs " + ", ".join(missing))
    web = os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "web")
    if args.image:
        argv = ["docker", "run", "--rm", "-i", "--network", "host", "--name", args.name, args.image]
        start = dict(receipt=STATIC_WEB_SERVER, forwarded=True)
    else:
        argv = ["node_modules/.bin/vite", "preview", "--host", "127.0.0.1", "--port", str(PORT), "--strictPort",
                "--outDir", os.path.abspath(args.dist)]
        start = {}
    with open(args.log, "w") as log:
        try:
            child = start_web(argv, web, log, **start)
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
