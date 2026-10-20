#!/usr/bin/env python3
"""The terminal login by effect against a real dex (Technical Plan P3.7; decision log D-053).

Runs `voidforge-bots login` as a person would, approves its code by driving dex's own pages over
HTTP (the verification page, the user code, the local login form, as a static test user), then
plays a little through the API with the saved login:

  login     exits 0 once the code is approved; the token file is 0600 in a 0700 directory
  me        the player (the first call joins), as JSON
  get       /leaderboard and /players/me/fleets, as JSON; a URL instead of a path is refused (2)
  command   a build on the homeworld; the idempotency key it used is printed, and a repeat with
            --key returns the same answer
  refusals  a tampered token (8, "log in again"), an expired one (8), a group-readable file (8,
            with the chmod fix), no login after logout (8)
  secrets   no output from any of it contains the token or its subject

It uses the device grant, which the default dex config has (no password grant). .github/scripts/
e2e.sh runs it against the e2e run's serve; it needs no other state.

Usage: device-login.py --bots <voidforge-bots> --api <url> --issuer <dex issuer>
           [--user bob@voidforge.test --password bob-local-test-password]
Exit codes: 0 every check passed, 1 a check failed, 2 could not run.
"""
import argparse
import base64
import http.cookiejar
import json
import os
import re
import stat
import subprocess
import sys
import tempfile
import time
import urllib.error
import urllib.parse
import urllib.request

failures = []


def ok(message):
    print("ok: " + message)


def fail(message):
    failures.append(message)
    print("FAIL: " + message)


def bots(args, env, timeout=60):
    proc = subprocess.run([ARGS.bots] + args, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                          universal_newlines=True, timeout=timeout)
    OUTPUTS.append(proc.stdout + proc.stderr)
    return proc


class Refused(Exception):
    """dex refused a step of the approval: a product failure, with dex's own words."""


