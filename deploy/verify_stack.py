#!/usr/bin/env python3
"""Verify the local stack by effect: every service healthy and usable from the host and from a
container, with one dex issuer for both (decision log D-014, D-015). `just up` runs this after
`docker compose up --wait`; `just status` runs it on its own.

Checks:
  compose    postgres, dex and otel-lgtm are running and healthy
  postgres   a TCP connection from the host, and `select 1` through psql in the container
  dex        from the host: discovery has the expected issuer, PKCE S256 and grant types, and
             the JWKS has keys; the CLI client gets a device code; the web client's PKCE
             authorization request is accepted and a foreign redirect is refused; client
             credentials are refused; the password grant is refused, or with
             DEX_CONFIG=config.ci.yaml it issues an ID token for a static test user
  container  a host-network container sees the same discovery issuer and JWKS key IDs
  otel-lgtm  Grafana's health API, an OTLP/HTTP trace export accepted, and OTLP/gRPC listening

Usage: verify_stack.py   (run from the repository root)
Exit codes: 0 all checks passed, 1 a check failed, 2 could not check (docker or compose missing).
"""
import base64
import hashlib
import json
import os
import socket
import subprocess
import sys
import urllib.error
import urllib.parse
import urllib.request

COMPOSE = ["docker", "compose", "-f", "deploy/compose.yaml"]
SERVICES = ["postgres", "dex", "otel-lgtm"]
ISSUER = "http://localhost:5556/dex"
CI_CONFIG = os.environ.get("DEX_CONFIG", "config.yaml") == "config.ci.yaml"
PROBE_IMAGE = "curlimages/curl:8.16.0@sha256:463eaf6072688fe96ac64fa623fe73e1dbe25d8ad6c34404a669ad3ce1f104b6"
HOST_NETWORK_HINT = (
    "A host-network container could not reach dex on localhost:5556. Containers that consume the "
    "issuer use host networking (decision log D-015). It works on Linux and colima; Docker Desktop "
    "needs Settings > Resources > Network > 'Enable host networking'."
)
# TEST-ONLY values from deploy/dex/config.yaml.
WEB_CLIENT, WEB_SECRET, WEB_REDIRECT = "voidforge-web", "voidforge-web-local-test-secret", "http://localhost:8081/callback"
CLI_CLIENT = "voidforge-cli"
TEST_USER, TEST_PASSWORD = "alice@voidforge.test", "alice-local-test-password"

failures = []


def ok(message):
    print("ok:   " + message)


def fail(message):
    failures.append(message)
    print("FAIL: " + message)


def run(cmd):
    try:
        return subprocess.run(cmd, stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True)
    except OSError as exc:
        print("error: cannot run " + cmd[0] + " (" + str(exc) + "); could not check the stack", file=sys.stderr)
        sys.exit(2)


class NoRedirect(urllib.request.HTTPRedirectHandler):
    def redirect_request(self, *args, **kwargs):
        return None


def http(url, data=None, headers=None, auth=None):
    """(status, body text, headers); never raises for HTTP error statuses."""
    headers = dict(headers or {})
    if auth:
        headers["Authorization"] = "Basic " + base64.b64encode((auth[0] + ":" + auth[1]).encode()).decode()
    body = None
    if data is not None:
        body = data if isinstance(data, bytes) else urllib.parse.urlencode(data).encode()
        headers.setdefault("Content-Type", "application/x-www-form-urlencoded")
    req = urllib.request.Request(url, data=body, headers=headers)
    opener = urllib.request.build_opener(NoRedirect)
    try:
        with opener.open(req, timeout=10) as resp:
            return resp.status, resp.read().decode(errors="replace"), resp.headers
    except urllib.error.HTTPError as exc:
        return exc.code, exc.read().decode(errors="replace"), exc.headers
    except (urllib.error.URLError, OSError) as exc:
        return None, str(exc), {}


def follow_within_dex(url):
    """Follow redirects while they stay on dex's origin; return the final (status, body, url)."""
    origin = ISSUER.split("/dex")[0]
    for _ in range(5):
        status, body, headers = http(url)
        location = headers.get("Location") if status in (301, 302, 303, 307, 308) else None
        if not location:
            return status, body, url
        url = urllib.parse.urljoin(url, location)
        if not url.startswith(origin + "/"):
            return status, body, url
    return status, body, url


