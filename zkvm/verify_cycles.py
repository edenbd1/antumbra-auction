#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Fail if CYCLES.md no longer matches what the executor actually reports.

The document is quoted from a grant proposal. A figure that drifts is not a
cosmetic problem: it is a public claim that stopped being true, and nobody
notices until a reader reproduces it. So the check is a build step rather than a
habit.

Usage:  cargo run --release | python3 verify_cycles.py
        python3 verify_cycles.py < saved-output.txt
"""
import re
import sys
import pathlib

DOC = pathlib.Path(__file__).with_name("CYCLES.md")

# Every row in the document, keyed by the operation the harness prints. An op
# measured but absent here, or a label present here but absent from the
# document, is a failure: a table that quietly loses a row is a table that
# stopped covering what it says it covers.
DOC_LABEL = {
    "multiplier_at":     "`multiplier_at`",
    "quote":             "`quote`",
    "settle":            "`settle`",
    "health":            "`health`",
    "liquidator_reward": "`liquidator_reward`",
    "accepts":           "`accepts`",
    "mul_div_floor":     "`mul_div_floor`",
    "mul_div_ceil":      "`mul_div_ceil`",
}


def main():
    run = sys.stdin.read()
    measured = {m.group(1): int(m.group(2))
                for m in re.finditer(r"^\| `([a-z_]+)` \| \d+ \| (\d+) \|", run, re.M)}
    if not measured:
        print("no measurements on stdin — did the harness run?", file=sys.stderr)
        return 2

    doc = DOC.read_text()
    fails, checked = [], 0

    for op, cycles in measured.items():
        label = DOC_LABEL.get(op)
        if label is None:
            fails.append("%s is measured but has no row in CYCLES.md" % op)
            continue
        row = re.search(r"^\| " + re.escape(label) + r" \| \d+ \| \*{0,2}([\d,]+)\*{0,2} \|",
                        doc, re.M)
        if row is None:
            fails.append("%s: no row matching '%s'" % (op, label))
            continue
        published = int(row.group(1).replace(",", ""))
        checked += 1
        if published != cycles:
            fails.append("%s: CYCLES.md says %s, the executor says %s"
                         % (op, format(published, ","), format(cycles, ",")))

    for label in DOC_LABEL.values():
        if label not in doc:
            fails.append("CYCLES.md is missing the row for %s" % label)

    whole = re.search(r"end to end: (\d+) cycles", run)
    if whole:
        want = int(whole.group(1))
        got = re.search(r"is \*\*([\d,]+)\*\* cycles end to end", doc)
        if not got or int(got.group(1).replace(",", "")) != want:
            fails.append("whole bid: CYCLES.md disagrees with the measured %s"
                         % format(want, ","))
    else:
        fails.append("the harness printed no end-to-end figure")

    if fails:
        print("CYCLES.md is out of date:", file=sys.stderr)
        for f in fails:
            print("  - " + f, file=sys.stderr)
        print("\nRegenerate it from the run rather than editing it by hand.", file=sys.stderr)
        return 1

    print("CYCLES.md matches the executor on all %d operations." % checked)
    return 0


if __name__ == "__main__":
    sys.exit(main())
