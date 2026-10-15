#!/usr/bin/env bash
# Re-check a v0.3 run of antumbra_auction against a LEZ v0.3 sequencer, from a
# manifest rather than from hashes written into this script.
#
#   ./scripts/verify-onchain.sh                       # evidence/v03/testnet.tsv, public testnet
#   ./scripts/verify-onchain.sh --manifest evidence/v03/local-e2e.tsv --rpc http://127.0.0.1:3040
#
# A manifest is what scripts/e2e-v03.sh writes: one line per transaction
#   label  expected(applied|refused)  verdict  block  tx
# and comment lines, of which three kinds are checked:
#   # program <header>  <image-id-hex>        the header's program_loader shard names this ImageID
#   # final   <account> <shard-program> <sha256-of-shard-bytes>
#   # check ok: ...                            (informational; counted, not re-run)
#
# What is checked, and what a v0.3 chain lets one check:
# - applied and reverted transactions are both included in a block, and must be
#   found on chain (one a block rewrite moved is reported as moved). On v0.3 a refused public transaction
#   is included and charged; what makes it a refusal is that it changed nothing,
#   which the final-state lines below pin down.
# - rejected and never-included transactions must not resolve.
# - every `final` line: the account's shard hashes to the recorded value now.
#   Configs and auctions are PDAs only this program can change, so their final
#   state is stable once a run ends.
# - a random hash must not resolve, or the endpoint is answering anything.
#
# Needs curl and python3. Exits non-zero on any mismatch.
set -uo pipefail
cd "$(dirname "$0")/.."
MANIFEST="evidence/v03/testnet.tsv"
RPC="${SEQUENCER_URL:-https://testnet.lez.logos.co}"
while [ $# -gt 0 ]; do
  case "$1" in
    --manifest) MANIFEST="${2:?}"; shift 2 ;;
    --rpc) RPC="${2:?}"; shift 2 ;;
    *) echo "usage: $0 [--manifest FILE] [--rpc URL]" >&2; exit 2 ;;
  esac
done

if [ ! -f "$MANIFEST" ]; then
  echo "No manifest at $MANIFEST."
  echo "The program has not been run on the public testnet v0.3 yet; the local run is"
  echo "evidence/v03/local-e2e.tsv (pass --manifest and --rpc to a local sequencer)."
  echo "The v0.2.4 evidence is historical; its checker is scripts/verify-onchain-v024.sh."
  exit 3
fi

python3 - "$MANIFEST" "$RPC" <<'PY'
import hashlib, json, os, secrets, sys, urllib.request

manifest, rpc = sys.argv[1], sys.argv[2]

def call(method, param):
    body = json.dumps({"jsonrpc": "2.0", "id": 1, "method": method, "params": [param]}).encode()
    req = urllib.request.Request(rpc, body, {"Content-Type": "application/json"})
    with urllib.request.urlopen(req, timeout=25) as r:
        return json.load(r)

def shard(account, program):
    res = call("getAccount", account).get("result") or {}
    return bytes(((res.get("data") or {}).get("shards") or {}).get(program, []))

LOADER = "JAQ5MVHbCkSYRzXunsrNuM2m1LS859PGveHfoYPAmcvZ"  # [0xFE; 32], the program_loader dispatch id
B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz"
def b58(b):
    n = int.from_bytes(b, "big"); s = ""
    while n:
        n, r = divmod(n, 58); s = B58[r] + s
    return "1" * (len(b) - len(b.lstrip(b"\0"))) + s
assert b58(bytes([0xFE] * 32)) == LOADER

ok = bad = moved = 0
def report(good, text):
    global ok, bad
    if good:
        ok += 1; print(f"  ok   {text}")
    else:
        bad += 1; print(f"  FAIL {text}")

print(f"antumbra_auction v0.3 against {rpc}, from {manifest}")
checks_recorded = 0
for line in open(manifest):
    line = line.rstrip("\n")
    if line.startswith("# program "):
        _, _, header, image = line.split()[:4]
        h = shard(header, LOADER)
        # ProgramHeader { image_id: [u32; 8], program_first_segment, immutable }
        got = h[:32].hex() if len(h) >= 32 else ""
        report(got == image, f"program header {header[:12]}… names ImageID {image[:16]}…" + ("" if got == image else f" (chain has {got or 'nothing'})"))
    elif line.startswith("# final "):
        _, _, account, program, want = line.split()[:5]
        got = hashlib.sha256(shard(account, program)).hexdigest()
        report(got == want, f"final state of {account[:12]}… shard {program[:8]}… is as the run left it")
    elif line.startswith("# check ok"):
        checks_recorded += 1
    elif line and not line.startswith("#"):
        label, expected, verdict, block, tx = line.split("\t")[:5]
        if tx in ("", "-"):
            report(verdict in ("rejected", "not-included"), f"{label}: {verdict}, never sent to a block")
            continue
        res = call("getTransaction", tx).get("result")
        found = None
        if isinstance(res, list) and len(res) > 1:
            found = res[1] if not isinstance(res[1], dict) else res[1].get("height")
        if verdict in ("applied", "reverted"):
            # Included is what is checked. A chain that rewrites recent blocks
            # (the public testnet has) can move an included transaction a few
            # blocks; that is reported, not failed: what it did is pinned by
            # the final-state lines, which do not move.
            if found is not None and str(found) != block:
                moved += 1
            report(found is not None,
                   f"{label}: {verdict}, in block {block}" + ("" if found is None or str(found) == block else f" (now in block {found}: moved by a block rewrite)") + ("" if found is not None else " (not found on chain)"))
        else:
            report(found is None, f"{label}: {verdict}, and it does not resolve")

ctl = secrets.token_hex(32)
res = call("getTransaction", ctl).get("result")
report(res is None, "control: a random hash does not resolve")
print(f"{ok} checks pass, {bad} fail ({checks_recorded} state checks recorded during the run" + (f"; {moved} transactions moved by a block rewrite since" if moved else "") + ")")
sys.exit(1 if bad else 0)
PY