def check_compose():
    proc = run(COMPOSE + ["ps", "--all", "--format", "json"])
    if proc.returncode != 0:
        print(proc.stderr, file=sys.stderr)
        print("error: docker compose ps failed; could not check the stack", file=sys.stderr)
        sys.exit(2)
    text = proc.stdout.strip()
    rows = json.loads(text) if text.startswith("[") else [json.loads(l) for l in text.splitlines() if l.strip()]
    by_service = {r.get("Service"): r for r in rows}
    for service in SERVICES:
        row = by_service.get(service)
        if row is None:
            fail("compose: %s has no container" % service)
        elif row.get("State") != "running" or row.get("Health") != "healthy":
            fail("compose: %s is %s/%s" % (service, row.get("State"), row.get("Health") or "no health"))
        else:
            ok("compose: %s running and healthy (%s)" % (service, row.get("Image")))


def check_postgres():
    try:
        socket.create_connection(("127.0.0.1", 5432), timeout=5).close()
        ok("postgres: accepts TCP connections on 127.0.0.1:5432 from the host")
    except OSError as exc:
        fail("postgres: no TCP connection on 127.0.0.1:5432 from the host (%s)" % exc)
    proc = run(COMPOSE + ["exec", "-T", "postgres", "psql", "--username=voidforge", "--dbname=voidforge",
                          "--tuples-only", "--no-align", "--command=select 1"])
    if proc.returncode == 0 and proc.stdout.strip() == "1":
        ok("postgres: select 1 returns 1 in database voidforge")
    else:
        fail("postgres: select 1 failed (exit %d): %s" % (proc.returncode, (proc.stderr or proc.stdout).strip()))


def check_dex():
    status, body, _ = http(ISSUER + "/.well-known/openid-configuration")
    if status != 200:
        fail("dex: discovery from the host returned %s: %s" % (status, body[:200]))
        return None
    disco = json.loads(body)
    if disco.get("issuer") == ISSUER:
        ok("dex: discovery from the host, issuer %s" % ISSUER)
    else:
        fail("dex: discovery issuer is %r, expected %r" % (disco.get("issuer"), ISSUER))
    grants = set(disco.get("grant_types_supported", []))
    want = {"authorization_code", "refresh_token", "urn:ietf:params:oauth:grant-type:device_code"}
    if CI_CONFIG:
        want.add("password")
    if grants == want:
        ok("dex: grant types are exactly %s" % ", ".join(sorted(grants)))
    else:
        fail("dex: grant types are %s, expected %s" % (sorted(grants), sorted(want)))
    if "S256" in disco.get("code_challenge_methods_supported", []):
        ok("dex: PKCE S256 supported")
    else:
        fail("dex: PKCE S256 not advertised")

    kids = None
    jwks_uri = disco.get("jwks_uri", "")
    status, body, _ = http(jwks_uri)
    if status == 200 and jwks_uri.startswith(ISSUER + "/"):
        kids = sorted(k.get("kid") for k in json.loads(body).get("keys", []))
        if kids:
            ok("dex: JWKS at %s has %d signing key(s)" % (jwks_uri, len(kids)))
        else:
            fail("dex: JWKS at %s has no keys" % jwks_uri)
    else:
        fail("dex: JWKS at %r returned %s" % (jwks_uri, status))

    # The public CLI client can start the device flow.
    status, body, _ = http(disco.get("device_authorization_endpoint", ISSUER + "/device/code"),
                           data={"client_id": CLI_CLIENT, "scope": "openid email profile"})
    if status == 200 and "device_code" in body:
        ok("dex: public client %s starts the device code flow" % CLI_CLIENT)
    else:
        fail("dex: device code request for %s returned %s: %s" % (CLI_CLIENT, status, body[:200]))

    # The confidential web client's PKCE authorization request is accepted; a foreign redirect is not.
    verifier = base64.urlsafe_b64encode(os.urandom(32)).rstrip(b"=").decode()
    challenge = base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    params = {"client_id": WEB_CLIENT, "redirect_uri": WEB_REDIRECT, "response_type": "code",
              "scope": "openid email", "state": "verify", "code_challenge": challenge,
              "code_challenge_method": "S256"}
    status, body, final = follow_within_dex(disco["authorization_endpoint"] + "?" + urllib.parse.urlencode(params))
    if status == 200 and "/auth/local/login" in final:
        ok("dex: web client %s PKCE authorization request reaches the login page" % WEB_CLIENT)
    else:
        fail("dex: web client PKCE authorization request ended at %s with %s: %s" % (final, status, body[:200]))
    params["redirect_uri"] = "http://localhost:9999/elsewhere"
    status, body, final = follow_within_dex(disco["authorization_endpoint"] + "?" + urllib.parse.urlencode(params))
    if status == 400 and "Unregistered redirect_uri" in body:
        ok("dex: web client refuses an unregistered redirect URI (HTTP 400)")
    else:
        fail("dex: unregistered redirect URI was not refused (ended at %s with %s)" % (final, status))

    token = disco["token_endpoint"]
    status, body, _ = http(token, data={"grant_type": "client_credentials", "scope": "openid"},
                           auth=(WEB_CLIENT, WEB_SECRET))
    if status is not None and status >= 400:
        ok("dex: client credentials grant refused (HTTP %s)" % status)
    else:
        fail("dex: client credentials grant was not refused (HTTP %s)" % status)

    status, body, _ = http(token, data={"grant_type": "password", "username": TEST_USER,
                                        "password": TEST_PASSWORD, "scope": "openid email"},
                           auth=(WEB_CLIENT, WEB_SECRET))
    if CI_CONFIG:
        claims = {}
        if status == 200 and "id_token" in body:
            payload = json.loads(body)["id_token"].split(".")[1]
            claims = json.loads(base64.urlsafe_b64decode(payload + "=" * (-len(payload) % 4)))
        if claims.get("iss") == ISSUER and claims.get("aud") == WEB_CLIENT and claims.get("email") == TEST_USER:
            ok("dex: CI config issues %s an ID token through the password grant (iss %s, aud %s)"
               % (TEST_USER, ISSUER, WEB_CLIENT))
        else:
            fail("dex: CI password grant for %s returned %s: %s" % (TEST_USER, status, body[:200]))
    elif status is not None and status >= 400:
        ok("dex: password grant refused in the local config (HTTP %s)" % status)
    else:
        fail("dex: password grant was not refused in the local config (HTTP %s)" % status)
    return kids


