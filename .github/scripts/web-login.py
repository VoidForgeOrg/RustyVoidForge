#!/usr/bin/env python3
"""The web login by effect against a real dex (Technical Plan P4.1; decision log D-057).

Starts `voidforge-web` on localhost:8081 (dex registers the redirect by exact string), then
logs alice in and out as a browser would, with two cookie jars: one for dex's pages and one for
the web app. Nothing here runs a browser; dex's pages are driven over HTTP as device-login.py
does.

  1  logged out:   Home offers a login link, and no session cookie is set
  2  login:        /login sets the cookie (HttpOnly, SameSite=Lax, Path=/, not Secure over
                   http) and sends the browser to dex with S256 PKCE, state and nonce
  3  dex:          dex shows its password form, and sends the browser back with a code
  4  callback:     303 to /, with a NEW session id (fixation)
  5  Home:         "Logged in as alice@voidforge.test", the short name and a score, which come
                   from the API's /players/me: the API accepted the web app's token
  6  replay:       the same callback again is refused (400), with or without the cookie
  7  tampered:     a callback whose state was changed is refused (400) and logs nobody in
  8  logout:       a GET can't log out; the POST does, and the old cookie is logged out too
  9  second login: dex shows its password form again (A2: no silent login)
  10 no secrets:   no token, code, state or the client secret in any page or the web app's log
  11 stop:         one SIGTERM, exit 0

Usage: web-login.py --web <voidforge-web> --api <url> --issuer <dex issuer> --log <file>
Exit codes: 0 every check passed; 1 a check failed; 2 the checks couldn't run.
"""
import argparse
import os
import re
import signal
import subprocess
import sys
import time
import urllib.error
import urllib.parse
import urllib.request
from http.cookiejar import CookieJar

WEB = "http://localhost:8081"
SECRET = "voidforge-web-local-test-secret"  # TEST-ONLY, from deploy/dex/config*.yaml
USER, PASSWORD = "alice@voidforge.test", "alice-local-test-password"
COOKIE = "voidforge_session"

failures = []


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


class NoRedirect(urllib.request.HTTPRedirectHandler):
    """Every redirect is returned, not followed, so each hop can be checked."""

    def redirect_request(self, req, fp, code, msg, headers, newurl):
        return None


class Browser:
    """One cookie jar and an opener that never follows redirects."""

    def __init__(self):
        self.jar = CookieJar()
        self.opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(self.jar), NoRedirect())
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


def wait_ready(proc):
    for _ in range(100):
        if proc.poll() is not None:
            return False
        try:
            urllib.request.urlopen(WEB + "/", timeout=1).read()
            return True
        except (urllib.error.URLError, ConnectionError, OSError):
            time.sleep(0.1)
    return False


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
    proc = subprocess.Popen([args.web, "--issuer", issuer, "--api-url", args.api], env=env,
                            stdout=log, stderr=subprocess.STDOUT, stdin=subprocess.DEVNULL)
    secrets_seen = []
    try:
        if not wait_ready(proc):
            raise RuntimeError("voidforge-web didn't become ready on %s; see %s" % (WEB, args.log))
        web, dex = Browser(), Browser()

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
        check(re.search(r"You appear to others as <span title=\"[0-9a-f-]{36}\">player [0-9a-f]{8}</span>", body),
              "Home shows the viewer's short name, from the API")
        check(re.search(r"Score: -?\d+", body), "Home shows a score from /players/me: the API took the token")

        # 6. Replay.
        status, _, body = web.request(callback)
        check(status == 400 and "Login failed" in body, "a replayed callback is refused (400)", str(status))
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
        check(status == 303 and headers.get("Location") == "/", "POST /logout redirects to /", str(status))
        status, _, body = web.request(WEB + "/")
        check("Logged in" not in body and 'href="/login"' in body, "after logout, Home is logged out")
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

    # 10. No secrets in any page or the log.
    with open(args.log, encoding="utf-8", errors="replace") as f:
        logged = f.read()
    texts = [("the web app's log", logged)] + [("a page", p) for p in web.pages + other.pages]
    leaked = [(where, s[:6]) for where, text in texts for s in secrets_seen + [SECRET] if s in text]
    leaked += [(where, "eyJ") for where, text in texts if re.search(r"eyJ[A-Za-z0-9_-]{10,}\.", text)]
    check(not leaked, "no token, code, state, nonce or client secret in any page or the log", repr(leaked[:5]))


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--web", required=True)
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
