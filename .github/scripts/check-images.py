#!/usr/bin/env python3
"""The images' checks (Technical Plan P5.2; decision log D-065).

  --static  (the lint job) dockerfiles/Dockerfile pins every base by digest, its builder is the
            toolchain rust-toolchain.toml names, and deploy/compose.images.yaml is a project of its
            own: named voidforge-images, none of the owner's services (postgres, dex, otel-lgtm),
            no volumes, no published ports, no build section, VOIDFORGE_DATABASE_URL without a
            default, and a server stop grace period that covers a clean stop (40 s). Every docker
            compose call on that file names the project with -p, since an inherited
            COMPOSE_PROJECT_NAME outranks `name:` (R1-1).
  --built   (the images job, after `just images`) voidforge-server:local and voidforge-web:local
            run as 65532:65532, are at most MAX_MB each, and hold no shell, no cargo and no
            sources; prints each one's size and user.

Usage: check-images.py --static | --built   (run from the repository root)
Exit codes: 0 every check passed; 1 a check failed; 2 the checks couldn't run.
"""
import json
import os
import re
import subprocess
import sys

DOCKERFILE = "dockerfiles/Dockerfile"
COMPOSE = "deploy/compose.images.yaml"
PROJECT = "voidforge-images"
OWNER_SERVICES = {"postgres", "dex", "otel-lgtm"}
IMAGES = {"voidforge-server:local": "/usr/local/bin/voidforge-server", "voidforge-web:local": "/usr/local/bin/voidforge-web"}
USER = "65532:65532"
MAX_MB = 64
# Paths a runtime layer must not hold: a shell, the toolchain, the sources.
ABSENT = ["/bin/sh", "/usr/local/cargo", "/usr/local/rustup", "/src"]

failures = []


def check(condition, message):
    print(("pass: " if condition else "FAIL: ") + message)
    if not condition:
        failures.append(message)


# Every file that runs docker compose on the images' file (R1-1).
CALLERS = [".github/scripts/e2e.sh", ".github/scripts/images-compose.sh", "justfile", ".github/workflows/ci.yml"]


def static(dockerfile=DOCKERFILE, compose=COMPOSE, toolchain="rust-toolchain.toml", callers=CALLERS):
    text = open(dockerfile).read()
    stages = set()
    froms = re.findall(r"^FROM\s+(\S+)(?:\s+AS\s+(\S+))?\s*$", text, re.M | re.I)
    check(bool(froms), "%s has FROM lines" % dockerfile)
    for image, stage in froms:
        pinned = re.fullmatch(r"[^@\s]+@sha256:[0-9a-f]{64}", image) is not None
        check(pinned or image in stages, "FROM %s is pinned by digest (or an earlier stage)" % image)
        if stage:
            stages.add(stage)
    channel = re.search(r'^channel\s*=\s*"([^"]+)"', open(toolchain).read(), re.M).group(1)
    builder = froms[0][0] if froms else ""
    check(builder.startswith("rust:%s-" % channel),
          "the builder (%s) is the toolchain rust-toolchain.toml names (%s)" % (builder.split("@")[0], channel))
    check("RUSTUP_TOOLCHAIN=%s" % channel in text, "RUSTUP_TOOLCHAIN names the same toolchain (%s)" % channel)

    env = dict(os.environ, VOIDFORGE_DATABASE_URL="postgres://x@localhost/voidforge_e2e_0_0",
               VOIDFORGE_WEB_CLIENT_SECRET="x")
    # The file's own name is what's checked here, whatever this shell inherits.
    env.pop("COMPOSE_PROJECT_NAME", None)
    out = subprocess.run(["docker", "compose", "-f", compose, "config", "--format", "json"],
                         env=env, capture_output=True, text=True)
    if out.returncode != 0:
        check(False, "docker compose config reads %s: %s" % (compose, out.stderr.strip()))
        return
    config = json.loads(out.stdout)
    check(config.get("name") == PROJECT, "the project is %s (got %r), never the owner's voidforge" % (PROJECT, config.get("name")))
    services = config.get("services", {})
    check(not OWNER_SERVICES & set(services), "no owner service in it (%s)" % ", ".join(sorted(services)))
    check(not config.get("volumes"), "no volumes")
    for name, service in services.items():
        check(not service.get("build"), "%s has no build section (only `just images` builds, with its commit)" % name)
        check(not service.get("ports"), "%s publishes no ports" % name)
        check(service.get("network_mode") == "host", "%s uses host networking (the issuer's URL)" % name)
        check(not service.get("volumes"), "%s mounts nothing" % name)
    # serve's clean stop is bounded by its drain (5 s by default), the 10 s join and the 5 s check
    # (D-050): SIGKILL must never come before them, with room to spare.
    grace = services.get("server", {}).get("stop_grace_period", "")
    seconds = sum(int(n) * {"h": 3600, "m": 60, "s": 1}[u] for n, u in re.findall(r"(\d+)([hms])", grace))
    check(seconds >= 40, "the server's stop_grace_period covers a clean stop: at least 40 s (%r)" % grace)
    # R1-1: an inherited COMPOSE_PROJECT_NAME outranks `name:`, so every command names the project.
    pinned = subprocess.run(["docker", "compose", "-f", compose, "-p", PROJECT, "config", "--format", "json"],
                            env=dict(env, COMPOSE_PROJECT_NAME="voidforge"), capture_output=True, text=True)
    check(pinned.returncode == 0 and json.loads(pinned.stdout).get("name") == PROJECT,
          "with COMPOSE_PROJECT_NAME=voidforge inherited, -p %s still names the project %s" % (PROJECT, PROJECT))
    for path in callers:
        for n, line in enumerate(open(path), 1):
            if "docker compose" in line and os.path.basename(compose) in line:
                check(re.search(r"\s-p\s", line) is not None,
                      "%s:%d names its compose project with -p" % (path, n))
    env.pop("VOIDFORGE_DATABASE_URL")
    out = subprocess.run(["docker", "compose", "-f", compose, "config"], env=env, capture_output=True, text=True)
    check(out.returncode != 0 and "VOIDFORGE_DATABASE_URL" in out.stderr,
          "VOIDFORGE_DATABASE_URL has no default: compose refuses without it")


