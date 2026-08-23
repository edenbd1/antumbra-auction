#!/usr/bin/env bash
# Re-check every claim this repository makes about the chain, from a clean
# clone, with nothing to set up.
#
#   ./scripts/verify-onchain.sh
#
# The point of the controls is the point of the whole script. A getTransaction
# that returns data proves only that the endpoint answers, until a hash that was
# never deployed is shown to return null — and until an instruction the program
# was supposed to *refuse* is shown to be absent. Both run, and neither can be
# filtered out.
#
# Every refusal on this runtime looks the same: no receipt, no reason, just
# "not found in preconfigured amount of blocks". That is why the refusals below
# are recorded by hash rather than described in prose. They cost a full proof
# each — 430 s against 62 s for a fill — so they were expensive to produce and
# are worth keeping.
#
# The account checks matter more than the transaction ones, and are newer. A
# transaction hash proves an instruction landed; an account proves what it did.
# The invariant this engine exists to hold is checkable by anyone from a public
# read, and a check that needs our cooperation is not a check.
#
# Needs curl and python3. Exits non-zero if an expected transaction is missing,
# if a control unexpectedly resolves, or if an account no longer reads as
# claimed.
set -uo pipefail
cd "$(dirname "$0")/.."

RPC="${SEQUENCER_URL:-https://testnet.lez.logos.co}"
BIN=artifacts/programs/antumbra_auction.bin
DEPLOY=ac43ac1a8833f87f623b3346be551ece27da0e3a0c049de3ed86ae4b9607a75b
FIRST_DEPLOY=c00b9698ae21fcfcff3eb05ec3f8367d1378d6df63627d8aa70082875e6c5e43
FIXED_AUCTION=F5cKK2ubAGUYqR5Hbm26GWPqv5zV19GqrJJQQDdzTmV9
FLAWED_AUCTION=9xMhmHEsJXX18f4rBSwFkRKA9kwE2tQBEnKSCXpdK2yB
fail=0
ran=0
refused=0

ok()  { printf '  \033[32mok\033[0m    %s\n' "$1"; ran=$((ran+1)); }
bad() { printf '  \033[31mFAIL\033[0m  %s\n' "$1"; fail=1; ran=$((ran+1)); }

resolves() {
  curl -s -m 25 -X POST "$RPC" -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getTransaction\",\"params\":[\"$1\"]}" \
  | grep -q '"result":\['
}

check() { # label hash expect(yes|no)
  [ "$3" = no ] && refused=$((refused+1))
  if resolves "$2"; then
    [ "$3" = yes ] && ok "$(printf '%-38s' "$1") ${2:0:16}…" \
                   || bad "$(printf '%-38s' "$1") RESOLVED, and it must not"
  else
    [ "$3" = no ]  && ok "$(printf '%-38s' "$1") absent, as a refusal must be" \
                   || bad "$(printf '%-38s' "$1") MISSING $2"
  fi
}

# One account read, decoded here rather than through any tool of ours, and
# tested by an expression given on the command line.
field() { # account skip-fields count expr label
  local out
  out=$(curl -s -m 25 -X POST "$RPC" -H 'Content-Type: application/json' \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccount\",\"params\":[\"$1\"]}" \
        | python3 -c "
import json, sys
r = json.load(sys.stdin).get('result')
if not r or not r.get('data'):
    print('EMPTY'); raise SystemExit
d = bytes(r['data'])
# The layout is the borsh one: a 32-byte id, then u128s in declaration order.
v = [int.from_bytes(d[32+16*i:48+16*i], 'little') for i in range($3)]
print('OK' if ($4) else 'NO', ' '.join(str(x) for x in v))
")
  case "$out" in
    OK*)    ok    "$(printf '%-38s' "$5") holds" ;;
    EMPTY*) bad   "$(printf '%-38s' "$5") the account is empty" ;;
    *)      bad   "$(printf '%-38s' "$5") does NOT hold — ${out#NO }" ;;
  esac
}

