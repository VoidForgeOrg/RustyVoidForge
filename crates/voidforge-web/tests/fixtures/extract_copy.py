#!/usr/bin/env python3
"""Extracts phase4-ux.md's §6.1 and §6.2 copy into copy.json, for tests/copy.rs (P4.2, D-059).

dev-design owns the copy; this keeps the test's expected strings the document's own rather than
restated by hand. Rerun it whenever §6.1 or §6.2 changes, then run the web tests.

Usage: extract_copy.py <phase4-ux.md> <copy.json>
"""
import json
import re
import sys


def main():
    src, out = sys.argv[1:]
    text = open(src, encoding="utf-8").read()
    section = text[text.index("### 6.1"):text.index("### 6.3")]
    codes = {}
    for line in section.splitlines():
        m = re.match(r"^\| `([a-z_]+)` \| (.+?) \|(?: (.+?) \|)?$", line)
        if not m:
            continue
        code, first, second = m.groups()
        # A trailing note for readers of the document, citing a section, isn't copy.
        first = re.sub(r" \([^()]*§[^()]*\)$", "", first)
        if second is None:
            codes[code] = {"page": first, "command": first}
        else:
            # 429, 500, 503: the order's copy, then the page's; the leaderboard's own line is P4.3's.
            second = re.sub(r" \(On the leaderboard: .*\)$", "", second)
            codes[code] = {"command": first, "page": second}
    without_number = {m.group(1): m.group(2)
                      for m in re.finditer(r"^- `([a-z_]+)`: `([^`]+)`", section, re.M)}
    row = next(l for l in section.splitlines() if l.startswith("| The API is unreachable"))
    m = re.search(r"Command: `([^`]+)` Page: `([^`]+)`", row)
    web = {"unreachable": {"command": m.group(1), "page": m.group(2)}}
    row = next(l for l in section.splitlines() if l.startswith("| An unexpected status"))
    web["unexpected"] = {"command": re.search(r"Command: `([^`]+)`", row).group(1),
                         "page": re.search(r"Page: `([^`]+)`", row).group(1)}
    # Copy never cites the document: a note for its readers left in a cell would ship (R2's note).
    strings = [v for pair in list(codes.values()) + list(web.values()) for v in pair.values()]
    strings += list(without_number.values())
    noted = [v for v in strings if "§" in v or "(Only" in v]
    if noted:
        sys.exit("a document note is in the copy, which would ship: %r" % noted[0][:100])
    if len(codes) != 44 or len(without_number) != 5:
        sys.exit("expected 44 codes and 5 forms without a number, found %d and %d"
                 % (len(codes), len(without_number)))
    doc = {"source": "phase4-ux.md §6.1 and §6.2 (dev-design), extracted mechanically for P4.2's table test",
           "codes": codes, "without_number": without_number, "web": web}
    with open(out, "w", encoding="utf-8") as f:
        json.dump(doc, f, indent=1, ensure_ascii=False)
        f.write("\n")


if __name__ == "__main__":
    main()