def docker(*args):
    """Runs docker; stdout may be a tar stream (`docker cp ... -`), so it's decoded leniently."""
    out = subprocess.run(["docker", *args], capture_output=True)
    return subprocess.CompletedProcess(out.args, out.returncode, out.stdout.decode("utf-8", "replace"),
                                       out.stderr.decode("utf-8", "replace"))


def built():
    for image, binary in IMAGES.items():
        out = docker("image", "inspect", image)
        if out.returncode != 0:
            check(False, "%s is built (just images)" % image)
            continue
        info = json.loads(out.stdout)[0]
        size_mb = info["Size"] / 1_000_000
        user = info["Config"].get("User", "")
        print("%s: %s, %.1f MB, user %s" % (image, info["Id"], size_mb, user or "(root)"))
        check(user == USER, "%s runs as %s (got %r)" % (image, USER, user))
        check(size_mb <= MAX_MB, "%s is at most %d MB (%.1f MB)" % (image, MAX_MB, size_mb))
        check(info["Config"].get("Entrypoint") == [binary], "%s's entrypoint is its binary, exec form" % image)
        created = docker("create", image)
        if created.returncode != 0:
            check(False, "docker create %s: %s" % (image, created.stderr.strip()))
            continue
        container = created.stdout.strip()
        try:
            check(docker("cp", "%s:%s" % (container, binary), "-").returncode == 0, "%s holds %s" % (image, binary))
            for path in ABSENT:
                check(docker("cp", "%s:%s" % (container, path), "-").returncode != 0, "%s holds no %s" % (image, path))
        finally:
            docker("rm", container)


def main():
    if not os.path.isfile(DOCKERFILE):
        print("error: run from the repository root", file=sys.stderr)
        sys.exit(2)
    mode = sys.argv[1:]
    if mode == ["--static"]:
        static()
    elif mode == ["--built"]:
        built()
    else:
        print(__doc__.split("\n\n")[-2], file=sys.stderr)
        sys.exit(2)
    if failures:
        print("error: %d image check(s) failed" % len(failures), file=sys.stderr)
        sys.exit(1)
    print("ok: the image checks passed")


if __name__ == "__main__":
    main()