# The control for the account reads, and it is not optional. Every `field` above
# would pass just as cheerfully against a node that returned the same bytes for
# every address, so one address that cannot exist has to come back empty.
no_account() { # account label
  refused=$((refused+1))
  local out
  out=$(curl -s -m 25 -X POST "$RPC" -H 'Content-Type: application/json' \
        -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccount\",\"params\":[\"$1\"]}" \
        | python3 -c "
import json, sys
r = json.load(sys.stdin).get('result')
print('EMPTY' if not r or not r.get('data') else 'HAS %d bytes' % len(r['data']))
")
  if [ "$out" = EMPTY ]; then
    ok  "$(printf '%-38s' "$2") empty, as an address nobody wrote must be"
  else
    bad "$(printf '%-38s' "$2") $out — the node answers for any address"
  fi
}

echo "Antumbra collateral auction on the public LEZ testnet — $RPC"

echo
echo "  -- the program, and that the bytes here are the bytes deployed --"
if [ -f "$BIN" ]; then
  computed=$(python3 -c "
import hashlib, struct, sys
b = open(sys.argv[1], 'rb').read()
print(hashlib.sha256(struct.pack('<I', len(b)) + b).hexdigest())" "$BIN")
  # Recomputed from the artifact rather than read from a table beside it: a hash
  # written down next to a binary proves only that someone wrote it down.
  if [ "$computed" = "$DEPLOY" ]; then
    ok "$(printf '%-38s' 'artifact hashes to the deploy') ${computed:0:16}…"
  else
    bad "$(printf '%-38s' 'artifact') hashes to $computed, not the deployed one"
  fi
else
  bad "$(printf '%-38s' 'artifact') $BIN is missing"
fi
check "deploy"                    "$DEPLOY" yes

echo
echo "  -- the lifecycle, driven end to end --"
check "init_config"               887c7b7e43c98265ff1c671b080c077b9138dbae23df73481f433e868ec95398 yes
check "start_auction"             53004d4c5b5bf1582f7df191fa15dfe68e4bd3f18cb2d4c6f8f7e915bde1fa8e yes
check "buy_collateral t=0"        66fe3871ccc9a5838bdc437d0a228c5af35e9f2d0a1ef4288f35ff20639fec60 yes
check "buy_collateral t=1800"     559ef1a16ce9d1c1e99417b30300f9291aef2b6f213acb096e1d5f2c27df8a74 yes
check "a bid larger than the lot"  3a5941a2dfe5b24db331005d9e50027f174992037d90096d90cc18f635190b9d yes

echo
echo "  -- a second configuration, and a second auction under it, at the same time --"
check "init_config (faster)"      0871332b40f5974c9ec81935641eac4b92f6b20a7c30b2e72a8115f6835d147f yes
check "start_auction #2"          260b0e5af35e850bab621cfecaa5196c7ff534c1b867eabffdad032f60228d8b yes
check "buy on auction #2"         a63aa5702e219b834abf9e9796ae3e72659627e791261f560fa0c660af777d65 yes

echo
echo "  -- what the accounts hold, which is what the transactions were for --"
# seized, left, sold, to_raise, raised — R6 from a public read, and a shortfall
# that the auction booked instead of hiding.
field "$FIXED_AUCTION" 0 5 "v[2] + v[1] == v[0]" "R6: sold + left == seized"
field "$FIXED_AUCTION" 0 5 "v[4] < v[3]" "it closed short, and booked it"
field "$FIXED_AUCTION" 0 5 "v[1] == 0" "the lot is exhausted"

echo
echo "  -- the first build, and the defect it still holds --"
# Kept deliberately. The account below is the reason the current build exists:
# it reports raised == to_raise on a lot the schedule could never have sold for
# that much. Layout has no `sold` field, so the indices differ by one.
check "first build's deploy"      "$FIRST_DEPLOY" yes
check "the bid that showed it"    e46fb1acefa644119e728980e64c82eab1a7f6ae5cf5e9ef1cfe5e5f77555ae2 yes
field "$FLAWED_AUCTION" 0 4 "v[3] == v[2]" "it still reads as fully raised"
no_account 11111111111111111111111111111111 "CONTROL an address nobody wrote"

echo
echo "  -- what the program refused, which is what the above means anything against --"
__REFUSALS__
check "CONTROL never-deployed"    dededededededededededededededededededededededededededededededede no

echo
echo "  -- the event mechanism RFP-014 lists as a resolved dependency --"
for method in getTransactionReceipt getEvents getLogs; do
  body="$(curl -s -m 25 -X POST "$RPC" -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":[]}")"
  if printf '%s' "$body" | grep -q 'Method not found'; then
    ok "$(printf '%-38s' "$method") absent, though the RFP calls it delivered"
  else
    bad "$(printf '%-38s' "$method") ANSWERS — the runtime gained events; update the docs"
  fi
done

echo
if [ "$fail" -eq 0 ]; then
  # Counted, not written down: this sentence said "two" while four refusals were
  # in the list, which is the drift the whole script exists to prevent.
  # Counted and conjugated. "1 of them are things that must NOT resolve" is the
  # kind of sentence that tells a reader the number beside it was not computed.
  if [ "$refused" -eq 1 ]; then
    echo "All $ran checks hold. One of them is a thing that must NOT resolve,"
    echo "and it still does not — which is what makes the rest mean something."
  else
    echo "All $ran checks hold. $refused of them are things that must NOT resolve,"
    echo "and they still do not — which is what makes the rest mean something."
  fi
else
  echo "Something above did not hold." >&2
fi
exit "$fail"