def check_container_view(host_kids):
    url = ISSUER + "/.well-known/openid-configuration"
    proc = run(["docker", "run", "--rm", "--network", "host", PROBE_IMAGE, "-fsS", "--max-time", "10", url])
    if proc.returncode != 0:
        fail("container: discovery from a host-network container failed: %s" % proc.stderr.strip())
        print("hint: " + HOST_NETWORK_HINT)
        return
    disco = json.loads(proc.stdout)
    if disco.get("issuer") == ISSUER:
        ok("container: a host-network container sees the same issuer %s" % ISSUER)
    else:
        fail("container: issuer seen from a container is %r, expected %r" % (disco.get("issuer"), ISSUER))
    proc = run(["docker", "run", "--rm", "--network", "host", PROBE_IMAGE, "-fsS", "--max-time", "10",
                disco.get("jwks_uri", "")])
    kids = sorted(k.get("kid") for k in json.loads(proc.stdout).get("keys", [])) if proc.returncode == 0 else None
    if kids and kids == host_kids:
        ok("container: JWKS from the container has the same key IDs as from the host")
    else:
        fail("container: JWKS key IDs from the container %s differ from the host's %s" % (kids, host_kids))


def check_otel():
    status, body, _ = http("http://localhost:3000/api/health")
    if status == 200 and json.loads(body).get("database") == "ok":
        ok("otel-lgtm: Grafana health OK on localhost:3000")
    else:
        fail("otel-lgtm: Grafana health returned %s: %s" % (status, body[:200]))
    status, body, _ = http("http://localhost:4318/v1/traces", data=b'{"resourceSpans":[]}',
                           headers={"Content-Type": "application/json"})
    if status == 200:
        ok("otel-lgtm: OTLP/HTTP trace export accepted on localhost:4318")
    else:
        fail("otel-lgtm: OTLP/HTTP export returned %s: %s" % (status, body[:200]))
    try:
        socket.create_connection(("127.0.0.1", 4317), timeout=5).close()
        ok("otel-lgtm: OTLP/gRPC listening on 127.0.0.1:4317")
    except OSError as exc:
        fail("otel-lgtm: OTLP/gRPC not listening on 127.0.0.1:4317 (%s)" % exc)


def main():
    if not os.path.isfile("deploy/compose.yaml"):
        print("error: run from the repository root; could not check the stack", file=sys.stderr)
        return 2
    print("dex config: " + ("config.ci.yaml (CI, password grant on)" if CI_CONFIG else "config.yaml (local)"))
    check_compose()
    check_postgres()
    kids = check_dex()
    check_container_view(kids)
    check_otel()
    if failures:
        print("error: %d stack check(s) failed" % len(failures), file=sys.stderr)
        return 1
    print("ok: the local stack is healthy: postgres, dex (issuer %s, host and container) and otel-lgtm" % ISSUER)
    return 0


if __name__ == "__main__":
    sys.exit(main())
