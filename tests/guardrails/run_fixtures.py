#!/usr/bin/env python3
"""Prove the P0.4 guardrails by effect: every banned item and forbidden dependency is rejected
with its intended diagnostic, and clean code is accepted.

The fixtures run against a temporary copy of this workspace, so the real crates, their
clippy.toml files and lint attributes are what gets tested. For each case:

  determinism/<category>.rs     must be rejected in core, rules and worldgen with exactly the
                                diagnostics in its "expect:" lines, nothing else, and must pass
                                unchanged in voidforge-host (the bans are scoped);
  determinism/<category>.ok.rs  the allowed alternative; must pass in all three crates;
  meta/*.rs                     traps for this harness itself: a guardrail that does not fire
                                must be reported MISSING, and an unrelated compile or
                                dependency error must be reported ERROR, never as a rejection;
  float_values/<name>.rs        floats clippy cannot see; check_no_floats.py must reject them in
                                core, rules and worldgen, naming exactly the "expect-float-fn:"
                                functions; <name>.ok.rs must pass; <name>.err.rs does not
                                compile and must make the check exit 2, never pass;
  WEB_CASES below               manifest edits checked with check_web_deps.py.

Before running cases it checks that the three clippy.toml files are identical and that every
banned path in them is expected by at least one fixture.

Fixture header lines:
  // expect: <lint code> [<path>]   a diagnostic the case must produce; <path> must appear in
                                    backticks in its message
  // dependency: <toml line>        added under [dependencies] of the crate under test
  // classify: MISSING|ERROR        meta fixtures only: the outcome this harness must report
  // expect-float-fn: <name>        float_values fixtures: a function (or const/static) the float
                                    check must report

Usage: run_fixtures.py [--only SUBSTRING] [--verbose]   (run from the repository root)
Exit codes: 0 every case behaved as intended, 1 a case did not, 2 the harness could not run.
"""
import argparse
import filecmp
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile

TARGETS = ["voidforge-core", "voidforge-rules", "voidforge-worldgen"]
SHELL = "voidforge-host"
HERE = os.path.dirname(os.path.abspath(__file__))
FIXTURE_MODULE = "guardrail_fixture"
IGNORED_MESSAGE = re.compile(r"^(aborting due to|\d+ warnings? emitted|.*previous errors?.*emitted)")


class HarnessError(Exception):
    pass


def run(cmd, cwd, env=None):
    try:
        return subprocess.run(cmd, cwd=cwd, env=env, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
                              universal_newlines=True)
    except OSError as exc:
        raise HarnessError("cannot run " + cmd[0] + ": " + str(exc))


def float_fns(path):
    with open(path) as f:
        return sorted(m.group(1) for m in (re.match(r"//\s*expect-float-fn:\s*(\S+)\s*$", l) for l in f) if m)


def parse_fixture(path):
    expects, deps, classify = [], [], None
    with open(path) as f:
        for line in f:
            m = re.match(r"//\s*expect:\s*(\S+)(?:\s+(\S+))?\s*$", line)
            if m:
                expects.append((m.group(1), m.group(2)))
            m = re.match(r"//\s*dependency:\s*(.+?)\s*$", line)
            if m:
                deps.append(m.group(1))
            m = re.match(r"//\s*classify:\s*(MISSING|ERROR)\s*$", line)
            if m:
                classify = m.group(1)
    return expects, deps, classify


def banned_paths(clippy_toml):
    with open(clippy_toml) as f:
        return re.findall(r'path\s*=\s*"([^"]+)"', f.read())


