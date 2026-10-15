#!/usr/bin/env bash
# Drive the whole antumbra_auction lifecycle against a LEZ v0.3 sequencer.
#
# Written for a local standalone sequencer (docs/deploy-v03.md), where the
# genesis accounts are funded; pointed at the public testnet it is the same run
# once PAYER holds LGO (and FUND_EACH funds the other signers).
#
# The scenario is the one the v0.2.4 program ran on the public testnet, on a
# clock the chain actually keeps: a 2% discount at open growing to 20%, ten
# units of collateral priced at 2000, raising 20000; the window is 120 s rather
# than an hour so the run finishes in minutes. Every step prints the CLI's JSON
# line and is checked against what it must do: `applied` where the program
# must accept, `reverted` (included, and charged, with no effect) or
# `rejected` (never included) where it must refuse. State is read back from the
# chain around the steps.
#
# Needs: the v0.3 `wallet` and `antumbra-auction` binaries, LEE_WALLET_HOME_DIR
# with PAYER imported, and the program deployed (ANTUMBRA_PROGRAM), or DEPLOY=1
# to deploy it first.
#
# Writes $OUT (default evidence/v03/local-e2e.tsv): label expected verdict block tx
set -uo pipefail
cd "$(dirname "$0")/.."

: "${LEE_WALLET_HOME_DIR:?set LEE_WALLET_HOME_DIR to the v0.3 wallet home}"
: "${PAYER:?base58 id of a funded key that pays every fee}"
RPC="${RPC:-http://127.0.0.1:3040}"
export ANTUMBRA_RPC="$RPC"
W="${WALLET:-wallet}"
CLI="${CLI:-cli/target/release/antumbra-auction}"
OUT="${OUT:-evidence/v03/local-e2e.tsv}"
TAG="${TAG:-e2e}"
export ANTUMBRA_ELF="${ANTUMBRA_ELF:-artifacts/programs/v0.3/antumbra_auction.bin}"
mkdir -p "$(dirname "$OUT")"

if [ "${DEPLOY:-0}" = 1 ]; then
  line="$("$CLI" deploy --payer "$PAYER" --immutable 2>/dev/null | grep '^{' | tail -1)"
  echo "$line"
  ANTUMBRA_PROGRAM="$(printf '%s' "$line" | python3 -c 'import json,sys;print(json.load(sys.stdin)["header"])')"
fi
: "${ANTUMBRA_PROGRAM:?deploy first (DEPLOY=1) or set ANTUMBRA_PROGRAM}"
export ANTUMBRA_PROGRAM
echo "# run against $RPC, $(date -u +%Y-%m-%dT%H:%M:%SZ)" | tee "$OUT"
echo "# program $ANTUMBRA_PROGRAM $("$CLI" image-id)" | tee -a "$OUT"

new_pub() { "$W" account new public 2>/dev/null | grep -oE 'Public/[A-Za-z0-9]+' | head -1 | cut -d/ -f2; }
rpc() { curl -s -m 20 -X POST "$RPC" -H 'Content-Type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":[\"$2\"]}"; }
shard() { # account program -> hex of that shard
  rpc getAccount "$1" | python3 -c '
import json,sys
r=json.load(sys.stdin).get("result") or {}
print(bytes((r.get("data") or {}).get("shards",{}).get(sys.argv[1],[])).hex())' "$2"; }
jget() { python3 -c 'import json,sys;print(json.loads(sys.argv[1])[sys.argv[2]])' "$1" "$2"; }
show() { "$CLI" show "$1"; }
field() { jget "$(show "$1")" "$2"; }
now() { "$CLI" now; }

fail=0
check() { # label got want
  if [ "$2" = "$3" ]; then echo "  ok  $1: $2"; echo "# check ok: $1 = $2" >> "$OUT"
  else echo "  FAIL $1: got $2, want $3"; echo "# check FAIL: $1: $2 != $3" >> "$OUT"; fail=1; fi
}
# step label expected(applied|refused) -- cli args...
step() {
  local label="$1" expect="$2"; shift 3
  local line verdict block tx
  line="$("$CLI" --label "$label" "$@" 2>/tmp/antumbra-auction-e2e.err | grep '^{' | tail -1)"
  [ -n "$line" ] || line="{\"verdict\":\"error\",\"error\":\"$(tail -1 /tmp/antumbra-auction-e2e.err | tr '"' "'")\"}"
  echo "$line"
  verdict="$(jget "$line" verdict)"; block="$(python3 -c 'import json,sys;print(json.loads(sys.argv[1]).get("block"))' "$line")"
  tx="$(python3 -c 'import json,sys;print(json.loads(sys.argv[1]).get("tx","-"))' "$line")"
  printf '%s\t%s\t%s\t%s\t%s\n' "$label" "$expect" "$verdict" "$block" "$tx" >> "$OUT"
  python3 -c 'import json,sys;a=json.loads(sys.argv[1]).get("attempts");a and print("# attempts",sys.argv[2]," ".join(a))' "$line" "$label" >> "$OUT"
  case "$expect:$verdict" in
    applied:applied|refused:reverted|refused:rejected|refused:not-included) echo "  ok  $label: $verdict" ;;
    *) echo "  FAIL $label: $verdict, expected $expect"; fail=1 ;;
  esac
}
# Wait until the chain's clock is at least $1 ms.
until_clock() { while [ "$(now)" -lt "$1" ]; do sleep 5; done; }

