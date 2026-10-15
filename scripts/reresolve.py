#!/usr/bin/env python3
"""Re-read, at the end of a run, the block each transaction of a manifest is in.

The public testnet has rewritten blocks: a transaction seen in block N while the
run waited on it can sit in block N+2 a minute later. The block recorded at
send time is then stale, and verify-onchain.sh would report a mismatch for a
transaction that is on chain. This rewrites the block column with what the
chain says now, and leaves a comment line for every change, so the manifest
says which records moved and from where.

A transaction recorded `not-included` that resolves now is left as it is and
flagged: it landed after the run stopped waiting, and the run's verdict for it
was made without it.

usage: reresolve.py <rpc> <manifest>
"""
import json, sys, urllib.request

rpc, manifest = sys.argv[1], sys.argv[2]

def block_of(tx):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": "getTransaction", "params": [tx]}).encode()
    req = urllib.request.Request(rpc, body, {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=25) as r:
        res = json.load(r).get("result")
    if isinstance(res, list) and len(res) > 1:
        return str(res[1] if not isinstance(res[1], dict) else res[1].get("height"))
    return None

lines = [l.rstrip("\n") for l in open(manifest)]
# `# attempts <label> <hash>...`: every attempt the CLI sent for that step.
attempts = {}
for l in lines:
    if l.startswith("# attempts "):
        _, _, label, *hs = l.split()
        attempts[label] = hs

out, notes = [], []
for line in lines:
    cols = line.split("\t")
    if line.startswith("#") or len(cols) < 5 or cols[4] in ("", "-"):
        out.append(line)
        continue
    label, verdict, block, tx = cols[0], cols[2], cols[3], cols[4]
    now = block_of(tx)
    if verdict in ("applied", "reverted") and now and now != block:
        notes.append(f"# moved {label}: recorded in block {block}, in block {now} at the end of the run")
        cols[3] = now
    elif verdict == "not-included":
        for h in [tx] + [h for h in attempts.get(label, []) if h != tx]:
            at = now if h == tx else block_of(h)
            if at:
                notes.append(f"# late {label}: not included while the run waited; attempt {h} is in block {at} at the end of the run")
                break
    out.append("\t".join(cols))

with open(manifest, "w") as f:
    f.write("\n".join(out + notes) + "\n")
for n in notes:
    print(" ", n[2:])
