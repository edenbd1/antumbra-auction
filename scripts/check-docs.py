#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Check that the documentation still points at things that exist.

    ./scripts/check-docs.py            # report, and exit non-zero on any failure
    ./scripts/check-docs.py --list     # also print what passed

Three classes of rot, each decidable from the tree alone — no allowlist, no
judgement about what a sentence meant:

  1. A dangling repository path. A path in backticks whose first segment is a
     real top-level directory here, pointing at something that is not there.
     The first-segment rule is what makes this usable rather than noisy: it
     leaves `logos-co/logos-package` and `@rpath/QtCore.framework/` alone, which
     are an organisation slug and a linker path, not claims about this tree.

  2. A markdown link whose target does not exist. Resolved relative to the
     document's own directory, the way a reader's browser resolves it — not
     through a repo-root fallback, which would answer yes too often. This is a
     separate class from 1 on purpose: a path in backticks is a claim, and a
     link is something somebody clicks, and the half that resolves is not always
     the half they click.

  3. A line citation past the end of its file. `foo.rs:663-691` in a file 645
     lines long is wrong without anyone needing to know what line 663 was
     supposed to say. Line numbers drift under a refactor and a citation nobody
     re-checks drifts with them.

WHY THE COUNTS ARE PRINTED, AND FLOORED IN CI

Every class here can pass by matching nothing. A regex that stops seeing
documents reports zero failures, which is the same exit code as a clean tree
and reads the same in a green check. So this prints how many candidates it
actually examined, and the workflow requires that number to stay above a floor.
A gate nobody has watched fail is not a gate.

WHAT IT FOUND WHEN IT WAS WRITTEN

Nothing. All three classes were clean on the commit that introduced this, which
is the honest thing to record: it is here to keep them clean through the edits
that come next, not because it caught something on the way in.
"""
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
BACKTICK = re.compile(r"`([^`\n]+)`")
LINK = re.compile(r"\[[^\]]*\]\(([^)\s]+)\)")
LINECITE = re.compile(
    r"`?([A-Za-z0-9_./-]+\.(?:rs|py|sh|toml|yml|yaml|cpp|h|md))`?:(\d+)(?:[-–](\d+))?")
SKIP_DIRS = ("vendor/", "target/", "_external/", "node_modules/")


def documents():
    """Tracked markdown only. An untracked file is not something a reader sees,
    and a scratch directory full of demo output would drown the report."""
    out = subprocess.run(["git", "ls-files", "*.md"], cwd=ROOT,
                         capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit("git ls-files failed: " + out.stderr.strip())
    return [f for f in out.stdout.split()
            if f and not any(f.startswith(d) for d in SKIP_DIRS)]


def top_level_dirs():
    return {d for d in os.listdir(ROOT) if os.path.isdir(os.path.join(ROOT, d))}


def main():
    verbose = "--list" in sys.argv
    docs = documents()
    tops = top_level_dirs()
    seen = {"path": 0, "link": 0, "line": 0}
    failures = []
    passed = []

    for doc in docs:
        full = os.path.join(ROOT, doc)
        here = os.path.dirname(full)
        with open(full, encoding="utf-8", errors="replace") as fh:
            lines = fh.read().splitlines()
        for i, line in enumerate(lines, 1):
            # 1 — a path in backticks that claims to be in this tree
            for s in BACKTICK.findall(line):
                s = s.rstrip(".,;:")
                if " " in s or "=" in s or "*" in s or "/" not in s:
                    continue
                if not re.fullmatch(r"[A-Za-z0-9_./-]+", s):
                    continue
                if s.split("/")[0] not in tops:
                    continue
                seen["path"] += 1
                if os.path.exists(os.path.join(ROOT, s.rstrip("/"))):
                    passed.append("path  %s:%d  %s" % (doc, i, s))
                else:
                    failures.append(
                        "%s:%d quotes `%s`, which is not in this tree — a reader who "
                        "goes looking finds nothing, and the sentence reads as a "
                        "stronger control than the repository has" % (doc, i, s))
            # 2 — a link somebody clicks
            for t in LINK.findall(line):
                if t.startswith(("http://", "https://", "mailto:", "#")):
                    continue
                target = t.split("#")[0]
                if not target:
                    continue
                seen["link"] += 1
                if os.path.exists(os.path.join(here, target)):
                    passed.append("link  %s:%d  -> %s" % (doc, i, target))
                else:
                    failures.append(
                        "%s:%d links to %s, which does not resolve from that "
                        "document's own directory — which is where a reader's "
                        "browser resolves it from" % (doc, i, target))
            # 3 — a line citation past the end of its file
            for name, lo, hi in LINECITE.findall(line):
                path = None
                for base in (ROOT, here):
                    cand = os.path.join(base, name)
                    if os.path.isfile(cand):
                        path = cand
                        break
                if path is None:
                    continue
                seen["line"] += 1
                with open(path, encoding="utf-8", errors="replace") as fh:
                    total = sum(1 for _ in fh)
                top = int(hi or lo)
                if top <= total:
                    passed.append("line  %s:%d  %s:%s" % (doc, i, name, hi or lo))
                else:
                    failures.append(
                        "%s:%d cites %s:%s-%s, but that file is %d lines long — the "
                        "citation drifted and nobody re-read it"
                        % (doc, i, name, lo, hi or lo, total))

    print("examined %d repository path(s), %d link(s) and %d line citation(s) "
          "across %d document(s)"
          % (seen["path"], seen["link"], seen["line"], len(docs)))
    # A class that examined nothing is not a class that passed. Saying "0" in a
    # sentence of three numbers is how a matcher that has quietly stopped
    # matching goes unnoticed for a dozen green runs, so it is said in words.
    for label, kind in (("repository path", "path"), ("link", "link"),
                        ("line citation", "line")):
        if seen[kind] == 0:
            print("  NOTE: no %s was examined. Nothing here is of that shape today;\n"
                  "        this class is carried for the documents that come next, and\n"
                  "        it has checked nothing on this run." % label)
    if verbose:
        for p in passed:
            print("  ok  " + p)

    if failures:
        print("\n%d document(s) point at something that is not there:\n" % len(failures))
        for f in failures:
            print("  " + f)
        return 1
    print("every quoted path, every link and every line citation resolves")
    return 0


if __name__ == "__main__":
    sys.exit(main())
