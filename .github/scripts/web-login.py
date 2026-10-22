#!/usr/bin/env python3
"""The web login by effect against a real dex (Technical Plan P4.1 to P4.4; decision log D-057, D-059 to D-061).

Starts `voidforge-web` on localhost:8081 (dex registers the redirect by exact string), and
refuses if anything already listens there; it sends its first request only after the child's
own "serving on" line (start_web; the Phase 4 gate's R2-G1), and only once the app on the port proves
it holds the walk's per-run token (--identity-token, D-065). Then it logs alice in and out as a browser would, with two cookie jars: one for dex's pages and one for
the web app. Nothing here runs a browser; dex's pages are driven over HTTP as device-login.py
does.

  1  logged out:   Home offers a login link, and no session cookie is set
  2  login:        /login sets the cookie (HttpOnly, SameSite=Lax, Path=/, not Secure over
                   http) and sends the browser to dex with S256 PKCE, state and nonce
  3  dex:          dex shows its password form, and sends the browser back with a code
  4  callback:     303 to /, with a NEW session id (fixation)
  5  Home:         "Logged in as alice@voidforge.test", the short name and a score, which come
                   from the API's /players/me through the typed client: the API accepted the
                   web app's token (P4.2: "Your empire", the score and the homeworld)
  5b pages:        after the bots have played, each page renders from live data (P4.3): the
                   homeworld's planet page (its Storage and the Drill and Refinery it starts
                   with), Fleets, the Universe around it (the homeworld at 0.0 u) and the
                   Leaderboard ("you", or its not-ready page); a bad planet number is refused
  5c forms:        the MVP loop through the web's forms (P4.4, D-061), paced under the write
                   limit and each step checked against the API's JSON with alice's own token:
                   build a Shipyard (a double submit says "already submitted", a forged key is
                   "expired", and neither builds twice), queue a Colony Ship and a Cargo Vessel,
                   assemble them with 1 t of ingots, colonize an unowned neighbour, disband
                   there, assemble the Cargo Vessel with 0.5 t, and Transport it home (each load
                   waits until the planet holds twice it, since ship builds drain the ingots)
  6  replay:       the same callback again is refused (400), with or without the cookie
  7  tampered:     a callback whose state was changed is refused (400) and logs nobody in
  8  logout:       a GET can't log out; the POST does and says so, and the old cookie is logged
                   out too
  9  second login: dex shows its password form again (A2: no silent login)
  10 no secrets:   no token, code, state or the client secret in any page or the web app's log
  11 stop:         one SIGTERM, exit 0
  12 unreachable:  the web app started against a closed port: a logged-in Home is the retry
                   page at 502 with "Can't reach the game server", never a trace (P4.2)

Usage: web-login.py (--web <voidforge-web> | --web-cmd <command>) --api <url> --issuer <dex issuer> --log <file>
  --web-cmd runs the web app through a command prefix, split as a shell would, for example the image
  (P5.2, D-065): --web-cmd 'docker run --rm -i --network host -e VOIDFORGE_WEB_CLIENT_SECRET
  voidforge-web:local'. Only `docker run` attached relays the stop's SIGTERM and returns the
  container's own exit code; `docker compose run` and `up` don't.
Exit codes: 0 every check passed; 1 a check failed; 2 the checks couldn't run.
"""
import argparse
import html
import json
import os
import queue
import re
import secrets
import shlex
import signal
import socket
import subprocess
import sys
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
from http.client import HTTPConnection
from http.cookiejar import CookieJar

# Fixed: dex registers the redirect by exact string. test-web-login-ready.py points it elsewhere.
WEB = "http://localhost:8081"
# How long the web app may take to say it's serving (the issuer's discovery comes first).
READY_SECONDS = 30
SECRET = "voidforge-web-local-test-secret"  # TEST-ONLY, from deploy/dex/config*.yaml
USER, PASSWORD = "alice@voidforge.test", "alice-local-test-password"
COOKIE = "voidforge_session"

failures = []


class Failed(Exception):
    """A step that can't go on, naming the request and what was expected. The walk records it as
    a failed check and stops (P4.5: a failed step names itself, never a bare exception)."""


def ok(message):
    print("ok: " + message)


def fail(message):
    failures.append(message)
    print("FAILED: " + message, file=sys.stderr)


def check(condition, message, detail=""):
    if condition:
        ok(message)
    else:
        fail(message + (": " + detail if detail else ""))
    return condition


class PinnedConnection(HTTPConnection):
    """An HTTP connection that sends WEB's requests to 127.0.0.1, the address start_web made the
    child bind and its receipt names, never to whatever else `localhost` resolves to (R2's delta on
    96f71b7: an app binding [::1]:8081 after the pre-check got them). The URL, so the Host header,
    the cookies and dex's exact redirect, stays `localhost`."""

    def connect(self):
        if self.host == "localhost" and self.port == urllib.parse.urlsplit(WEB).port:
            self.sock = socket.create_connection(("127.0.0.1", self.port), self.timeout)
        else:
            super().connect()