def approve(link, user, password, issuer):
    """Drives dex's pages as a browser would: the verification page, the code, the login form."""
    jar = http.cookiejar.CookieJar()
    opener = urllib.request.build_opener(urllib.request.HTTPCookieProcessor(jar))

    class Browser:
        @staticmethod
        def open(url, data=None, timeout=10):
            try:
                return opener.open(url, data, timeout=timeout)
            except urllib.error.HTTPError as e:
                text = re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", e.read().decode(errors="replace")))
                raise Refused("dex answered HTTP %s at %s: %s" % (e.code, url.split("?")[0], text.strip()[:200]))
    browser = Browser()
    page = browser.open(link, timeout=10).read().decode()
    user_code = re.search(r'name="user_code"[^>]*value="([^"]+)"', page)
    if not user_code:
        raise RuntimeError("dex's verification page has no user code: " + page[:300])
    origin = re.match(r"(https?://[^/]+)", issuer).group(1)
    form = re.search(r'<form[^>]*action="([^"]+)"', page).group(1).replace("&amp;", "&")
    response = browser.open(origin + form, urllib.parse.urlencode({"user_code": user_code.group(1)}).encode(),
                            timeout=10)
    page = response.read().decode()
    login = re.search(r'<form method="post" action="([^"]+)"', page)
    if not login or "password" not in page:
        raise RuntimeError("dex didn't show its login form after the user code (at %s): %s"
                           % (response.geturl(), re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", page))[:300]))
    response = browser.open(origin + login.group(1).replace("&amp;", "&"),
                            urllib.parse.urlencode({"login": user, "password": password}).encode(), timeout=10)
    page = re.sub(r"\s+", " ", re.sub(r"<[^>]+>", " ", response.read().decode()))
    if "Login Successful" not in page:
        raise RuntimeError("dex didn't confirm the login (at %s): %s" % (response.geturl(), page[:300]))


def main():
    work = tempfile.mkdtemp(prefix="vf-device-login-")
    token_file = os.path.join(work, "config", "voidforge", "token.json")
    env = dict(os.environ, VOIDFORGE_TOKEN_FILE=token_file, VOIDFORGE_API=ARGS.api, VOIDFORGE_ISSUER=ARGS.issuer)

    # login: start it, read the link from stderr, approve it, and wait for the exit.
    proc = subprocess.Popen([ARGS.bots, "login"], env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                            universal_newlines=True)
    shown, link, deadline = [], None, time.time() + 30
    while link is None and time.time() < deadline:
        line = proc.stderr.readline()
        if not line:
            break
        shown.append(line)
        match = re.match(r"\s+(https?://\S+user_code=\S+)\s*$", line)
        link = match.group(1) if match else None
    if link is None:
        proc.kill()
        fail("login showed no link to open: %r" % "".join(shown))
        return
    try:
        approve(link, ARGS.user, ARGS.password, ARGS.issuer)
    except Refused as refused:
        fail("approving the code in dex's pages: %s" % refused)
        return
    finally:
        # A failed approval must not leave login polling dex until its code expires.
        if proc.poll() is None and (failures or sys.exc_info()[0] is not None):
            proc.kill()
            proc.wait()
    # Not communicate(): it reads the pipes directly and would drop what readline() has buffered.
    proc.wait(timeout=30)
    out, err = proc.stdout.read(), proc.stderr.read()
    OUTPUTS.append("".join(shown) + out + err)
    said = "".join(shown) + err
    if "Waiting for you to approve the login; the code expires in" in said and "enter the code" in said:
        ok("login showed the link, the plain URL with the code, and that it was waiting")
    else:
        fail("login's prompt: %r" % said[:400])
    if proc.returncode == 0 and "Logged in." in err and out == "":
        ok("login: the code approved through dex's pages, exit 0, nothing on stdout")
    else:
        fail("login: exit %s, stdout %r, stderr %r" % (proc.returncode, out, err[-300:]))
        return
    modes = (stat.S_IMODE(os.stat(token_file).st_mode), stat.S_IMODE(os.stat(os.path.dirname(token_file)).st_mode))
    if modes == (0o600, 0o700):
        ok("the token file is 0600 in a 0700 directory")
    else:
        fail("the token file and directory are %o and %o, not 600 and 700" % modes)
    saved = json.load(open(token_file))
    token = saved["id_token"]
    claims = json.loads(base64.urlsafe_b64decode(token.split(".")[1] + "=="))
    secrets = {"the token": token, "its subject": claims["sub"]}
    if claims.get("aud") == "voidforge-cli" and claims.get("iss") == ARGS.issuer:
        ok("the token is dex's, for voidforge-cli (aud and iss checked)")
    else:
        fail("the token's aud %r and iss %r" % (claims.get("aud"), claims.get("iss")))

    # me, get, command: play a little.
    me = bots(["me"], env)
    player = json.loads(me.stdout) if me.returncode == 0 else {}
    if "player_id" in player and player["homeworld_id"] in player["planet_ids"]:
        ok("me: the player, with its homeworld (the API accepts the device-flow token)")
    else:
        fail("me: exit %s, %r %r" % (me.returncode, me.stdout[:200], me.stderr[-200:]))
        return
    for path in ["/leaderboard", "/players/me/fleets"]:
        got = bots(["get", path], env)
        try:
            json.loads(got.stdout)
            ok("get %s: JSON, exit 0" % path)
        except ValueError:
            fail("get %s: exit %s, %r" % (path, got.returncode, got.stderr[-200:]))
    url = bots(["get", "http://example.com/"], env)
    if url.returncode == 2:
        ok("get refuses a URL (exit 2): only API paths")
    else:
        fail("get with a URL: exit %s" % url.returncode)
    build = json.dumps({"type": "build_building",
                        "payload": {"planet_id": player["homeworld_id"], "building_type": "drill"}})
    first = bots(["command", build], env)
    key = re.search(r"Idempotency key: (\S+)", first.stderr)
    again = bots(["command", build, "--key", key.group(1)], env) if key else None
    if first.returncode == 0 and key and again.returncode == 0 and json.loads(again.stdout) == json.loads(first.stdout):
        ok("command: accepted, its key printed, and a repeat with --key answers the same")
    else:
        fail("command: exit %s, %r %r" % (first.returncode, first.stdout[:200], first.stderr[-200:]))
    typo = bots(["command", '{"type":"build_building","payload":{"planet":1}}'], env)
    if typo.returncode == 2 and "not a command" in typo.stderr:
        ok("command refuses JSON that isn't a command (exit 2), before calling the API")
    else:
        fail("command with a typo: exit %s, %r" % (typo.returncode, typo.stderr[-200:]))

    # Refusals: each says to log in again, never a raw 401.
    def with_saved(change, label, expect):
        data = dict(saved)
        change(data)
        with open(token_file, "w") as f:
            json.dump(data, f)
        os.chmod(token_file, 0o600)
        got = bots(["me"], env)
        if got.returncode == 8 and expect in got.stderr and "401" not in got.stderr:
            ok("%s: exit 8, %r" % (label, got.stderr.strip()))
        else:
            fail("%s: exit %s, %r" % (label, got.returncode, got.stderr[-300:]))
    with_saved(lambda d: d.update(id_token=token[:-4] + ("AAAA" if not token.endswith("AAAA") else "BBBB")),
               "a tampered token", "your login has expired; log in again")
    with_saved(lambda d: d.update(expires_at=int(time.time()) - 1), "an expired login",
               "your login has expired; log in again")
    with open(token_file, "w") as f:
        json.dump(saved, f)
    os.chmod(token_file, 0o644)
    readable = bots(["me"], env)
    if readable.returncode == 8 and "chmod 600" in readable.stderr:
        ok("a group- and world-readable login is refused with the chmod fix (exit 8)")
    else:
        fail("a readable login: exit %s, %r" % (readable.returncode, readable.stderr[-300:]))
    os.chmod(token_file, 0o600)
    out = bots(["logout"], env)
    after = bots(["me"], env)
    if out.returncode == 0 and not os.path.exists(token_file) and after.returncode == 8:
        ok("logout deletes the login; me then says to log in (exit 8)")
    else:
        fail("logout: exit %s, file left %s, me after exit %s" % (out.returncode, os.path.exists(token_file),
                                                                   after.returncode))

    # Secrets: nothing printed by any command contains the token or its subject.
    leaked = [name for name, value in secrets.items() if any(value in text for text in OUTPUTS)]
    if leaked:
        fail("output contains %s" % ", ".join(leaked))
    else:
        ok("no output (%d commands) contains the token or its subject" % len(OUTPUTS))


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--bots", required=True)
    parser.add_argument("--api", required=True)
    parser.add_argument("--issuer", required=True)
    parser.add_argument("--user", default="bob@voidforge.test")
    parser.add_argument("--password", default="bob-local-test-password")  # TEST-ONLY (deploy/dex)
    ARGS = parser.parse_args()
    OUTPUTS = []
    try:
        main()
    except (OSError, RuntimeError, subprocess.TimeoutExpired, urllib.error.URLError) as exc:
        print("error: could not run the device login check: %s" % exc, file=sys.stderr)
        sys.exit(2)
    if failures:
        print("error: %d device login check(s) failed" % len(failures), file=sys.stderr)
        sys.exit(1)
    print("ok: the terminal login works against dex, and the API accepts its token")