class Workspace:
    """A scratch copy of the repository plus a pristine copy to restore from between cases."""

    def __init__(self, repo):
        self.base = tempfile.mkdtemp(prefix="voidforge-guardrails-")
        self.pristine = os.path.join(self.base, "pristine")
        self.root = os.path.join(self.base, "work")
        listed = run(["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"], repo)
        if listed.returncode != 0:
            raise HarnessError("git ls-files failed: " + listed.stderr.strip())
        for rel in filter(None, listed.stdout.split("\0")):
            src = os.path.join(repo, rel)
            if not os.path.isfile(src):
                continue
            for top in (self.pristine, self.root):
                dst = os.path.join(top, rel)
                os.makedirs(os.path.dirname(dst), exist_ok=True)
                shutil.copy2(src, dst)
        self.env = dict(os.environ, CARGO_TARGET_DIR=os.path.join(self.base, "target"))
        self.touched = set()

    def path(self, rel):
        return os.path.join(self.root, rel)

    def write(self, rel, text, append=False):
        self.touched.add(rel)
        with open(self.path(rel), "a" if append else "w") as f:
            f.write(text)

    def add_dependencies(self, crate, section, lines):
        """Add lines to a manifest table, inserting under the table if the crate already has it."""
        if not lines:
            return
        rel = "crates/" + crate + "/Cargo.toml"
        with open(self.path(rel)) as f:
            manifest = f.read()
        header = "[" + section + "]\n"
        # A crate that already declares a dependency in this section (for example
        # voidforge-rules and toml) has what the fixture needs; adding it again would be a
        # duplicate key, not a test. Only this section's keys count: a dev-dependency or a
        # [package] key of the same name does not satisfy a fixture's normal dependency.
        body = ""
        if manifest.startswith(header) or ("\n" + header) in manifest:
            start = manifest.index(header) + len(header)
            nxt = re.search(r"(?m)^\[", manifest[start:])
            body = manifest[start:start + nxt.start()] if nxt else manifest[start:]
        declared = set(re.findall(r"(?m)^([A-Za-z0-9_-]+)(?:\.workspace)?\s*=", body))
        lines = [l for l in lines if re.match(r"\s*([A-Za-z0-9_-]+)", l).group(1) not in declared]
        if not lines:
            return
        if manifest.startswith(header) or ("\n" + header) in manifest:
            at = manifest.index(header) + len(header)
            manifest = manifest[:at] + "\n".join(lines) + "\n" + manifest[at:]
        else:
            manifest += "\n" + header + "\n".join(lines) + "\n"
        self.write(rel, manifest)
        self.touched.add("Cargo.lock")

    def reset(self):
        for rel in self.touched:
            src, dst = os.path.join(self.pristine, rel), self.path(rel)
            if os.path.exists(src):
                shutil.copy2(src, dst)
            elif os.path.exists(dst):
                os.remove(dst)
        self.touched = set()

    def cleanup(self):
        shutil.rmtree(self.base, ignore_errors=True)


def clippy(ws, crate, locked):
    cmd = ["cargo", "clippy", "-p", crate, "--all-targets", "--message-format=json"]
    if locked:
        cmd.append("--locked")
    proc = run(cmd + ["--", "-D", "warnings"], ws.root, ws.env)
    diags = []
    for line in proc.stdout.splitlines():
        try:
            msg = json.loads(line)
        except ValueError:
            continue
        if msg.get("reason") != "compiler-message":
            continue
        d = msg["message"]
        if not d["spans"] and (d["level"] == "failure-note" or IGNORED_MESSAGE.match(d["message"])):
            continue  # rustc's summary lines and "rustc --explain" footnotes, not diagnostics
        primary = [s for s in d["spans"] if s.get("is_primary")] or d["spans"]
        diags.append({
            "level": d["level"],
            "code": (d.get("code") or {}).get("code"),
            "message": d["message"],
            "file": primary[0]["file_name"] if primary else None,
            "line": primary[0]["line_start"] if primary else None,
        })
    return proc.returncode, diags, proc.stderr


def describe(d):
    where = "%s:%s" % (d["file"], d["line"]) if d["file"] else "(no span)"
    return "%s %s %s: %s" % (d["level"], d["code"] or "-", where, d["message"])