class PinnedHTTP(urllib.request.HTTPHandler):
    """Every http:// request of the walk, through a Browser or a bare urlopen, on PinnedConnection.
    Redirects aren't followed (NoRedirect), so each hop, dex's absolute callback URL included, is
    a request of its own through here."""

    def http_open(self, req):
        return self.do_open(PinnedConnection, req)


urllib.request.install_opener(urllib.request.build_opener(PinnedHTTP()))


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Every redirect is returned, not followed, so each hop can be checked."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Browser:
    """One cookie jar and an opener that never follows redirects."""

    def __init__(self):
        self.jar = CookieJar()
        self.opener = urllib.request.build_opener(PinnedHTTP(), urllib.request.HTTPCookieProcessor(self.jar),
                                                  NoRedirect())
        self.pages = []

    def request(self, url, data=None, method=None):
        req = urllib.request.Request(url, data=data, method=method)
        try:
            response = self.opener.open(req, timeout=15)
        except urllib.error.HTTPError as e:
            response = e
        body = response.read().decode("utf-8", "replace")
        self.pages.append(body)
        return response.status, response.headers, body

    def cookie(self):
        return next((c.value for c in self.jar if c.name == COOKIE), None)


def set_cookie_header(headers):
    return next((v for v in headers.get_all("Set-Cookie") or [] if v.startswith(COOKIE + "=")), None)


def dex_login(dex, url, origin):
    """Follows dex's pages from the authorization URL to the redirect back. Returns
    (whether the password form was shown, the callback URL)."""
    status, headers, body = dex.request(url)
    hops = 0
    while status in (302, 303) and hops < 5:
        status, headers, body = dex.request(urllib.parse.urljoin(url, headers["Location"]))
        hops += 1
    form = re.search(r'<form method="post" action="([^"]+)"', body)
    if not form or "password" not in body:
        raise RuntimeError("dex didn't show its login form: %s" % body[:300])
    status, headers, body = dex.request(origin + form.group(1).replace("&amp;", "&"),
                                        urllib.parse.urlencode({"login": USER, "password": PASSWORD}).encode())
    hops = 0
    while status in (302, 303) and hops < 5:
        location = urllib.parse.urljoin(origin, headers["Location"])
        if location.startswith(WEB + "/callback"):
            return True, location
        status, headers, body = dex.request(location)
        hops += 1
    raise RuntimeError("dex didn't send the browser back (status %s): %s" % (status, body[:300]))


def listeners():
    """The loopback addresses on WEB's port that accept a connection: 127.0.0.1, ::1, and anything
    else `localhost` resolves to. A listener on either catches `localhost` requests, whichever the
    child binds. Only connects; it sends nothing."""
    port = urllib.parse.urlsplit(WEB).port
    addresses = {("127.0.0.1", socket.AF_INET), ("::1", socket.AF_INET6)}
    try:
        addresses |= {(a[4][0], a[0]) for a in socket.getaddrinfo("localhost", port, type=socket.SOCK_STREAM)}
    except OSError:
        pass
    found = []
    for host, family in sorted(addresses, key=str):
        try:
            with socket.socket(family, socket.SOCK_STREAM) as s:
                s.settimeout(1)
                s.connect((host, port))
                found.append(host)
        except OSError:
            pass
    return found


# After the walk stops its own web, how long start_web waits for the port to be free before its
# pre-check (P5.2, D-065, T2): on macOS, colima's forward of a stopped container's port lingers for
# up to ~3 s. The wait only waits; the pre-check after it is unchanged.
FREE_WAIT_SECONDS = 10
# With --web-cmd on macOS, the receipt names the bind inside colima's VM, and the Mac's side of the
# port is colima's forward, which appears ~20-30 ms later (measured, D-065): until then a connect is
# refused. How long start_web waits, only connecting, for the forward after the receipt.
FORWARD_WAIT_SECONDS = 5
# The header the walk's own web puts on every response, with the per-run token it was given
# (--identity-token; the lead's ruling on D-065): no proof, no requests.
IDENTITY_HEADER = "x-voidforge-identity"


