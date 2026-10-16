#!/usr/bin/env bash
# Liquidation, the price feed and the debt auction of antumbra_auction against a
# LEZ v0.3 sequencer: F1, F4, R2, R4 and R5 on chain.
#
# The companion of scripts/e2e-v03.sh, with the same helpers and the same
# manifest format, so scripts/verify-onchain.sh re-checks it unchanged.
#
#   1. a config, two collateral types (A and B) with their own feeds, the
#      config's debt auction terms over a mock protocol token address, and five
#      positions opened by one owner, safe at the opening prices;
#   2. F1: both feeds fall; once A's time-weighted price is below the
#      position's ratio, three DIFFERENT fresh signers each liquidate one
#      position (two of type A, one of type B); a second liquidation of the
#      same position and one of a safe position are refused;
#   3. F4: the first liquidation's auction is bought out at its deepest
#      discount and closes short of debt plus penalty; anyone books the
#      shortfall into a debt auction (once); two bidders shrink the protocol
#      token lot by at least 3% each; it settles when the standing bid expires;
#   4. R2/R4/R5: A's feed is left to go stale while B's is refreshed: a
#      liquidation of type A is refused while one of type B lands, and a bid on
#      type A's open auction still lands; A's feed resumes and so does A.
#
# Needs: the v0.3 `wallet` and `antumbra-auction` binaries, LEE_WALLET_HOME_DIR
# with PAYER imported, and the program deployed (ANTUMBRA_PROGRAM) or DEPLOY=1.
# PAYER pays every fee and is the config's admin and the feeds' writer; every
# other signer is a fresh account made by this run (FUND_EACH funds each on a
# chain where a signer must hold a balance).
#
# Writes $OUT (default evidence/v03/liquidation/local-e2e.tsv).
set -uo pipefail
cd "$(dirname "$0")/.."

: "${LEE_WALLET_HOME_DIR:?set LEE_WALLET_HOME_DIR to the v0.3 wallet home}"
: "${PAYER:?base58 id of a funded key that pays every fee}"
RPC="${RPC:-http://127.0.0.1:3040}"
export ANTUMBRA_RPC="$RPC"
W="${WALLET:-wallet}"
CLI="${CLI:-cli/target/release/antumbra-auction}"
OUT="${OUT:-evidence/v03/liquidation/local-e2e.tsv}"
TAG="${TAG:-liq}"
# A's feed is usable for liquidation while (now + 2 min) - last price <= this.
MAX_AGE_A="${MAX_AGE_A:-300000}"
# How long a standing debt bid has before it wins.
BID_TTL="${BID_TTL:-60000}"
export ANTUMBRA_ELF="${ANTUMBRA_ELF:-artifacts/programs/v0.3/antumbra_auction.bin}"
mkdir -p "$(dirname "$OUT")"

balance() { curl -s -m 20 -X POST "$RPC" -H 'Content-Type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccount\",\"params\":[\"$1\"]}" |
  python3 -c 'import json,sys;r=json.load(sys.stdin).get("result") or {};print(int.from_bytes(bytes((r.get("data") or {}).get("shards",{}).get("11111111111111111111111111111111",[])),"little"))'; }
DEPLOY_SPENT=""
if [ "${DEPLOY:-0}" = 1 ]; then
  b0=$(balance "$PAYER")
  line="$("$CLI" deploy --payer "$PAYER" --immutable 2>/dev/null | grep '^{' | tail -1)"
  DEPLOY_SPENT=$(python3 -c "print($b0 - $(balance "$PAYER"))")
  echo "$line"
  ANTUMBRA_PROGRAM="$(printf '%s' "$line" | python3 -c 'import json,sys;print(json.load(sys.stdin)["header"])')"
fi
: "${ANTUMBRA_PROGRAM:?deploy first (DEPLOY=1) or set ANTUMBRA_PROGRAM}"
export ANTUMBRA_PROGRAM
echo "# run against $RPC, $(date -u +%Y-%m-%dT%H:%M:%SZ)" | tee "$OUT"
echo "# program $ANTUMBRA_PROGRAM $("$CLI" image-id)" | tee -a "$OUT"
[ -n "$DEPLOY_SPENT" ] && echo "# payer spent $DEPLOY_SPENT on the deploy" >> "$OUT"
[ -n "${GAS_LIMIT:-}" ] && echo "# gas limit $GAS_LIMIT on every transaction (a refused one forfeits gas_limit x base fee)" >> "$OUT"

new_pub() { "$W" account new public 2>/dev/null | grep -oE 'Public/[A-Za-z0-9]+' | head -1 | cut -d/ -f2; }
rpc() { curl -s -m 20 -X POST "$RPC" -H 'Content-Type: application/json' \
  -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\",\"params\":[\"$2\"]}"; }