def classify_rejection(code, diags, expects):
    """PASS if every expected diagnostic appears and nothing else does; MISSING if a guardrail did
    not fire; ERROR if anything unrelated happened (other diagnostics, cargo failures)."""
    def matches(d, exp):
        lint, path = exp
        return d["code"] == lint and (path is None or "`" + path + "`" in d["message"]) \
            and (d["file"] or "").endswith(FIXTURE_MODULE + ".rs")
    unexpected = [d for d in diags if not any(matches(d, e) for e in expects)]
    missing = [e for e in expects if not any(matches(d, e) for d in diags)]
    if unexpected or (code != 0 and not diags):
        return "ERROR", unexpected, missing
    if missing or code == 0:
        return "MISSING", unexpected, missing
    return "PASS", unexpected, missing


class Report:
    def __init__(self, verbose):
        self.verbose = verbose
        self.failures = 0
        self.count = 0

    def case(self, name, ok, details=()):
        self.count += 1
        if ok:
            print("pass: " + name)
        else:
            self.failures += 1
            print("FAIL: " + name)
        if details and (not ok or self.verbose):
            for line in details:
                print("    " + line)


def inject(ws, crate, fixture, deps):
    src = "crates/" + crate + "/src/"
    with open(fixture) as f:
        ws.write(src + FIXTURE_MODULE + ".rs", f.read())
    lib = src + ("lib.rs" if os.path.exists(ws.path(src + "lib.rs")) else "main.rs")
    ws.write(lib, "\npub mod " + FIXTURE_MODULE + ";\n", append=True)
    ws.add_dependencies(crate, "dependencies", deps)


def run_determinism(ws, report, only):
    folder = os.path.join(HERE, "determinism")
    negatives = sorted(f for f in os.listdir(folder) if f.endswith(".rs") and not f.endswith(".ok.rs"))
    controls = sorted(f for f in os.listdir(folder) if f.endswith(".ok.rs"))

    for crate in TARGETS + [SHELL]:
        name = "baseline: %s is clean" % crate
        if only and only not in name:
            continue
        code, diags, stderr = clippy(ws, crate, locked=True)
        report.case(name, code == 0 and not diags, [describe(d) for d in diags] + stderr.splitlines()[-3:])

    for fname in negatives:
        expects, deps, _ = parse_fixture(os.path.join(folder, fname))
        if not expects:
            raise HarnessError(fname + " has no expect: lines")
        for crate in TARGETS:
            name = "reject: %s in %s" % (fname, crate)
            if only and only not in name:
                continue
            inject(ws, crate, os.path.join(folder, fname), deps)
            code, diags, stderr = clippy(ws, crate, locked=not deps)
            ws.reset()
            outcome, unexpected, missing = classify_rejection(code, diags, expects)
            details = ["outcome %s, clippy exit %d" % (outcome, code)]
            details += ["got " + describe(d) for d in diags]
            details += ["missing %s %s" % m for m in missing]
            details += ["unexpected " + describe(d) for d in unexpected]
            if outcome == "ERROR" and not diags:
                details += stderr.splitlines()[-5:]
            report.case(name, outcome == "PASS", details)
        name = "scoped: %s passes in %s" % (fname, SHELL)
        if fname.startswith("exception"):
            continue  # an #[allow] in a crate with no forbid is not a violation worth asserting
        if only and only not in name:
            continue
        inject(ws, SHELL, os.path.join(folder, fname), deps)
        code, diags, stderr = clippy(ws, SHELL, locked=not deps)
        ws.reset()
        report.case(name, code == 0 and not diags, [describe(d) for d in diags] + stderr.splitlines()[-3:])

    for fname in controls:
        _, deps, _ = parse_fixture(os.path.join(folder, fname))
        for crate in TARGETS:
            name = "accept: %s in %s" % (fname, crate)
            if only and only not in name:
                continue
            inject(ws, crate, os.path.join(folder, fname), deps)
            code, diags, stderr = clippy(ws, crate, locked=not deps)
            ws.reset()
            report.case(name, code == 0 and not diags, [describe(d) for d in diags] + stderr.splitlines()[-3:])