def start_web(argv, env, log, after_own_child=False, forwarded=False):
    """Starts voidforge-web on WEB's port and returns it once it says so itself (R2-G1, R1-G2).

    It refuses when something already listens there: the walk would log in to and post forms to
    that app, whatever API it uses (an owner's web on voidforge_dev, another seat's). It never
    stops or replaces it. Then it is ready only on the child's own receipt, the line it prints
    after a successful bind, read from its own stdout while it is alive; no request is sent
    before. A child that lost the port between the check and its bind exits, and is refused
    with its log's tail. The child's stdout and stderr both go to `log`, as before.

    `after_own_child` (T2, D-065): the walk has just stopped its own web on this port, so it first
    waits up to FREE_WAIT_SECONDS for the port to be free, only connecting, never sending. Then the
    same pre-check runs and still refuses anything listening. The first start never waits."""
    port = urllib.parse.urlsplit(WEB).port
    if after_own_child:
        deadline = time.monotonic() + FREE_WAIT_SECONDS
        while listeners() and time.monotonic() < deadline:
            time.sleep(0.2)
    taken = listeners()
    if taken:
        raise RuntimeError("something already listens on localhost:%d (%s); the walk won't use another app. "
                           "Stop it, or wait until whoever holds the port is done" % (port, ", ".join(taken)))
    bind = "127.0.0.1:%d" % port
    token = secrets.token_hex(16)
    proc = subprocess.Popen(argv + ["--bind", bind, "--identity-token", token], env=env, stdout=subprocess.PIPE,
                            stderr=log, stdin=subprocess.DEVNULL, text=True, bufsize=1)
    lines = queue.Queue()

    def tee():
        for line in proc.stdout:
            log.write(line)
            log.flush()
            lines.put(line.rstrip("\n"))
        lines.put(None)

    threading.Thread(target=tee, daemon=True).start()
    receipt = "voidforge-web: serving on http://" + bind
    deadline = time.monotonic() + READY_SECONDS
    while time.monotonic() < deadline:
        try:
            line = lines.get(timeout=0.1)
        except queue.Empty:
            continue
        if line is None:
            break
        if line == receipt and proc.poll() is None:
            if forwarded and not forward_ready(port, proc):
                break
            proven, why = proves_identity(token)
            if not proven:
                proc.kill()
                proc.wait()
                raise RuntimeError("the app on localhost:%d didn't prove it's the walk's own voidforge-web (%s); "
                                   "nothing else was sent to it" % (port, why))
            return proc
    if proc.poll() is None:
        proc.kill()
    proc.wait()
    log.flush()
    with open(log.name, encoding="utf-8", errors="replace") as f:
        tail = f.read()[-600:]
    raise RuntimeError("voidforge-web didn't say %r within %d s (exit %s); its log %s ends: %s"
                       % (receipt, READY_SECONDS, proc.returncode, log.name, tail))


def forward_ready(port, proc):
    """After a --web-cmd child's receipt: waits, bounded, until 127.0.0.1:port accepts a connection
    (colima's forward of the VM's port), only connecting, never sending. False if the child exits or
    the wait ends first."""
    deadline = time.monotonic() + FORWARD_WAIT_SECONDS
    while time.monotonic() < deadline and proc.poll() is None:
        try:
            socket.create_connection(("127.0.0.1", port), 1).close()
            return True
        except OSError:
            time.sleep(0.01)
    return False


def proves_identity(token):
    """One harmless GET of WEB's / (no cookie, no credentials, redirects not followed), before any
    login request: the answer must carry the per-run token in IDENTITY_HEADER (D-065). Returns
    (proven, why not)."""
    opener = urllib.request.build_opener(PinnedHTTP(), NoRedirect())
    try:
        with opener.open(WEB + "/", timeout=5) as r:
            got = r.headers.get(IDENTITY_HEADER)
    except urllib.error.HTTPError as e:
        got = e.headers.get(IDENTITY_HEADER)
    except OSError as e:
        return False, "GET / failed: %s" % e
    if got != token:
        return False, "GET / answered %s" % ("without the token" if got is None else "another token")
    return True, ""


def start_login(web, dex_origin):
    status, headers, _ = web.request(WEB + "/login")
    if status != 303:
        raise RuntimeError("/login answered %s" % status)
    return headers["Location"], set_cookie_header(headers)