shard() { # account program -> hex of that shard
  rpc getAccount "$1" | python3 -c '
import json,sys
r=json.load(sys.stdin).get("result") or {}
print(bytes((r.get("data") or {}).get("shards",{}).get(sys.argv[1],[])).hex())' "$2"; }
jget() { python3 -c 'import json,sys;v=json.loads(sys.argv[1])[sys.argv[2]];print(v)' "$1" "$2"; }
now() { "$CLI" now; }
ids() { "$CLI" ids "$@"; }

fail=0
check() { # label got want
  if [ "$2" = "$3" ]; then echo "  ok  $1: $2"; echo "# check ok: $1 = $2" >> "$OUT"
  else echo "  FAIL $1: got $2, want $3"; echo "# check FAIL: $1: $2 != $3" >> "$OUT"; fail=1; fi
}
# step label expected(applied|refused) -- cli args...
step() {
  local label="$1" expect="$2"; shift 3
  local line verdict block tx
  line="$("$CLI" --label "$label" "$@" 2>/tmp/antumbra-liq-e2e.err | grep '^{' | tail -1)"
  [ -n "$line" ] || line="{\"verdict\":\"error\",\"error\":\"$(tail -1 /tmp/antumbra-liq-e2e.err | tr '"' "'")\"}"
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
until_clock() { while [ "$(now)" -lt "$1" ]; do sleep 5; done; }
# Amounts in base units, exactly: `w 9.8` is 9.8 WAD.
w() { python3 -c "from fractions import Fraction as F;print(int(F('$1') * 10**18))"; }
pct() { python3 -c "print(int(10**18 * $1 // 100))"; }
lt() { python3 -c "print(int('$1') < int('$2'))"; }
gt() { python3 -c "print(int('$1') > int('$2'))"; }

OWNER=$(new_pub); K1=$(new_pub); K2=$(new_pub); K3=$(new_pub); BIDDER=$(new_pub)
ADMIN="$PAYER"
echo "# payer $PAYER admin $ADMIN owner $OWNER keeper1 $K1 keeper2 $K2 keeper3 $K3 bidder $BIDDER" >> "$OUT"
check "three liquidators, three different accounts" "$(printf '%s\n' "$K1" "$K2" "$K3" | sort -u | wc -l | tr -d ' ')" 3
P=(--payer "$PAYER")
# A gas limit for every transaction (each reserves gas_limit x base fee).
[ -n "${GAS_LIMIT:-}" ] && P+=(--gas-limit "$GAS_LIMIT")
FUND_EACH="${FUND_EACH:-0}"
if [ "$FUND_EACH" -gt 0 ]; then
  for a in "$OWNER" "$K1" "$K2" "$K3" "$BIDDER"; do
    "$W" auth-transfer send --from "Public/$PAYER" --to "Public/$a" --amount "$FUND_EACH" 2>&1 | grep -E "hash" | sed "s/^/  fund $a: /"
  done
fi
PAYER_BEFORE=$(balance "$PAYER")
echo "# payer balance before $PAYER_BEFORE" >> "$OUT"
FINAL=()
pin() { FINAL+=("$1 $ANTUMBRA_PROGRAM"); }

CF="$TAG-cfg"; TA="$TAG-type-a"; TB="$TAG-type-b"
P1="$TAG-pos-1"; P2="$TAG-pos-2"; P3="$TAG-pos-3"; P4="$TAG-pos-4"; P5="$TAG-pos-5"
MIN=$(python3 -c 'print(10**18 - 10**18 // 50)'); MAX=$(python3 -c 'print(10**18 - 10**18 // 5)')
RATIO=$(pct 150); PENALTY=$(pct 113); TIP=$(pct 2); BEG=$(pct 3)
laid() { jget "$(ids --position-id "$1")" liquidation_auction_id; }
pos() { jget "$("$CLI" show-position "$1")" "$2"; }
twap_a() { jget "$("$CLI" show-ctype "$TA")" twap_at_clock; }

echo "== 1. a config, two collateral types with their own feeds, debt auction terms, five positions"
step init-config applied -- "${P[@]}" init-config --config-id "$CF" --min-multiplier "$MIN" --max-multiplier "$MAX" --window 120 --admin "$ADMIN"
step init-type-a applied -- "${P[@]}" init-ctype --ctype-id "$TA" --config-id "$CF" --liquidation-ratio "$RATIO" --penalty "$PENALTY" --tip "$TIP" --max-age-ms "$MAX_AGE_A" --admin "$ADMIN"
step init-type-b applied -- "${P[@]}" init-ctype --ctype-id "$TB" --config-id "$CF" --liquidation-ratio "$RATIO" --penalty "$PENALTY" --tip "$TIP" --max-age-ms 3600000 --admin "$ADMIN"
step init-type-by-non-admin refused -- "${P[@]}" --force init-ctype --ctype-id "$TAG-type-x" --config-id "$CF" --liquidation-ratio "$RATIO" --penalty "$PENALTY" --tip "$TIP" --max-age-ms 1 --admin "$OWNER"
step init-debt-terms applied -- "${P[@]}" init-debt-config --config-id "$CF" --protocol-token mock-protocol-token --initial-lot "$(w 1000)" --beg "$BEG" --bid-ttl-ms "$BID_TTL" --auction-ttl-ms 1800000 --admin "$ADMIN"
for spec in "$P1 $TA 10 12000" "$P2 $TA 10 12500" "$P4 $TA 10 12000" "$P3 $TB 100 6000" "$P5 $TB 100 6000"; do
  set -- $spec
  step "open-${1#$TAG-}" applied -- "${P[@]}" open-position --position-id "$1" --ctype-id "$2" --collateral "$(w "$3")" --debt "$(w "$4")" --owner "$OWNER"
done
step price-a-2000 applied -- "${P[@]}" update-price --ctype-id "$TA" --price "$(w 2000)" --admin "$ADMIN"
step price-b-100 applied -- "${P[@]}" update-price --ctype-id "$TB" --price "$(w 100)" --admin "$ADMIN"
step price-by-non-admin refused -- "${P[@]}" --force update-price --ctype-id "$TA" --price "$(w 1)" --admin "$OWNER"

echo "== 2. F1: the price falls, and three different signers liquidate"
step liquidate-safe refused -- "${P[@]}" --force liquidate --position-id "$P1" --liquidator "$K1"
check "the safe position is untouched" "$(pos "$P1" liquidated)" 0
step price-a-1000 applied -- "${P[@]}" update-price --ctype-id "$TA" --price "$(w 1000)" --admin "$ADMIN"
step price-b-50 applied -- "${P[@]}" update-price --ctype-id "$TB" --price "$(w 50)" --admin "$ADMIN"
# The new print weighs nothing when it lands; wait until A's time-weighted
# price is below 1650, where 10 units owing 12000 are unsafe at 150% and a
# lot of 9.8 at the deepest discount (0.8) cannot raise 12000 plus 13%.
for _ in $(seq 120); do [ "$(lt "$(twap_a)" "$(w 1650)")" = True ] && break; sleep 5; done
echo "# A's time-weighted price before liquidating: $(twap_a)" >> "$OUT"
step liquidate-p1-by-keeper1 applied -- "${P[@]}" liquidate --position-id "$P1" --liquidator "$K1"
step liquidate-p1-again-by-keeper2 refused -- "${P[@]}" --force liquidate --position-id "$P1" --liquidator "$K2"
step liquidate-p2-by-keeper2 applied -- "${P[@]}" liquidate --position-id "$P2" --liquidator "$K2"
step liquidate-p3-by-keeper3 applied -- "${P[@]}" liquidate --position-id "$P3" --liquidator "$K3"
check "p1 liquidated by keeper 1" "$(pos "$P1" liquidator)" "$K1"
check "p2 liquidated by keeper 2" "$(pos "$P2" liquidator)" "$K2"
check "p3 (type B) liquidated by keeper 3" "$(pos "$P3" liquidator)" "$K3"
check "the keeper's tip is 2% of the collateral" "$(pos "$P1" tip)" "$(w 0.2)"
A1=$(laid "$P1"); A2=$(laid "$P2"); A3=$(laid "$P3"); A4=$(laid "$P4"); A5=$(laid "$P5")
check "p1's auction raises debt plus 13%" "$(jget "$("$CLI" show "$A1")" to_raise)" "$(w 13560)"
check "p1's auction sells the lot less the tip" "$(jget "$("$CLI" show "$A1")" collateral_seized)" "$(w 9.8)"

echo "== 3. F4: p1's lot runs out short of its target; the shortfall goes to a debt auction"
OPENED=$(jget "$("$CLI" show "$A1")" opened_at)
until_clock $((OPENED + 120000))
step buy-out-p1 applied -- "${P[@]}" buy --auction-id "$A1" --bid "$(w 20000)" --at "$((OPENED + 120000))" --bidder "$BIDDER"
S1="$("$CLI" show "$A1")"
check "p1's lot is gone" "$(jget "$S1" collateral_left)" 0
check "sold + left == seized (R6)" "$(jget "$S1" r6_sold_plus_left_is_seized)" True
check "p1's auction closed itself" "$(jget "$S1" settled)" 1
SHORT=$(jget "$S1" shortfall)
check "it closed short of debt plus penalty" "$(gt "$SHORT" 0)" True
step book-wrong-amount refused -- "${P[@]}" --force book-shortfall --auction-id "$A1" --caller "$K2" --shortfall "$(python3 -c "print(int('$SHORT') - 1)")"
step book-shortfall-by-keeper2 applied -- "${P[@]}" book-shortfall --auction-id "$A1" --caller "$K2"
step book-again-by-keeper3 refused -- "${P[@]}" --force book-shortfall --auction-id "$A1" --caller "$K3"
D="$("$CLI" show-debt "$A1")"
check "the debt auction covers exactly the shortfall" "$(jget "$D" coin)" "$SHORT"
check "it offers the configured lot of the protocol token" "$(jget "$D" lot)" "$(w 1000)"
step settle-debt-no-bid refused -- "${P[@]}" --force settle-debt --auction-id "$A1" --caller "$K3"
step debt-bid-not-3pct refused -- "${P[@]}" --force debt-bid --auction-id "$A1" --lot "$(w 971)" --bidder "$K1"
step debt-bid-keeper1-970 applied -- "${P[@]}" debt-bid --auction-id "$A1" --lot "$(w 970)" --bidder "$K1"
step debt-bid-unchanged refused -- "${P[@]}" --force debt-bid --auction-id "$A1" --lot "$(w 970)" --bidder "$K2"
step debt-bid-keeper2-900 applied -- "${P[@]}" debt-bid --auction-id "$A1" --lot "$(w 900)" --bidder "$K2"
step settle-debt-early refused -- "${P[@]}" --force settle-debt --auction-id "$A1" --caller "$K3"
until_clock "$(jget "$("$CLI" show-debt "$A1")" bid_expiry)"
step settle-debt applied -- "${P[@]}" settle-debt --auction-id "$A1" --caller "$K3"
D="$("$CLI" show-debt "$A1")"
check "the debt auction settled" "$(jget "$D" settled)" 1
check "keeper 2 won it" "$(jget "$D" bidder)" "$K2"
check "for 900 of the protocol token" "$(jget "$D" lot)" "$(w 900)"

echo "== 4. R2, R4, R5: A's feed goes stale, B's does not"
LAST_A=$(python3 -c 'import json,sys;print(json.loads(sys.argv[1])["observations"][-1]["at"])' "$("$CLI" show-ctype "$TA")")
until_clock $((LAST_A + MAX_AGE_A - 120000 + 5000))
step price-b-refresh applied -- "${P[@]}" update-price --ctype-id "$TB" --price "$(w 50)" --admin "$ADMIN"
check "A's feed is stale" "$(jget "$("$CLI" show-ctype "$TA")" stale_at_clock)" True
check "B's feed is fresh" "$(jget "$("$CLI" show-ctype "$TB")" stale_at_clock)" False
step liquidate-p4-stale-type-a refused -- "${P[@]}" --force liquidate --position-id "$P4" --liquidator "$K1"
check "p4 is untouched while A is paused" "$(pos "$P4" liquidated)" 0
step liquidate-p5-type-b-by-keeper2 applied -- "${P[@]}" liquidate --position-id "$P5" --liquidator "$K2"
step bid-on-type-a-auction applied -- "${P[@]}" buy --auction-id "$A2" --bid "$(w 1000)" --bidder "$BIDDER"
check "type A's auction kept taking bids" "$(gt "$(jget "$("$CLI" show "$A2")" raised)" 0)" True
step price-a-resumes applied -- "${P[@]}" update-price --ctype-id "$TA" --price "$(w 1000)" --admin "$ADMIN"
step liquidate-p4-by-keeper3 applied -- "${P[@]}" liquidate --position-id "$P4" --liquidator "$K3"
check "p4 liquidated once A's feed resumed" "$(pos "$P4" liquidator)" "$K3"

PAYER_AFTER=$(balance "$PAYER")
echo "# payer balance after $PAYER_AFTER" >> "$OUT"
echo "# payer spent $(python3 -c "print($PAYER_BEFORE - $PAYER_AFTER)") over $(grep -cvE '^#' "$OUT") transactions" >> "$OUT"

for x in "ctype_id $TA" "ctype_id $TB"; do set -- $x; pin "$(jget "$(ids --ctype-id "$2")" ctype)"; done
for p in "$P1" "$P2" "$P3" "$P4" "$P5"; do
  J="$(ids --position-id "$p")"; pin "$(jget "$J" position)"; pin "$(jget "$J" liquidation_auction)"
done
pin "$(jget "$(ids --position-id "$P1")" debt_auction)"
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