def run_float_values(ws, report, only):
    folder = os.path.join(HERE, "float_values")
    checker = os.path.join(HERE, "check_no_floats.py")
    for fname in sorted(f for f in os.listdir(folder) if f.endswith(".rs")):
        path = os.path.join(folder, fname)
        _, deps, _ = parse_fixture(path)
        want_fns = float_fns(path)
        kind = "err" if fname.endswith(".err.rs") else "ok" if fname.endswith(".ok.rs") else "reject"
        if kind == "reject" and not want_fns:
            raise HarnessError(fname + " has no expect-float-fn: lines")
        for crate in TARGETS:
            name = {"reject": "float check rejects %s in %s", "ok": "float check accepts %s in %s",
                    "err": "float check exits 2 (not a pass) for %s in %s"}[kind] % (fname, crate)
            if only and only not in name:
                continue
            inject(ws, crate, path, deps)
            proc = run([sys.executable, checker, ws.root], ws.root, ws.env)
            ws.reset()
            lines = [l for l in proc.stdout.splitlines() if l.startswith("violation: ")]
            got_fns = sorted(re.match(r"violation: \S+: (?:fn|const|static|static mut|item) (\S+): ", l).group(1)
                             if re.match(r"violation: \S+: (?:fn|const|static|static mut|item) (\S+): ", l) else l
                             for l in lines)
            got_crates = {l.split()[1].rstrip(":") for l in lines}
            if kind == "reject":
                ok = proc.returncode == 1 and got_fns == want_fns and got_crates == {crate}
            elif kind == "ok":
                ok = proc.returncode == 0 and not lines
            else:
                ok = proc.returncode == 2 and "could not check" in proc.stderr and not lines
            report.case(name, ok, ["exit %d" % proc.returncode, "want fns %s, got %s" % (want_fns, got_fns)]
                        + lines + proc.stderr.splitlines()[-3:])


def run_meta(ws, report, only):
    folder = os.path.join(HERE, "meta")
    for fname in sorted(f for f in os.listdir(folder) if f.endswith(".rs")):
        expects, deps, classify = parse_fixture(os.path.join(folder, fname))
        if classify is None:
            raise HarnessError("meta fixture " + fname + " has no classify: line")
        name = "harness reports %s for %s" % (classify, fname)
        if only and only not in name:
            continue
        crate = TARGETS[0]
        inject(ws, crate, os.path.join(folder, fname), deps)
        code, diags, stderr = clippy(ws, crate, locked=not deps)
        ws.reset()
        outcome, _, _ = classify_rejection(code, diags, expects)
        report.case(name, outcome == classify,
                    ["classified %s, clippy exit %d" % (outcome, code)] + [describe(d) for d in diags])


