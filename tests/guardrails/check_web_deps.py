#!/usr/bin/env python3
"""Fail if voidforge-web depends on any workspace crate other than api-types and telemetry.

Technical Plan, Project layering: voidforge-web's only workspace dependencies are
voidforge-api-types and voidforge-telemetry; it never links core, host, store or server.
The check covers declared dependencies of every kind (normal, dev, build), including
renamed ones and dependencies cargo ignores because the target has no library, and the
full transitive graph that cargo resolves for voidforge-web.

Usage: check_web_deps.py [workspace-dir]
Exit codes: 0 isolated, 1 violation found, 2 could not check (cargo or metadata error).
"""
import json
import os
import subprocess
import sys

WEB = "voidforge-web"
ALLOWED = {"voidforge-api-types", "voidforge-telemetry"}


def error(message):
    print("error: " + message + "; could not check voidforge-web's dependencies", file=sys.stderr)
    sys.exit(2)


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else "."
    if not os.path.isdir(root):
        error("no such directory: " + root)
    try:
        proc = subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--all-features"],
            cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True,
        )
    except OSError as exc:
        error("cannot run cargo (" + str(exc) + ")")
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        error("cargo metadata failed (exit " + str(proc.returncode) + ")")
    try:
        meta = json.loads(proc.stdout)
    except ValueError as exc:
        error("cargo metadata printed invalid JSON (" + str(exc) + ")")

    members = set(meta["workspace_members"])
    by_id = {p["id"]: p for p in meta["packages"]}
    workspace = {by_id[i]["name"]: by_id[i] for i in members}
    if WEB not in workspace:
        error(WEB + " is not a workspace member")
    web = workspace[WEB]
    web_dir = os.path.dirname(web["manifest_path"])
    workspace_dirs = {os.path.realpath(os.path.dirname(p["manifest_path"])): n for n, p in workspace.items()}

    violations = []

    # 1. Declared dependencies, including ones cargo drops from the resolved graph.
    for dep in web["dependencies"]:
        name = dep["name"]
        if dep.get("path"):
            name = workspace_dirs.get(os.path.realpath(dep["path"]), name)
        if name in workspace and name not in ALLOWED and name != WEB:
            kind = dep.get("kind") or "normal"
            shown = name if not dep.get("rename") else name + " (as " + dep["rename"] + ")"
            violations.append("declared " + kind + " dependency: " + WEB + " -> " + shown)

    # 2. The resolved graph, every edge kind, walked from voidforge-web.
    nodes = {n["id"]: n for n in (meta.get("resolve") or {}).get("nodes", [])}
    if web["id"] not in nodes:
        error("voidforge-web is missing from the resolved dependency graph")
    seen = {web["id"]: [WEB]}
    queue = [web["id"]]
    while queue:
        current = queue.pop(0)
        for dep in nodes[current]["deps"]:
            child = dep["pkg"]
            if child in seen:
                continue
            path = seen[current] + [by_id[child]["name"]]
            seen[child] = path
            queue.append(child)
            name = by_id[child]["name"]
            if child in members and name not in ALLOWED:
                kinds = ",".join(sorted({k.get("kind") or "normal" for k in dep["dep_kinds"]}))
                violations.append("resolved " + kinds + " path: " + " -> ".join(path))

    if violations:
        for v in sorted(set(violations)):
            print("violation: " + v)
        print("error: voidforge-web may depend only on " + ", ".join(sorted(ALLOWED))
              + " among workspace crates (Technical Plan, Project layering)", file=sys.stderr)
        return 1
    print("ok: voidforge-web depends on no workspace crate besides " + ", ".join(sorted(ALLOWED)))
    return 0


if __name__ == "__main__":
    sys.exit(main())
