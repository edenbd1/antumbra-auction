#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Every explorer link in these documents points at something the chain has.

    ./scripts/check-chain-refs.py            # resolve every reference
    ./scripts/check-chain-refs.py --list     # also print what resolved

WHY THIS EXISTS

A transaction hash is sixty-four characters of hex. Nothing about looking at one
tells you whether it is real, and a document full of them reads as evidence
whether or not any of them exist. That is not a hypothetical: this gate was
written after a hash was assembled from a prefix and a suffix that were both
correct, pasted into a link, and read as fine — the two ends matched the table
above it, and the fifty-six characters in between were invented. One of them was
even sixty-three characters long. Nothing in the tree could have noticed.

WHAT IT CHECKS

Every `explorer.testnet.lez.logos.co/transaction/<hash>` resolves through
`getTransaction`, and every `/account/<address>` through `getAccount`. Those are
the two the documents link to; a reference that resolves is real and a reference
that does not is either invented or on a chain this one has replaced, and both
are worth stopping for.

WHAT IT DOES NOT CHECK

That a transaction is the *right* one. A hash can exist and still be the wrong
citation, and no amount of resolving will say so. This closes the cheaper half:
that it exists at all.

--explorer ALSO ASKS THE EXPLORER

`getTransaction` says the node has it. It does not say a reader clicking the
link sees anything: the explorer is a separate index and reaches a transaction
about an hour and three quarters after the sequencer does. With `--explorer`
each transaction page is fetched too, and judged on size — the page for a hash
that cannot exist is a ~2.4 kB shell, and any real one is several times that.

The threshold is not trusted, it is measured on every run: an impossible hash is
fetched first, and if THAT comes back looking like a found transaction the whole
comparison is meaningless and this aborts rather than reporting verdicts against
a baseline that says nothing. That control is the reason this can use a size at
all instead of driving a browser.