# Web dependency cases: (name, edits, expected exit, expected violation lines or stderr fragments).
# An edit is (crate, section, line) appended to that crate's Cargo.toml.
WEB_CASES = [
    ("web as committed (telemetry only, and telemetry's external crates)", [], 0, []),
    ("web adds api-types next to telemetry (allowed)",
     [("voidforge-web", "dependencies", "voidforge-api-types = { workspace = true }")], 0, []),
    ("web on core", [("voidforge-web", "dependencies", "voidforge-core = { workspace = true }")], 1,
     ["violation: declared normal dependency: voidforge-web -> voidforge-core",
      "violation: resolved normal path: voidforge-web -> voidforge-core"]),
    ("web on host by path", [("voidforge-web", "dependencies", 'voidforge-host = { path = "../voidforge-host" }')], 1,
     ["violation: declared normal dependency: voidforge-web -> voidforge-host",
      "violation: resolved normal path: voidforge-web -> voidforge-host"]),
    ("web on store as a dev-dependency",
     [("voidforge-web", "dev-dependencies", "voidforge-store = { workspace = true }")], 1,
     ["violation: declared dev dependency: voidforge-web -> voidforge-store",
      "violation: resolved dev path: voidforge-web -> voidforge-store"]),
    ("web on core as a build-dependency",
     [("voidforge-web", "build-dependencies", "voidforge-core = { workspace = true }")], 1,
     ["violation: declared build dependency: voidforge-web -> voidforge-core",
      "violation: resolved build path: voidforge-web -> voidforge-core"]),
    ("web on core under another name",
     [("voidforge-web", "dependencies", 'engine = { package = "voidforge-core", path = "../voidforge-core" }')], 1,
     ["violation: declared normal dependency: voidforge-web -> voidforge-core (as engine)",
      "violation: resolved normal path: voidforge-web -> voidforge-core"]),
    ("web on server by path (cargo ignores it: no library target)",
     [("voidforge-web", "dependencies", 'voidforge-server = { path = "../voidforge-server" }')], 1,
     ["violation: declared normal dependency: voidforge-web -> voidforge-server"]),
    ("web on core through api-types",
     [("voidforge-api-types", "dependencies", "voidforge-core = { workspace = true }"),
      ("voidforge-web", "dependencies", "voidforge-api-types = { workspace = true }")], 1,
     ["violation: declared normal dependency: voidforge-web -> voidforge-api-types -> voidforge-core",
      "violation: resolved normal path: voidforge-web -> voidforge-api-types -> voidforge-core"]),
    ("web on store through telemetry and host",
     [("voidforge-host", "dependencies", "voidforge-store = { workspace = true }"),
      ("voidforge-telemetry", "dependencies", "voidforge-host = { workspace = true }")], 1,
     ["violation: declared normal dependency: voidforge-web -> voidforge-telemetry -> voidforge-host",
      "violation: declared normal dependency: voidforge-web -> voidforge-telemetry -> voidforge-host -> voidforge-store",
      "violation: resolved normal path: voidforge-web -> voidforge-telemetry -> voidforge-host",
      "violation: resolved normal path: voidforge-web -> voidforge-telemetry -> voidforge-host -> voidforge-store"]),
    # Since P0.6 every binary depends on telemetry, so telemetry -> server would be a cycle;
    # api-types is the reachable crate that declares the binary here.
    ("web on server through api-types by path (a declaration cargo drops, in a reachable crate)",
     [("voidforge-api-types", "dependencies", 'voidforge-server = { path = "../voidforge-server" }'),
      ("voidforge-web", "dependencies", "voidforge-api-types = { workspace = true }")], 1,
     ["violation: declared normal dependency: voidforge-web -> voidforge-api-types -> voidforge-server"]),
    ("reachable crate declaring an allowed crate (telemetry on api-types)",
     [("voidforge-telemetry", "dependencies", "voidforge-api-types = { workspace = true }")], 0, []),
    ("unreachable crate may declare anything (server on core; web does not depend on server)",
     [("voidforge-server", "dependencies", "voidforge-core = { workspace = true }")], 0, []),
    ("web on another workspace crate (rules)",
     [("voidforge-web", "dependencies", "voidforge-rules = { workspace = true }")], 1,
     # Since P1.2 rules depends on core, so web reaches core through it as well.
     ["violation: declared normal dependency: voidforge-web -> voidforge-rules",
      "violation: declared normal dependency: voidforge-web -> voidforge-rules -> voidforge-core",
      "violation: resolved normal path: voidforge-web -> voidforge-rules",
      "violation: resolved normal path: voidforge-web -> voidforge-rules -> voidforge-core"]),
    ("cargo itself rejects web on server through [workspace.dependencies]",
     [("voidforge-web", "dependencies", "voidforge-server = { workspace = true }")], 2,
     ["stderr:voidforge-server", "stderr:workspace.dependencies"]),
    ("checker error: broken manifest is exit 2, not a pass",
     [("voidforge-web", "dependencies", "this is not toml")], 2, ["stderr:could not check"]),
]