def run(args):
    issuer = args.issuer.rstrip("/")
    origin = re.match(r"https?://[^/]+", issuer).group(0)
    env = dict(os.environ, VOIDFORGE_WEB_CLIENT_SECRET=SECRET)
    log = open(args.log, "w")
    proc = start_web(web_argv(args) + ["--issuer", issuer, "--api-url", args.api], env, log,
                     forwarded=bool(args.web_cmd))
    secrets_seen = []
    try:
        web, dex = Browser(), Browser()
        started = time.monotonic()

        # 1. Logged out.
        status, headers, body = web.request(WEB + "/")
        check(status == 200 and 'href="/login"' in body and "Logged in" not in body,
              "logged out: Home offers the login link", body[:200])
        check(set_cookie_header(headers) is None, "logged out: Home sets no session cookie")
        check(headers.get("Cache-Control") == "no-store", "pages say Cache-Control: no-store")

        # 2. /login.
        location, set_cookie = start_login(web, origin)
        attrs = [a.strip().lower() for a in (set_cookie or "").split(";")[1:]]
        check("httponly" in attrs and "samesite=lax" in attrs and "path=/" in attrs and "secure" not in attrs,
              "the session cookie is HttpOnly, SameSite=Lax, Path=/, and not Secure over http", set_cookie or "none")
        query = urllib.parse.parse_qs(urllib.parse.urlparse(location).query)
        check(location.startswith(issuer + "/auth?"), "/login sends the browser to dex's authorization endpoint",
              location[:80])
        check(query.get("code_challenge_method") == ["S256"] and len(query.get("code_challenge", [""])[0]) == 43
              and "code_verifier" not in query,
              "the authorization request carries an S256 challenge, never the verifier")
        check(all(len(query.get(k, [""])[0]) == 43 for k in ("state", "nonce")),
              "state and nonce are 32 random bytes each, base64url")
        check(query.get("redirect_uri") == [WEB + "/callback"] and query.get("client_id") == ["voidforge-web"],
              "the redirect and client are the registered ones")
        secrets_seen += [query["state"][0], query["nonce"][0]]
        before = web.cookie()

        # 3. dex.
        shown, callback = dex_login(dex, location, origin)
        check(shown, "dex showed its password form")
        callback_query = urllib.parse.parse_qs(urllib.parse.urlparse(callback).query)
        secrets_seen.append(callback_query["code"][0])

        # 4. The callback.
        status, headers, body = web.request(callback)
        check(status == 303 and headers.get("Location") == "/", "the callback logs in and returns to /",
              "%s %s" % (status, body[:200]))
        after = web.cookie()
        check(after is not None and after != before, "the session id changed at login (fixation)")

        # 5. Home, logged in, with the API's answer.
        status, _, body = web.request(WEB + "/")
        check("Logged in as <strong>%s</strong>" % USER in body, "Home shows the viewer's email", body[:300])
        check(re.search(r"You: <span title=\"[0-9a-f-]{36}\">player [0-9a-f]{8}</span> · score -?[\d,]+", body),
              "Home's frame shows the viewer's short name and score, from the API")
        check(re.search(r"Score: -?[\d,]+ \(current\)", body) and "<h1>Your empire</h1>" in body,
              "Home shows a score from /players/me: the API took the token")
        homeworld = re.search(r'Homeworld: <a href="/planets/(\d+)">Planet #\d+</a>', body)
        check(homeworld, "Home shows the homeworld")

        # 5b. The pages, from live data (P4.3).
        if homeworld:
            walk(web, homeworld.group(1))

        # 5c. The forms, through the whole MVP loop (P4.4). A step that can't go on is a
        # failed check naming its request; the steps after the walk still run (P4.5).
        if homeworld:
            try:
                forms_walk(web, homeworld.group(1), args.api.rstrip("/"), issuer)
            except Failed as e:
                fail("the form walk stopped: %s" % e)

        # 6. Replay.
        status, _, body = web.request(callback)
        check(status == 400 and "Login failed" in body, "a replayed callback is refused (400)", str(status))
        check("You are still logged in" in body and "not logged in" not in body,
              "the refusal says the browser is still logged in (QA's F1)", body[:300])
        stranger = Browser()
        status, _, body = stranger.request(callback)
        check(status == 400 and "Logged in" not in stranger.request(WEB + "/")[2],
              "a replayed callback in another browser is refused (400) and logs nobody in", str(status))
        status, _, body = web.request(WEB + "/")
        check("Logged in as" in body, "a refused replay leaves the real login alone")

        # 7. A tampered state, in a fresh browser.
        other, other_dex = Browser(), Browser()
        location, _ = start_login(other, origin)
        _, callback2 = dex_login(other_dex, location, origin)
        parts = urllib.parse.urlparse(callback2)
        q = urllib.parse.parse_qs(parts.query)
        secrets_seen += [q["code"][0], q["state"][0]]
        q["state"] = [q["state"][0][:-2] + ("AA" if not q["state"][0].endswith("AA") else "BB")]
        tampered = parts._replace(query=urllib.parse.urlencode({k: v[0] for k, v in q.items()})).geturl()
        status, _, body = other.request(tampered)
        check(status == 400 and "Login failed" in body, "a tampered state is refused (400)", str(status))
        status, _, body = other.request(WEB + "/")
        check("Logged in" not in body and 'href="/login"' in body, "a tampered state logs nobody in")

        # 8. Logout.
        status, _, _ = web.request(WEB + "/logout")
        check(status == 405, "GET /logout doesn't log out (405)", str(status))
        status, _, body = web.request(WEB + "/")
        check("Logged in as" in body, "still logged in after the GET")
        old = web.cookie()
        status, headers, _ = web.request(WEB + "/logout", data=b"", method="POST")
        check(status == 303 and headers.get("Location") == "/?logged_out", "POST /logout redirects to /?logged_out",
              "%s %s" % (status, headers.get("Location")))
        status, _, body = web.request(WEB + "/?logged_out")
        check("Logged in" not in body and 'href="/login"' in body and "You're logged out." in body,
              "after logout, Home is logged out and says so")
        stale = urllib.request.Request(WEB + "/", headers={"Cookie": "%s=%s" % (COOKIE, old)})
        body = urllib.request.urlopen(stale, timeout=15).read().decode()
        check("Logged in" not in body, "the old session cookie is logged out too (the session is gone server-side)")

        # 9. A second login in the same dex browser.
        location, _ = start_login(web, origin)
        shown, callback3 = dex_login(dex, location, origin)
        check(shown, "a second login shows dex's password form again (no silent login)")
        secrets_seen.append(urllib.parse.parse_qs(urllib.parse.urlparse(callback3).query)["code"][0])
        status, _, _ = web.request(callback3)
        check(status == 303, "the second login completes")
        # The walk's duration, steps 1 to 9; e2e.sh requires this line, so a run that skipped or
        # cut short the walk can't pass (P4.5, D-062).
        if not failures:
            ok("the smoke walk took %d s (login, pages, the forms' MVP loop, replay, logout)"
               % (time.monotonic() - started))
    finally:
        # 11. One SIGTERM, exit 0.
        if proc.poll() is None:
            proc.send_signal(signal.SIGTERM)
            try:
                code = proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                proc.kill()
                code = proc.wait()
                fail("voidforge-web didn't stop within 15 s of SIGTERM")
            check(code == 0, "voidforge-web stopped on one SIGTERM with exit 0", "exit %s" % code)
        else:
            fail("voidforge-web exited early with %s; see %s" % (proc.returncode, args.log))
        log.close()

    # 12. An unreachable API, after the main run so 8081 is free again.
    unreachable_pages = unreachable(args, issuer, origin, env, secrets_seen)

    # 10. No secrets in any page or the log.
    with open(args.log, encoding="utf-8", errors="replace") as f:
        logged = f.read()
    with open(args.log + ".unreachable", encoding="utf-8", errors="replace") as f:
        logged_unreachable = f.read()
    texts = [("the web app's log", logged), ("the unreachable run's log", logged_unreachable)]
    texts += [("a page", p) for p in web.pages + other.pages + unreachable_pages]
    leaked = [(where, s[:6]) for where, text in texts for s in secrets_seen + [SECRET] if s in text]
    leaked += [(where, "eyJ") for where, text in texts if re.search(r"eyJ[A-Za-z0-9_-]{10,}\.", text)]
    check(not leaked, "no token, code, state, nonce or client secret in any page or the log", repr(leaked[:5]))


