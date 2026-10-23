#!/usr/bin/env python3
"""The nightly workflow's static rules (Technical Plan P5.3; decision log D-066), run by CI's lint
job. A finding is an error with the rule it breaks.

- The workflow reads only: `permissions: contents: read` at the top.
- Only `report` may write, and only issues: `contents: read, issues: write`.
- `report` never runs for a pull request: its `if` excludes the event.
- A forced failure happens only on a manual dispatch: every step that reads `force_fail` is
  guarded by `github.event_name == 'workflow_dispatch'`.
- Every job has a timeout, and every action is pinned to a full commit SHA.
- Every run that reports shares one concurrency group, whatever its ref (R2-4): the group is a
  pull request's own, or else exactly 'nightly'.

Usage: check-nightly.py [workflow]   (default .github/workflows/nightly.yml)
       check-nightly.py --self-test  (each rule refuses a planted break)
Exit codes: 0 clean; 1 findings; 2 could not check.
"""

import copy
import re
import sys

import yaml

DEFAULT = ".github/workflows/nightly.yml"
PINNED = re.compile(r"^[^@\s]+@[0-9a-f]{40}$")
NOT_PR = "github.event_name != 'pull_request'"
DISPATCH = "github.event_name == 'workflow_dispatch'"


def findings(doc):
    out = []
    if doc.get("permissions") != {"contents": "read"}:
        out.append("the workflow's permissions must be exactly contents: read")
    group = str((doc.get("concurrency") or {}).get("group", ""))
    if not group.endswith("|| 'nightly' }}") or "github.event_name == 'pull_request' &&" not in group:
        out.append("the concurrency group must be 'nightly' for every run but a pull request's")
    jobs = doc.get("jobs") or {}
    if "report" not in jobs:
        out.append("no report job")
    for name, job in jobs.items():
        perms = job.get("permissions")
        if name == "report":
            if perms != {"contents": "read", "issues": "write"}:
                out.append("report's permissions must be exactly contents: read, issues: write")
            if NOT_PR not in str(job.get("if", "")):
                out.append(f"report must never run for a pull request (its if needs {NOT_PR})")
        elif perms is not None:
            out.append(f"job {name} sets permissions: only report may")
        if "timeout-minutes" not in job:
            out.append(f"job {name} has no timeout-minutes")
        for step in job.get("steps") or []:
            uses = step.get("uses")
            if uses and not PINNED.match(uses.split(" ")[0]):
                out.append(f"job {name}: {uses} isn't pinned to a commit SHA")
            if "force_fail" in str(step.get("if", "")) + str(step.get("run", "")) \
                    and DISPATCH not in str(step.get("if", "")):
                out.append(f"job {name}: step {step.get('name')!r} reads force_fail outside a "
                           f"manual dispatch")
    return out


def self_test(doc):
    """Each rule must refuse a planted break, and the real workflow must pass."""
    def report(d):
        return d["jobs"]["report"]

    def first_job(d):
        return next(j for n, j in d["jobs"].items() if n != "report")

    def forced_step(d):
        return next(s for j in d["jobs"].values() for s in j.get("steps", [])
                    if "force_fail" in str(s.get("if", "")))

    breaks = {
        "workflow-level write": lambda d: d.__setitem__("permissions", {"contents": "read", "issues": "write"}),
        "report writes contents": lambda d: report(d)["permissions"].__setitem__("contents", "write"),
        "another job writes issues": lambda d: first_job(d).__setitem__("permissions", {"issues": "write"}),
        "report on a pull request": lambda d: report(d).__setitem__("if", "always()"),
        "no timeout": lambda d: first_job(d).pop("timeout-minutes"),
        "an unpinned action": lambda d: first_job(d)["steps"][0].__setitem__("uses", "actions/checkout@v7"),
        "forced outside a dispatch": lambda d: forced_step(d).__setitem__("if", "inputs.force_fail == 'crash'"),
        "a per-ref group": lambda d: d["concurrency"].__setitem__("group", "nightly-${{ github.ref }}"),
    }
    failed = 0
    if findings(doc):
        print(f"FAIL: the workflow itself has findings: {findings(doc)}")
        failed += 1
    for name, plant in breaks.items():
        broken = copy.deepcopy(doc)
        plant(broken)
        if findings(broken):
            print(f"pass: refuses {name}")
        else:
            print(f"FAIL: {name} passed the check")
            failed += 1
    return failed


def main(argv):
    test = "--self-test" in argv[1:]
    paths = [a for a in argv[1:] if a != "--self-test"]
    path = paths[0] if paths else DEFAULT
    try:
        with open(path, encoding="utf-8") as f:
            doc = yaml.safe_load(f)
    except (OSError, yaml.YAMLError) as e:
        print(f"error: can't read {path}: {e}", file=sys.stderr)
        return 2
    if test:
        return 1 if self_test(doc) else 0
    found = findings(doc)
    for f in found:
        print(f"error: {path}: {f}")
    if not found:
        print(f"ok: {path} keeps the nightly's rules")
    return 1 if found else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