NO SKIP PATH, AND TWO KINDS OF FAILURE. If the node cannot be reached this exits
non-zero and says the node was unreachable — it does not report the references
as missing, because "I could not look" and "it is not there" are different
sentences and only one of them is about the documents.
"""
import json
import os
import re
import subprocess
import sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
RPC = os.environ.get("LEZ_RPC", "https://testnet.lez.logos.co")
TX = re.compile(r"explorer\.testnet\.lez\.logos\.co/transaction/([0-9a-fA-F]{8,})")
ACCT = re.compile(r"explorer\.testnet\.lez\.logos\.co/account/([1-9A-HJ-NP-Za-km-z]{20,})")
SKIP_DIRS = ("vendor/", "target/", "_external/", "node_modules/")


def documents():
    out = subprocess.run(["git", "ls-files", "*.md"], cwd=ROOT,
                         capture_output=True, text=True)
    if out.returncode != 0:
        sys.exit("git ls-files failed: " + out.stderr.strip())
    return [f for f in out.stdout.split()
            if f and not any(f.startswith(d) for d in SKIP_DIRS)]


def call(method, param):
    """Returns (result, transport_ok). transport_ok is False only when the node
    could not be reached — never when it answered and said no."""
    try:
        r = subprocess.run(
            ["curl", "-sS", "-X", "POST", RPC, "-H", "Content-Type: application/json",
             "--max-time", "30", "-d",
             json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": [param]})],
            capture_output=True, text=True)
    except (FileNotFoundError, PermissionError) as exc:
        return None, False
    if r.returncode != 0 or not r.stdout.strip():
        return None, False
    try:
        body = json.loads(r.stdout)
    except ValueError:
        return None, False
    if "error" in body and body["error"].get("code") == -32601:
        # The method itself is gone. That is a change in the node, not a
        # document that lies, and reporting it as one would be a confident
        # accusation about the wrong thing.
        print("the node does not know `%s`. This gate cannot check anything\n"
              "until it is told the right method name; it is not reporting the\n"
              "documents as wrong, because it has not looked at the chain."
              % method)
        sys.exit(1)
    return body.get("result"), True


def page_size(url):
    r = subprocess.run(["curl", "-sS", "-o", "/dev/null", "-w", "%{size_download}",
                        "-L", "--max-time", "45", url], capture_output=True, text=True)
    try:
        return int(r.stdout.strip())
    except ValueError:
        return None


def explorer_baseline():
    """What a page for a hash that cannot exist looks like. Measured, not assumed."""
    impossible = "ff" * 32
    n = page_size("https://explorer.testnet.lez.logos.co/transaction/" + impossible)
    if n is None:
        print("the explorer could not be reached, so no page could be checked.")
        return None
    return n


def main():
    verbose = "--list" in sys.argv
    want_explorer = "--explorer" in sys.argv
    refs = {}
    for doc in documents():
        with open(os.path.join(ROOT, doc), encoding="utf-8", errors="replace") as fh:
            for lineno, line in enumerate(fh, 1):
                for h in TX.findall(line):
                    refs.setdefault(("tx", h.lower()), []).append("%s:%d" % (doc, lineno))
                for a in ACCT.findall(line):
                    refs.setdefault(("acct", a), []).append("%s:%d" % (doc, lineno))

    if not refs:
        print("no explorer link appears in any document, so there is nothing to\n"
              "resolve. That is suspicious rather than clean: a deployment nobody\n"
              "can look up is asserted rather than checkable.")
        return 1

    failures, resolved, short = [], 0, 0
    for (kind, key), sites in sorted(refs.items()):
        if kind == "tx" and len(key) != 64:
            # A truncated hash in a URL cannot be resolved and cannot be
            # trusted either; say so rather than skip it.
            short += 1
            failures.append("%s is %d hex characters, not 64, and is used as a "
                            "transaction URL at %s — a link nobody can follow"
                            % (key, len(key), ", ".join(sites)))
            continue
        result, ok = call("getTransaction" if kind == "tx" else "getAccount", key)
        if not ok:
            print("the node at %s could not be reached, so no reference could be\n"
                  "checked. This gate has no skip path: passing here without having\n"
                  "looked is the failure it exists to prevent." % RPC)
            return 1
        if result:
            resolved += 1
            if verbose:
                print("  ok  %-4s %s" % (kind, key))
        else:
            failures.append("%s %s does not exist on this chain, and is linked at %s"
                            % (kind, key, ", ".join(sites)))

    print("resolved %d of %d explorer reference(s) across %d document(s)"
          % (resolved, len(refs), len(documents())))

    if want_explorer and not failures:
        base = explorer_baseline()
        if base is None:
            return 1
        # A found page must be clearly bigger than the not-found shell. If the
        # shell is not small, the size says nothing and neither does this gate.
        if base > 3500:
            print("\nthe control page for a hash that cannot exist came back at %d bytes.\n"
                  "That is not the small shell this comparison depends on, so a size\n"
                  "cannot separate found from not-found here and no verdict below\n"
                  "would mean anything. Aborting rather than reporting one." % base)
            return 1
        print("explorer control: a hash that cannot exist renders %d bytes" % base)
        thin = []
        for (kind, key), sites in sorted(refs.items()):
            if kind != "tx":
                continue
            n = page_size("https://explorer.testnet.lez.logos.co/transaction/" + key)
            if n is None or n <= base * 1.3:
                thin.append("%s renders %s bytes on the explorer, against a %d-byte "
                            "not-found shell — the node has it, the index does not "
                            "show it yet; linked at %s"
                            % (key, n, base, ", ".join(sites)))
        if thin:
            print("\n%d transaction(s) the node has and the explorer does not show:\n"
                  % len(thin))
            for t in thin:
                print("  " + t)
            return 1
        print("every transaction page renders well above the not-found shell")
    if failures:
        print("\n%d reference(s) a reader could not follow:\n" % len(failures))
        for f in failures:
            print("  " + f)
        return 1
    print("every explorer link points at something this chain has")
    return 0


if __name__ == "__main__":
    sys.exit(main())