def walk(web, home):
    """Step 5b: every page answers 200 from live data, each with one fact the API decided."""
    status, _, body = web.request(WEB + "/planets/" + home)
    check(status == 200 and "<h1>Planet #%s</h1>" % home in body and "Owner: you (homeworld)" in body,
          "the homeworld's page names it as yours", "%s %s" % (status, body[:300]))
    check(re.search(r'<th scope="row">Iron ore</th><td>[\d,.]+ t of [\d,.]+ t', body)
          and re.search(r'<th scope="row">Drill #\d+</th>', body)
          and re.search(r'<th scope="row">Refinery #\d+</th>', body),
          "the homeworld's page shows its storage and the Drill and Refinery it starts with")
    check("These values hold until" in body or "Nothing is scheduled" in body,
          "the planet page has its freshness line")
    status, _, body = web.request(WEB + "/fleets")
    check(status == 200 and "<h1>Your fleets</h1>" in body
          and ("You have no fleets." in body or 'id="fleet-' in body),
          "the Fleets page lists the player's fleets", "%s %s" % (status, body[:300]))
    status, _, body = web.request(WEB + "/universe")
    check(status == 200 and 'around <a href="/planets/%s">' % home in body
          and '<a href="/planets/%s">Planet #%s</a></th><td>you (homeworld)</td><td>0.0 u</td>' % (home, home) in body,
          "the Universe page is centred on the homeworld, which is in its table at 0.0 u",
          "%s %s" % (status, body[:300]))
    check(len(re.findall(r'<th scope="row"><a href="/planets/\d+">', body)) >= 6,
          "the Universe page lists the homeworld's neighbours (worldgen promises at least 5)")
    status, _, body = web.request(WEB + "/leaderboard")
    check((status == 200 and "Ranked as of" in body
           and ("<strong>you</strong>" in body or "You're not on the board yet" in body))
          or (status == 503 and "The leaderboard isn't ready yet." in body),
          "the Leaderboard ranks the player, or says it isn't ready", "%s %s" % (status, body[:300]))
    status, _, body = web.request(WEB + "/planets/not-a-number")
    check(status == 400 and "That isn't a planet number." in body, "a bad planet number is refused (400)",
          str(status))


