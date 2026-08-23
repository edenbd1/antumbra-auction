#!/usr/bin/env bash
# Re-check every claim this repository makes about the chain, from a clean
# clone, with nothing to set up.
#
#   ./scripts/verify-onchain.sh
#
# The point of the last section is the point of the whole script. A
# getTransaction that returns data proves only that the endpoint answers, until
# a hash that was never deployed is shown to return null — and until an
# instruction the program was supposed to *refuse* is shown to be absent. Both
# controls run, and neither can be filtered out.
#
# Every refusal on this runtime looks the same: no receipt, no reason, just
# "not found in preconfigured amount of blocks". That is why the refusals below
# are recorded by hash rather than described in prose. They cost a full proof
# each — 430 s against 62 s for a fill — so they were expensive to produce and
# are worth keeping.
#
# Needs curl and python3. Exits non-zero if an expected transaction is missing
# or if a control unexpectedly resolves.
set -uo pipefail
cd "$(dirname "$0")/.."

RPC="${SEQUENCER_URL:-https://testnet.lez.logos.co}"
BIN=artifacts/programs/antumbra_auction.bin
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
    [ "$3" = yes ] && ok "$(printf '%-30s' "$1") ${2:0:16}…" \
                   || bad "$(printf '%-30s' "$1") RESOLVED, and it must not"
  else
    [ "$3" = no ]  && ok "$(printf '%-30s' "$1") absent, as a refusal must be" \
                   || bad "$(printf '%-30s' "$1") MISSING $2"
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
  if [ "$computed" = c00b9698ae21fcfcff3eb05ec3f8367d1378d6df63627d8aa70082875e6c5e43 ]; then
    ok "$(printf '%-30s' 'artifact hashes to the deploy') ${computed:0:16}…"
  else
    bad "$(printf '%-30s' 'artifact') hashes to $computed, not the deployed one"
  fi
else
  bad "$(printf '%-30s' 'artifact') $BIN is missing"
fi
check "deploy"                c00b9698ae21fcfcff3eb05ec3f8367d1378d6df63627d8aa70082875e6c5e43 yes

echo
echo "  -- the lifecycle, driven end to end --"
check "init_config"           37581bb093e7e775710e985847f84a5baf5ca0b0709401c8378ffb93deb17321 yes
check "start_auction"         1379ed3c9df06752c0dfd4749175b50de64d79317274ea2856fbce0abab9986b yes
check "buy_collateral t=0"    e95fe0d05c3dd009b6df578754f6d0a0332cda482aa42a9c0803915b37117029 yes
check "buy_collateral t=1800" fd108f3f7e681971d81c3ea7cebb277506b7fa294f205737ca6b5b7e1234ae3e yes
check "buy that exhausts the lot" 5ad0ab52648edf1de9c0bfce49e9e0d294eb0619bf9e13e2377438524116d0ca yes

echo
echo "  -- a second configuration, and a second auction under it, at the same time --"
check "init_config (faster)"  1fdf1a3052d904f9549fc85b8d9cbd708f60f54c36345b1fc33c3f532aeb0de3 yes
check "start_auction #2"      4a69eaf6d5e963a814c2cab756104a79e3423f9ce861b91024b948b4c5c5f14d yes
check "buy on auction #2"     961f437162ce144fed0e462a783c8908862e3786a782353f9fb428132dd6d22e yes

echo
echo "  -- what the program refused, which is what the above means anything against --"
check "REFUSED clock rewound" 25c6014dbc1a643fedcfda871970ac944ae56807ca706700631e3309ad92d51e no
check "REFUSED settle early"  d7de1bc3371e78d0072385e852dadbe22edfc5322d7484cca76c864bcbbbc112 no
check "REFUSED no such auction" a61aed9edd7f838a6c8118238bd7afa5b2af1480ef0541aeda56ff4f2f21cbfd no
check "REFUSED settle twice"  1a16f15f16e33c91eff87d118015957bf5c5e02c867526bef80701586541b28f no
check "CONTROL never-deployed"  dededededededededededededededededededededededededededededededede no

echo
echo "  -- the event mechanism RFP-014 lists as a resolved dependency --"
for method in getTransactionReceipt getEvents getLogs; do
  body="$(curl -s -m 25 -X POST "$RPC" -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$method\",\"params\":[]}")"
  if printf '%s' "$body" | grep -q 'Method not found'; then
    ok "$(printf '%-30s' "$method") absent, though the RFP calls it delivered"
  else
    bad "$(printf '%-30s' "$method") ANSWERS — the runtime gained events; update the docs"
  fi
done

echo
if [ "$fail" -eq 0 ]; then
  # Counted, not written down: this sentence said "two" while four refusals were
  # in the list, which is the drift the whole script exists to prevent.
  echo "All $ran checks hold. $refused of them are things that must NOT resolve,"
  echo "and they still do not — which is what makes the rest mean something."
else
  echo "Something above did not hold." >&2
fi
exit "$fail"
