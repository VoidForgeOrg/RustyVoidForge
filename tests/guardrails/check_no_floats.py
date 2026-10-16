#!/usr/bin/env python3
"""Fail if voidforge-core, voidforge-rules or voidforge-worldgen compute with a float anywhere.

Architectural Design, Determinism rules: integers only; floats never appear in the core.
Clippy's bans (crate clippy.toml, decision log D-010) catch named float types and float
operators, but not a float that is never named, such as `1.25f64.sqrt().to_bits()`,
`let x = 2.5; x as i64`, or `Duration::as_secs_f64()`. This check is type-aware: it
compiles each crate's library to MIR, where every local, constant and call carries its
type, and rejects any f32 or f64 there. String literals and the allocation blocks that dump
constant data as bytes are skipped, so the text "f64" in a message is not a float.

Usage: check_no_floats.py [workspace-dir]
Exit codes: 0 no floats, 1 floats found, 2 could not check (build failed or no MIR produced).
"""
import glob
import os
import re
import shutil
import subprocess
import sys
import tempfile

CRATES = ["voidforge-core", "voidforge-rules", "voidforge-worldgen"]
FLOAT = re.compile(r"\bf(?:32|64)\b")
STRING = re.compile(r'"(?:[^"\\]|\\.)*"')
ITEM_HEADER = re.compile(r"^(?:(fn) (\S+?)\(|(const|static|static mut) (\S+?): )")
ALLOC_BLOCK = re.compile(r"^alloc\d+ \(")  # "allocN (size: …) {" … "}": bytes of string and other constant data


def error(message):
    print("error: " + message + "; could not check for floats", file=sys.stderr)
    sys.exit(2)


def mir_for(root, crate, out_dir):
    try:
        proc = subprocess.run(
            ["cargo", "rustc", "-p", crate, "--lib", "--", "--emit=mir", "-o", os.path.join(out_dir, "lib.mir")],
            cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE, universal_newlines=True,
        )
    except OSError as exc:
        error("cannot run cargo (" + str(exc) + ")")
    if proc.returncode != 0:
        sys.stderr.write(proc.stderr)
        error("cargo rustc failed for " + crate + " (exit " + str(proc.returncode) + ")")
    files = glob.glob(os.path.join(out_dir, "*.mir"))
    if len(files) != 1:
        error("expected one MIR file for " + crate + ", found " + str(len(files)))
    with open(files[0]) as f:
        return f.read()


def main():
    root = sys.argv[1] if len(sys.argv) > 1 else "."
    if not os.path.isfile(os.path.join(root, "Cargo.toml")):
        error("no Cargo.toml in " + root)
    violations = []
    for crate in CRATES:
        out_dir = tempfile.mkdtemp(prefix="voidforge-mir-")
        try:
            mir = mir_for(root, crate, out_dir)
        finally:
            shutil.rmtree(out_dir, ignore_errors=True)
        if not mir.strip():
            error("empty MIR for " + crate)
        current = "item (unnamed)"
        found = {}
        in_alloc = False
        for line in mir.splitlines():
            if in_alloc:
                # An allocation block ends at its "}". Stop early at the next item or allocation
                # header too, so an unexpected format can never hide the rest of the MIR.
                if line == "}":
                    in_alloc = False
                    continue
                if not (ITEM_HEADER.match(line) or ALLOC_BLOCK.match(line)):
                    continue
                in_alloc = False
            if ALLOC_BLOCK.match(line):
                in_alloc = not line.rstrip().endswith("}")
                continue
            header = ITEM_HEADER.match(line)
            if header:
                kind, name = (header.group(1), header.group(2)) if header.group(1) else (header.group(3), header.group(4))
                current = kind + " " + name
            if FLOAT.search(STRING.sub('""', line)):
                found.setdefault(current, line.strip())
        for fn, line in sorted(found.items()):  # fn is "<kind> <name>", e.g. "fn halves"
            violations.append(crate + ": " + fn + ": " + line)
    if violations:
        for v in violations:
            print("violation: " + v)
        print("error: floats in " + ", ".join(CRATES) + " (Architectural Design, Determinism rules: integers only)",
              file=sys.stderr)
        return 1
    print("ok: no f32 or f64 in the MIR of " + ", ".join(CRATES))
    return 0


if __name__ == "__main__":
    sys.exit(main())