def api_token(issuer):
    """Alice's own ID token through dex's password grant (the bots' public client): the same
    account the web logs in, so the walk can read the API's JSON beside the pages."""
    data = urllib.parse.urlencode({"grant_type": "password", "username": USER, "password": PASSWORD,
                                   "client_id": "voidforge-cli", "scope": "openid"}).encode()
    try:
        with urllib.request.urlopen(urllib.request.Request(issuer + "/token", data=data), timeout=15) as r:
            return json.loads(r.read())["id_token"]
    except (urllib.error.URLError, OSError, KeyError, ValueError) as e:
        raise Failed("POST %s/token (alice's password grant) didn't give an ID token: %s" % (issuer, e))


def api_get(api, token, path):
    """One API read, waiting out a rate limit or a catch-up (D-054, D-049). Any other failure is
    a Failed naming the method and path."""
    for _ in range(10):
        req = urllib.request.Request(api + path, headers={"Authorization": "Bearer " + token})
        try:
            with urllib.request.urlopen(req, timeout=15) as r:
                return json.loads(r.read())
        except urllib.error.HTTPError as e:
            if e.code not in (429, 503):
                raise Failed("GET %s answered %d: %s" % (path, e.code, e.read()[:200].decode("utf-8", "replace")))
            time.sleep(1)
        except (urllib.error.URLError, OSError, ValueError) as e:
            raise Failed("GET %s: no usable answer (%s)" % (path, e))
    raise Failed("GET %s: still 429 or 503 after 10 tries" % path)


def wait_for(what, probe, seconds=90):
    """Polls `probe` once a second until it returns something truthy; fails after `seconds`."""
    deadline = time.monotonic() + seconds
    while time.monotonic() < deadline:
        value = probe()
        if value:
            return value
        time.sleep(1)
    raise Failed("%s didn't happen within %d s" % (what, seconds))


def act_form(body, action, **hidden):
    """The hidden fields of the page's form for `action` (and any given hidden values)."""
    for form in re.findall(r'<form method="post" action="/act">(.*?)</form>', body, re.S):
        fields = [(n, html.unescape(v)) for n, v in
                  re.findall(r'<input type="hidden" name="([^"]+)" value="([^"]*)">', form)]
        values = dict(fields)
        if values.get("action") == action and all(values.get(k) == str(v) for k, v in hidden.items()):
            return fields
    return None


def submit(web, page, action, choose, hidden=None, expect="Done:", to=None):
    """Opens `page`, submits its form for `action` with the fields `choose` sets, paced under the
    write limit (D-054: 2/s), then reads the page it redirects to, which must be `to` when given
    (§5.3: Fleets after Assemble and Send). Returns (fields sent, banner page)."""
    status, _, body = web.request(WEB + page)
    fields = act_form(body, action, **(hidden or {}))
    if not fields:
        raise Failed("GET %s answered %s without the %s form%s" % (
            page, status, action, " for %r" % hidden if hidden else ""))
    sent = fields + list(choose)
    time.sleep(0.6)
    status, headers, _ = web.request(WEB + "/act", data=urllib.parse.urlencode(sent).encode(), method="POST")
    location = headers.get("Location", "")
    _, _, shown = web.request(WEB + location) if status == 303 else (None, None, "")
    banner = re.search(r"<hr>(.*?)(<h1>|$)", shown, re.S)
    request = "POST /act %s" % urllib.parse.urlencode([(k, "[key]" if k == "key" else v) for k, v in sent])
    check(status == 303 and expect in shown and (to is None or location == to),
          "%s: one POST, a 303%s and %r" % (action, " to %s" % to if to else "", expect),
          "%s answered %s, Location %r; banner: %s" % (
              request, status, location,
              banner.group(1)[:400] if banner else re.sub(r"\s+", " ", shown)[:400]))
    return sent, shown


