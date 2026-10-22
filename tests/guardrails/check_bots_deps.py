#!/usr/bin/env python3
"""Fail if a crate crosses the Technical Plan's layering among workspace crates.

Technical Plan, Project layering, checked mechanically (D-055, review-r1's R1-G8):
  isolated   voidforge-bots depends on no workspace crate but voidforge-api-types and
             voidforge-telemetry, through any dependency kind, transitively: the bots reach the
             server over HTTP alone.
  direct     voidforge-server never depends on voidforge-core itself, as a normal dependency: it
             consumes the host's re-exports (D-047). Its tests may (api_codes checks the codes).

For an isolated crate, two passes, over every dependency kind (normal, dev, build):
  resolved  the graph cargo resolves from it, walked transitively;
  declared  the manifest declarations of it and of every workspace crate it reaches,
            by name, by path or renamed. This catches declarations cargo drops from the graph,
            such as a path dependency on a binary-only crate. For those it enforces the
            declaration; cargo links no library from a binary-only crate.

Usage: check_bots_deps.py [workspace-dir]
Exit codes: 0 every rule holds, 1 violation found, 2 could not check (cargo or metadata error).
"""
import json
import os
import subprocess
import sys

ALLOWED = {"voidforge-api-types", "voidforge-telemetry"}
ISOLATED = ["voidforge-bots"]
# (crate, banned workspace crate, dependency kinds): never declared or resolved directly.
DIRECT = [("voidforge-server", "voidforge-core", {"normal"})]


def error(message):
    print("error: " + message + "; could not check the workspace's dependencies", file=sys.stderr)
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
    for crate in ISOLATED + [c for c, _, _ in DIRECT]:
        if crate not in workspace:
            error(crate + " is not a workspace member")
    workspace_dirs = {os.path.realpath(os.path.dirname(p["manifest_path"])): n for n, p in workspace.items()}
    nodes = {n["id"]: n for n in (meta.get("resolve") or {}).get("nodes", [])}

    def declared_name(dep):
        """A declaration's workspace crate: by name, by path, or under another name."""
        name = dep["name"]
        if dep.get("path"):
            name = workspace_dirs.get(os.path.realpath(dep["path"]), name)
        return name

    def shown(dep, name):
        return name if not dep.get("rename") else name + " (as " + dep["rename"] + ")"

    failed = []
    violations = []
    for crate in ISOLATED:
        found = isolation(crate, workspace, by_id, members, nodes, declared_name, shown)
        if found:
            violations += found
            failed.append(crate + " may depend only on " + ", ".join(sorted(ALLOWED))
                          + " among workspace crates")
    for crate, banned, kinds in DIRECT:
        found = []
        package = workspace[crate]
        for dep in package["dependencies"]:
            kind = dep.get("kind") or "normal"
            if declared_name(dep) == banned and kind in kinds:
                found.append("declared " + kind + " dependency: " + crate + " -> " + shown(dep, banned))
        if package["id"] in nodes:
            for dep in nodes[package["id"]]["deps"]:
                hit = {k.get("kind") or "normal" for k in dep["dep_kinds"]} & kinds
                if by_id[dep["pkg"]]["name"] == banned and hit:
                    found.append("resolved " + ",".join(sorted(hit)) + " path: " + crate + " -> " + banned)
        if found:
            violations += found
            failed.append(crate + " must not depend on " + banned + " directly ("
                          + ", ".join(sorted(kinds)) + ")")

    if violations:
        for v in sorted(set(violations)):
            print("violation: " + v)
        for f in failed:
            print("error: " + f + " (Technical Plan, Project layering)", file=sys.stderr)
        return 1
    print("ok: " + " and ".join(ISOLATED) + " depends on no workspace crate besides "
          + ", ".join(sorted(ALLOWED)) + "; "
          + "; ".join(c + " never depends on " + b + " directly" for c, b, _ in DIRECT))
    return 0


def isolation(crate, workspace, by_id, members, nodes, declared_name, shown):
    """An isolated crate's violations: the resolved graph walked from it, and the declarations of
    every workspace crate it reaches."""
    root = workspace[crate]
    if root["id"] not in nodes:
        error(crate + " is missing from the resolved dependency graph")
    violations = []

    def check_declared(package, path):
        for dep in package["dependencies"]:
            name = declared_name(dep)
            if name in workspace and name not in ALLOWED and name != crate:
                kind = dep.get("kind") or "normal"
                violations.append("declared " + kind + " dependency: " + " -> ".join(path + [shown(dep, name)]))

    seen = {root["id"]: [crate]}
    queue = [root["id"]]
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
    return violations


if __name__ == "__main__":
    sys.exit(main())