WAD=1000000000000000000
# Amounts in base units. 10 WAD is past bash's 64-bit integers, so every
# amount goes through python: `w 2000` is 2000 WAD.
w() { python3 -c "print(int($1) * 10**18)"; }
gt() { python3 -c "print(int('$1') > int('$2'))"; }
ADMIN="$PAYER"; BIDDER=$(new_pub); OTHER=$(new_pub)
echo "# payer $PAYER admin $ADMIN bidder $BIDDER other $OTHER" >> "$OUT"
P=(--payer "$PAYER")
# On a chain with real fees every account that signs must be able to pay for
# its own transaction. FUND_EACH sends that much from PAYER to each such
# account; the local debug chain does not need it.
FUND_EACH="${FUND_EACH:-0}"
if [ "$FUND_EACH" -gt 0 ]; then
  for a in "$BIDDER" "$OTHER"; do
    "$W" auth-transfer send --from "Public/$PAYER" --to "Public/$a" --amount "$FUND_EACH" 2>&1 | grep -E "hash" | sed "s/^/  fund $a: /"
  done
fi
FINAL=()
pin() { FINAL+=("$1 $ANTUMBRA_PROGRAM"); }

CA="$TAG-cfg-a"; CB="$TAG-cfg-b"; AX="$TAG-lot-x"; AY="$TAG-lot-y"
MIN=$((WAD - WAD / 50)); MAX=$((WAD - WAD / 5))

echo "== 1. two configurations, side by side (F7, R3)"
step init-a applied -- "${P[@]}" init-config --config-id "$CA" --min-multiplier "$MIN" --max-multiplier "$MAX" --window 120 --admin "$ADMIN"
check "config A names its admin" "$(jget "$("$CLI" show-config "$CA")" admin)" "$ADMIN"
step init-a-again refused -- "${P[@]}" init-config --config-id "$CA" --min-multiplier "$MIN" --max-multiplier "$MAX" --window 120 --admin "$ADMIN"
step init-bad-schedule refused -- "${P[@]}" init-config --config-id "$TAG-cfg-bad" --min-multiplier "$MAX" --max-multiplier "$MIN" --window 120 --admin "$ADMIN"
step init-b applied -- "${P[@]}" init-config --config-id "$CB" --min-multiplier $((WAD - WAD / 20)) --max-multiplier $((WAD - WAD * 3 / 10)) --window 60 --admin "$OTHER"
pin "$(jget "$("$CLI" ids --config-id "$CA")" config)"; pin "$(jget "$("$CLI" ids --config-id "$CB")" config)"

echo "== 2. an auction opened by the config's admin, its schedule checked against the config"
step start-by-non-admin refused -- "${P[@]}" --force start --auction-id "$AX" --config-id "$CA" --collateral $(w 10) --to-raise $(w 20000) --price $(w 2000) --admin "$BIDDER"
step start-wrong-schedule refused -- "${P[@]}" --force start --auction-id "$AX" --config-id "$CA" --collateral $(w 10) --to-raise $(w 20000) --price $(w 2000) --admin "$ADMIN" --max-multiplier $((MAX - 1))
step start-empty-lot refused -- "${P[@]}" start --auction-id "$AX" --config-id "$CA" --collateral 0 --to-raise $(w 20000) --price $(w 2000) --admin "$ADMIN"
check "nothing opened by the refusals" "$(shard "$(jget "$("$CLI" ids --auction-id "$AX")" auction)" "$ANTUMBRA_PROGRAM")" ""
step start applied -- "${P[@]}" start --auction-id "$AX" --config-id "$CA" --collateral $(w 10) --to-raise $(w 20000) --price $(w 2000) --admin "$ADMIN"
OPENED=$(field "$AX" opened_at)
check "the lot holds what was seized" "$(field "$AX" collateral_left)" $(w 10)
step start-again refused -- "${P[@]}" --force start --auction-id "$AX" --config-id "$CA" --collateral $(w 10) --to-raise $(w 20000) --price $(w 2000) --admin "$ADMIN"