def tonnes(g):
    """Grams as the exact tonnes a cargo field takes: 6 decimals."""
    return "%d.%06d" % (g // 1_000_000, g % 1_000_000)


def load(api, token, planet, grams):
    """`grams`, once the planet holds twice that, as the API shows it: ship builds drain the
    ingots, and a load of 0 would test nothing (run 2 of P4.4's e2e did exactly that)."""
    wait_for("%s t of ingots on %s" % (tonnes(2 * grams), planet), lambda:
             api_get(api, token, planet)["stocks"]["iron_ingot"]["amount_g"] >= 2 * grams, 60)
    return grams


def buildings_of(planet, kind):
    return [b for b in planet["buildings"] if b["type"] == kind]


def forms_walk(web, home, api, issuer):
    """Step 5c: the MVP loop through the web's forms (P4.4, §7's gate walk without a browser), each
    step checked against the API's JSON (T6): build, queue ships, assemble, colonize, transport."""
    token = api_token(issuer)
    me = api_get(api, token, "/players/me")
    check(str(me["homeworld_id"]) == home, "the password-grant token is the web's player (same homeworld)")
    here = "/planets/" + home

    # Build a Shipyard; then the same form again (a double submit) and a form this session never issued.
    yards = len(buildings_of(api_get(api, token, here), "shipyard"))
    sent, _ = submit(web, here, "build_building", [("building_type", "shipyard")])
    planet = api_get(api, token, here)
    check(len(buildings_of(planet, "shipyard")) == yards + 1, "build: the API shows one new Shipyard")
    time.sleep(0.6)
    status, headers, _ = web.request(WEB + "/act", data=urllib.parse.urlencode(sent).encode(), method="POST")
    _, _, shown = web.request(WEB + headers.get("Location", "/"))
    check(status == 303 and "This form was already submitted; this is its result. Nothing was done twice." in shown,
          "a double submit says it was already submitted (§5.2)", re.sub(r"\s+", " ", shown)[:300])
    forged = [(k, "AAAAAAAAAAAAAAAAAAAAAA" if k == "key" else v) for k, v in sent]
    time.sleep(0.6)
    web.request(WEB + "/act", data=urllib.parse.urlencode(forged).encode(), method="POST")
    _, _, shown = web.request(WEB + here)
    check("That form has expired." in shown, "a key the session never issued is refused (Q3)")
    check(len(buildings_of(api_get(api, token, here), "shipyard")) == yards + 1,
          "neither the double submit nor the forged form built a second Shipyard")
    yard = wait_for("the Shipyard becoming operational", lambda: next(
        (b["id"] for b in buildings_of(api_get(api, token, here), "shipyard") if b["state"] == "operational"), None))
    ok("the Shipyard #%s is operational" % yard)

    # Queue a Colony Ship and a Cargo Vessel; wait until both are on the roster.
    for ship in ("colony_ship", "cargo_vessel"):
        submit(web, here, "queue_ship", [("ship_type", ship)], {"shipyard_id": yard})
    builds = [b["ship_type"] for b in api_get(api, token, here)["ship_builds"] if b["shipyard_id"] == yard]
    check(sorted(builds) == ["cargo_vessel", "colony_ship"] or len(builds) < 2,
          "queue: the API shows both builds at the Shipyard", repr(builds))

    def roster():
        r = {s["ship_type"]: s["ship_id"] for s in api_get(api, token, here)["roster"] if s["owner"] == me["player_id"]}
        return r if {"colony_ship", "cargo_vessel"} <= set(r) else None
    ships = wait_for("both ships on the homeworld's roster", roster, 150)
    ok("both ships are on the roster")

    # An unowned neighbour from the Universe page.
    _, _, body = web.request(WEB + "/universe")
    target = re.search(r'<th scope="row"><a href="/planets/(\d+)">Planet #\d+</a></th><td>unowned</td>', body)
    if not target:
        raise Failed("GET /universe listed no unowned neighbour to colonize")
    colony = target.group(1)

    # Assemble both ships with up to 1 t of ingots; the API shows the fleet and its exact cargo.
    loaded = load(api, token, here, 1_000_000)
    submit(web, here, "assemble_fleet",
           [("ship", str(ships["colony_ship"])), ("ship", str(ships["cargo_vessel"])), ("ore", ""),
            ("ingots", tonnes(loaded))],
           expect="Done: a fleet of 2 ships was assembled at Planet #%s." % home, to="/fleets")
    fleets = api_get(api, token, "/players/me/fleets")["fleets"]
    fleet = next((f for f in fleets if f["state"] == "stationed" and str(f["at_planet_id"]) == home
                  and {s["ship_id"] for s in f["ships"]} == set(ships.values())), None)
    check(fleet and fleet["cargo"] == {"iron_ore_g": 0, "iron_ingot_g": loaded},
          "assemble: the API shows the fleet of both ships carrying exactly the %s t loaded" % tonnes(loaded),
          repr(fleet))
    if not fleet:
        raise Failed("GET /players/me/fleets showed no stationed fleet of ships %r at Planet #%s" % (
            sorted(ships.values()), home))
    fid = fleet["fleet_id"]

    # Colonize from the Fleets page; wait until the planet is the player's.
    submit(web, "/fleets", "send_fleet", [("target_planet_id", colony), ("mission", "colonize")], {"fleet_id": fid},
           expect="Done: Fleet #%s left for Planet #%s (Colonize)." % (fid, colony), to="/fleets")
    flying = next((f for f in api_get(api, token, "/players/me/fleets")["fleets"] if f["fleet_id"] == fid), {})
    check(flying.get("state") == "in_transit" and str(flying.get("target_planet_id")) == colony
          and flying.get("mission") == "colonize", "send: the API shows the fleet flying to colonize", repr(flying))
    wait_for("Planet #%s becoming the player's" % colony,
             lambda: int(colony) in api_get(api, token, "/players/me")["planet_ids"], 60)
    planet = api_get(api, token, "/planets/" + colony)
    check(planet["owner"] == me["player_id"] and planet["stocks"]["iron_ingot"]["amount_g"] >= loaded,
          "colonize: the planet is the player's, with the ingots unloaded")
    _, _, body = web.request(WEB + "/planets/" + colony)
    check("Owner: you" in body, "the colony's page says Owner: you")

    # Disband the Cargo Vessel's fleet at the colony, assemble it there with 0.5 t, and Transport it home.
    submit(web, "/fleets", "disband_fleet", [], {"fleet_id": fid})
    on_roster = [s["ship_id"] for s in api_get(api, token, "/planets/" + colony)["roster"]]
    check(ships["cargo_vessel"] in on_roster, "disband: the Cargo Vessel is on the colony's roster")
    carried = load(api, token, "/planets/" + colony, 500_000)
    submit(web, "/planets/" + colony, "assemble_fleet",
           [("ship", str(ships["cargo_vessel"])), ("ingots", tonnes(carried))], to="/fleets")
    fleet = next((f for f in api_get(api, token, "/players/me/fleets")["fleets"]
                  if f["state"] == "stationed" and str(f["at_planet_id"]) == colony), None)
    check(fleet and fleet["cargo"]["iron_ingot_g"] == carried,
          "assemble at the colony: the %s t loaded is aboard" % tonnes(carried), repr(fleet))
    if not fleet:
        raise Failed("GET /players/me/fleets showed no stationed fleet at the colony, Planet #%s" % colony)
    fid = fleet["fleet_id"]
    submit(web, "/fleets", "send_fleet", [("target_planet_id", home), ("mission", "transport")], {"fleet_id": fid},
           expect="Done: Fleet #%s left for Planet #%s (Transport)." % (fid, home), to="/fleets")
    back = wait_for("the transport arriving home", lambda: next(
        (f for f in api_get(api, token, "/players/me/fleets")["fleets"]
         if f["fleet_id"] == fid and f["state"] == "stationed" and str(f["at_planet_id"]) == home), None), 60)
    check(back["cargo"] == {"iron_ore_g": 0, "iron_ingot_g": 0},
          "transport: the fleet arrived home and its cargo was unloaded", repr(back))


def unreachable(args, issuer, origin, env, secrets_seen):
    """Starts the web app against a port nothing listens on, logs in, and reads Home. Returns the
    pages it saw, for step 10, and adds its login's code to `secrets_seen`."""
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        closed = "http://127.0.0.1:%d" % s.getsockname()[1]
    log = open(args.log + ".unreachable", "w")
    proc = start_web(web_argv(args) + ["--issuer", issuer, "--api-url", closed], env, log,
                     after_own_child=True, forwarded=bool(args.web_cmd))
    try:
        web, dex = Browser(), Browser()
        location, _ = start_login(web, origin)
        _, callback = dex_login(dex, location, origin)
        secrets_seen.append(urllib.parse.parse_qs(urllib.parse.urlparse(callback).query)["code"][0])
        status, _, _ = web.request(callback)
        check(status == 303, "unreachable: the login completes without the API")
        status, _, body = web.request(WEB + "/")
        check(status == 502 and "Can't reach the game server. Try again in a moment." in body
              and '<a href="/">Try again</a>' in body and "Logged in as" in body,
              "unreachable: Home is the retry page at 502, logged in", "%s %s" % (status, body[:300]))
        check("panicked" not in body and "reqwest" not in body and "error" not in body.lower(),
              "unreachable: the page shows no trace or error detail", body[:300])
    finally:
        if proc.poll() is None:
            proc.send_signal(signal.SIGTERM)
            try:
                proc.wait(timeout=15)
            except subprocess.TimeoutExpired:
                proc.kill()
                proc.wait()
        log.close()
    return web.pages


def web_argv(args):
    """The web app's command: the binary, or --web-cmd split as a shell would."""
    return shlex.split(args.web_cmd) if args.web_cmd else [args.web]


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    web = parser.add_mutually_exclusive_group(required=True)
    web.add_argument("--web")
    web.add_argument("--web-cmd")
    parser.add_argument("--api", required=True)
    parser.add_argument("--issuer", required=True)
    parser.add_argument("--log", required=True)
    args = parser.parse_args()
    try:
        run(args)
    except Exception as exc:  # The checks couldn't run: say so, never pass.
        print("error: could not run the web login check: %s" % exc, file=sys.stderr)
        sys.exit(2)
    if failures:
        print("error: %d web login check(s) failed" % len(failures), file=sys.stderr)
        sys.exit(1)
    print("ok: the web login works against dex, and the API accepts its token")


if __name__ == "__main__":
    main()