def run_web(ws, report, only):
    checker = os.path.join(HERE, "check_web_deps.py")
    for name, edits, want_code, want in WEB_CASES:
        name = "web deps: " + name
        if only and only not in name:
            continue
        grouped = {}
        for crate, section, line in edits:
            grouped.setdefault((crate, section), []).append(line)
        for (crate, section), lines in grouped.items():
            ws.add_dependencies(crate, section, lines)
        proc = run([sys.executable, checker, ws.root], ws.root, ws.env)
        ws.reset()
        got = sorted(l for l in proc.stdout.splitlines() if l.startswith("violation:"))
        want_lines = sorted(w for w in want if not w.startswith("stderr:"))
        want_err = [w[len("stderr:"):] for w in want if w.startswith("stderr:")]
        ok = proc.returncode == want_code and got == want_lines and all(e in proc.stderr for e in want_err)
        report.case(name, ok, ["exit %d (want %d)" % (proc.returncode, want_code)]
                    + ["got " + g for g in got] + ["want " + w for w in want_lines]
                    + ["stderr " + l for l in proc.stderr.splitlines()[-4:]])

    name = "web deps: checker error: a directory that is not a workspace is exit 2, not a pass"
    if not only or only in name:
        empty = tempfile.mkdtemp(prefix="voidforge-guardrails-empty-")
        proc = run([sys.executable, checker, empty], empty)
        shutil.rmtree(empty, ignore_errors=True)
        report.case(name, proc.returncode == 2 and "could not check" in proc.stderr,
                    ["exit %d" % proc.returncode] + proc.stderr.splitlines()[-3:])


def check_structure(repo, report):
    tomls = [os.path.join(repo, "crates", c, "clippy.toml") for c in TARGETS]
    for t in tomls:
        if not os.path.isfile(t):
            raise HarnessError("missing " + t)
    same = all(filecmp.cmp(tomls[0], t, shallow=False) for t in tomls[1:])
    report.case("clippy.toml is identical in " + ", ".join(TARGETS), same)

    stray = []
    for top, dirs, files in os.walk(repo):
        dirs[:] = [d for d in dirs if d not in (".git", "target")]
        for f in files:
            if f in ("clippy.toml", ".clippy.toml"):
                rel = os.path.relpath(os.path.join(top, f), repo)
                if rel not in ["crates/%s/clippy.toml" % c for c in TARGETS]:
                    stray.append(rel)
    report.case("no clippy.toml outside the three target crates (decision log D-010)", not stray,
                ["found " + s for s in stray])

    expected = set()
    for sub in ("determinism",):
        folder = os.path.join(HERE, sub)
        for f in os.listdir(folder):
            if f.endswith(".rs") and not f.endswith(".ok.rs"):
                expected.update(p for _, p in parse_fixture(os.path.join(folder, f))[0] if p)
    uncovered = [p for p in banned_paths(tomls[0]) if p not in expected]
    report.case("every banned path is expected by a fixture", not uncovered,
                ["no fixture expects " + p for p in uncovered])


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    parser.add_argument("--only", help="run only cases whose name contains this text")
    parser.add_argument("--verbose", action="store_true", help="print details for passing cases too")
    args = parser.parse_args()

    repo = os.getcwd()
    if not os.path.isfile(os.path.join(repo, "Cargo.toml")) or not os.path.isdir(os.path.join(repo, "crates")):
        print("error: run from the repository root", file=sys.stderr)
        return 2
    report = Report(args.verbose)
    ws = None
    try:
        version = run(["cargo", "clippy", "--version"], repo)
        if version.returncode != 0:
            raise HarnessError("cargo clippy is not available: " + version.stderr.strip())
        print("using " + version.stdout.strip())
        if not args.only:
            check_structure(repo, report)
        ws = Workspace(repo)
        run_determinism(ws, report, args.only)
        run_float_values(ws, report, args.only)
        run_meta(ws, report, args.only)
        run_web(ws, report, args.only)
    except HarnessError as exc:
        print("error: " + str(exc) + "; the guardrail fixtures could not run", file=sys.stderr)
        return 2
    except Exception:  # a bug in this harness must never look like a guardrail result
        import traceback
        traceback.print_exc()
        print("error: the guardrail harness crashed; the guardrail fixtures could not run", file=sys.stderr)
        return 2
    finally:
        if ws:
            ws.cleanup()
    if report.count == 0:
        print("error: no case matched", file=sys.stderr)
        return 2
    if report.failures:
        print("error: %d of %d guardrail cases did not behave as intended" % (report.failures, report.count),
              file=sys.stderr)
        return 1
    print("ok: all %d guardrail cases behaved as intended" % report.count)
    return 0


if __name__ == "__main__":
    sys.exit(main())
