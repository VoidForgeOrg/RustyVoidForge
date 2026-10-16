#!/usr/bin/env python3
"""Fail if voidforge-web depends on any workspace crate other than api-types and telemetry.

Technical Plan, Project layering: voidforge-web's only workspace dependencies are
voidforge-api-types and voidforge-telemetry; it never links core, host, store or server.
Two passes, over every dependency kind (normal, dev, build):
  resolved  the graph cargo resolves from voidforge-web, walked transitively;
  declared  the manifest declarations of voidforge-web and of every workspace crate it reaches,
            by name, by path or renamed. This catches declarations cargo drops from the graph,
            such as a path dependency on a binary-only crate. For those it enforces the
            declaration; cargo links no library from a binary-only crate.

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
    workspace_dirs = {os.path.realpath(os.path.dirname(p["manifest_path"])): n for n, p in workspace.items()}

    violations = []
    nodes = {n["id"]: n for n in (meta.get("resolve") or {}).get("nodes", [])}
    if web["id"] not in nodes:
        error("voidforge-web is missing from the resolved dependency graph")

    def check_declared(package, path):
        """Declared workspace dependencies of one reachable workspace crate."""
        for dep in package["dependencies"]:
            name = dep["name"]
            if dep.get("path"):
                name = workspace_dirs.get(os.path.realpath(dep["path"]), name)
            if name in workspace and name not in ALLOWED and name != WEB:
                kind = dep.get("kind") or "normal"
                shown = name if not dep.get("rename") else name + " (as " + dep["rename"] + ")"
                violations.append("declared " + kind + " dependency: " + " -> ".join(path + [shown]))

    # Walk the resolved graph from voidforge-web; check each reached workspace crate's declarations.
    seen = {web["id"]: [WEB]}
    queue = [web["id"]]
    while queue:
        current = queue.pop(0)
        if current in members:
            check_declared(by_id[current], seen[current])
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