echo "== 3. partial fills at an escalating discount (F2, F3)"
step bid-too-small refused -- "${P[@]}" --force buy --auction-id "$AX" --bid 1 --at "$OPENED" --bidder "$BIDDER"
step buy-at-open applied -- "${P[@]}" buy --auction-id "$AX" --bid $(w 2000) --at "$OPENED" --bidder "$BIDDER"
FIRST=$(field "$AX" collateral_sold)
until_clock $((OPENED + 60000))
step buy-60s-later applied -- "${P[@]}" buy --auction-id "$AX" --bid $(w 2000) --at $((OPENED + 60000)) --bidder "$BIDDER"
SECOND=$(python3 -c "print(int('$(field "$AX" collateral_sold)') - int('$FIRST'))")
check "the same bid bought more collateral 60 s later" "$(gt "$SECOND" "$FIRST")" True
step bid-backdated refused -- "${P[@]}" --force buy --auction-id "$AX" --bid $(w 2000) --at $((OPENED + 30000)) --bidder "$BIDDER"
step settle-unfinished refused -- "${P[@]}" --force settle --auction-id "$AX" --at $((OPENED + 60000)) --caller "$BIDDER"

echo "== 4. the lot runs out: the price re-struck, the shortfall booked, the auction closed (F2, R6)"
until_clock $((OPENED + 120000))
step buy-out applied -- "${P[@]}" buy --auction-id "$AX" --bid $(w 16000) --at $((OPENED + 120000)) --bidder "$BIDDER"
check "collateral left" "$(field "$AX" collateral_left)" 0
check "sold + left == seized (R6, from the account)" "$(field "$AX" r6_sold_plus_left_is_seized)" True
check "settled by the last fill" "$(field "$AX" settled)" 1
check "a shortfall is booked, not hidden" "$(gt "$(field "$AX" shortfall)" 0)" True
check "raised is the library's figure" "$(field "$AX" raised)" 16569594129786746160000
step settle-twice refused -- "${P[@]}" --force settle --auction-id "$AX" --at $((OPENED + 120000)) --caller "$BIDDER"
step bid-after-settle refused -- "${P[@]}" --force buy --auction-id "$AX" --bid "$WAD" --at $((OPENED + 120000)) --bidder "$BIDDER"
pin "$(jget "$("$CLI" ids --auction-id "$AX")" auction)"

echo "== 5. under the second config, a target met exactly (F2's other end)"
step start-y applied -- "${P[@]}" start --auction-id "$AY" --config-id "$CB" --collateral $(w 1000) --to-raise $(w 2000) --price $(w 2000) --admin "$OTHER"
step buy-target applied -- "${P[@]}" buy --auction-id "$AY" --bid $(w 5000) --at "$(field "$AY" opened_at)" --bidder "$BIDDER"
check "raised exactly the target, the bid clamped" "$(field "$AY" raised)" $(w 2000)
check "settled" "$(field "$AY" settled)" 1
pin "$(jget "$("$CLI" ids --auction-id "$AY")" auction)"

echo "== 6. a bid on an auction that does not exist"
step bid-nowhere refused -- "${P[@]}" --force buy --auction-id "$TAG-no-such-lot" --bid "$WAD" --at "$(now)" --bidder "$BIDDER"

for f in "${FINAL[@]}"; do
  set -- $f
  echo "# final $1 $2 $(shard "$1" "$2" | python3 -c 'import hashlib,sys;print(hashlib.sha256(bytes.fromhex(sys.stdin.read().strip())).hexdigest())')" >> "$OUT"
done

python3 scripts/reresolve.py "$RPC" "$OUT"
applied=$(awk -F'\t' '$3=="applied"' "$OUT" | wc -l | tr -d ' ')
refused=$(awk -F'\t' '$2=="refused" && ($3=="reverted"||$3=="rejected"||$3=="not-included")' "$OUT" | wc -l | tr -d ' ')
checks=$(grep -c '^# check ok' "$OUT")
echo "summary: $applied applied, $refused refused as expected, $checks state checks ok, failures=$fail"
exit $fail
