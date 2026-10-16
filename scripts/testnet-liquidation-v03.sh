#!/usr/bin/env bash
# One command: deploy the liquidation build of antumbra_auction on the public
# LEZ testnet v0.3 and drive scripts/e2e-liquidation-v03.sh there, then
# re-check the manifest with scripts/verify-onchain.sh.
#
#   PAYER=<funded public account> ./scripts/testnet-liquidation-v03.sh
#   DRY_RUN=1 PAYER=<...> ./scripts/testnet-liquidation-v03.sh      # preflight only, sends nothing
#
# The build differs from the one at ADLHPytq… (ImageID dc3618e8…), so this is a
# new deployment at a new header; the earlier one and its evidence stay valid.
#
# Preflight, before anything is sent:
# - the committed binary is the one the scenario was measured with (ImageID);
# - PAYER is not an account another job holds (BUSY_ACCOUNT, until BUSY_UNTIL);
# - PAYER's balance covers the measured cost of the run (docs/cycles-v03.txt,
#   evidence/v03/liquidation/README.md) plus FUND_EACH for each of the five
#   fresh signers (0 by default), with a 25% margin, and one reserve.
#   TOPUP_FROM="<id> <id> ..." moves the shortfall from other public accounts of
#   the same wallet (an auth-transfer per account, leaving FEE_RESERVE in each).
set -uo pipefail
cd "$(dirname "$0")/.."

export LEE_WALLET_HOME_DIR="${LEE_WALLET_HOME_DIR:-$HOME/logos-bedrock/lez-wallet-home}"
export RPC="${RPC:-https://testnet.lez.logos.co}"
: "${PAYER:?PAYER=<base58 id of a funded public account in $LEE_WALLET_HOME_DIR>}"
W="${WALLET:-$HOME/logos-bedrock/bin/lez-wallet}"
[ -x "$W" ] || W=wallet
export WALLET="$W"
CLI="${CLI:-cli/target/release/antumbra-auction}"
IMAGE_ID=72c22843aa2c128f2e580ed7d96a478e6184b9d734dd09abe29199c1c4c1a433
BUSY_ACCOUNT="${BUSY_ACCOUNT:-9nqn7xA7z54QZPHs8wWn5VzTeXTgbwq8SREr38tVK9aw}"
BUSY_UNTIL="${BUSY_UNTIL:-2026-10-17 08:00}"
# Measured on the local v0.3.1 sequencer at base fee 8/8, the testnet's on
# 2026-10-16 (evidence/v03/liquidation/local-e2e.tsv): the deploy, and the
# scenario's 37 transactions at GAS_LIMIT 150000. A refused transaction forfeits
# its whole reserve, so the 11 deliberate refusals are about 1.2M each.
DEPLOY_COST="${DEPLOY_COST:-3688992}"
RUN_COST="${RUN_COST:-23626728}"
# Every public transaction reserves gas_limit x base_fee_exec up front.
GAS_LIMIT="${GAS_LIMIT:-150000}"
# PAYER pays every fee, so the fresh signers need no balance: the local run
# signs with five unfunded accounts. Set FUND_EACH if a chain ever requires it.
FUND_EACH="${FUND_EACH:-0}"
FEE_RESERVE="${FEE_RESERVE:-500000}"
OUTDIR="${OUTDIR:-evidence/v03/liquidation}"
TAG="${TAG:-liqtn$(date -u +%m%d%H%M)}"

fail() { echo "preflight: $*" >&2; exit 2; }
balance() {
  curl -s -m 20 -X POST "$RPC" -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getAccount\",\"params\":[\"$1\"]}" |
    python3 -c 'import json,sys;r=json.load(sys.stdin).get("result") or {};print(int.from_bytes(bytes((r.get("data") or {}).get("shards",{}).get("11111111111111111111111111111111",[])),"little"))'
}

[ "$("$CLI" image-id)" = "$IMAGE_ID" ] || fail "the committed binary is not ImageID $IMAGE_ID; rebuild with scripts/build-guest.sh"
if [ "$PAYER" = "$BUSY_ACCOUNT" ] && [ "$(date '+%Y-%m-%d %H:%M')" \< "$BUSY_UNTIL" ]; then
  fail "$PAYER is held by another job until $BUSY_UNTIL local; pick another PAYER"
fi
# Captured first: `grep -q` on a pipe exits early, and under pipefail the
# wallet's SIGPIPE would read as "not found".
ACCOUNTS="$("$W" account list 2>/dev/null)"
case "$ACCOUNTS" in *"Public/$PAYER"*) ;; *) fail "$PAYER is not a public account of $LEE_WALLET_HOME_DIR" ;; esac

NEED=$(python3 -c "print(int(($DEPLOY_COST + $RUN_COST + 5 * $FUND_EACH) * 1.25) + $GAS_LIMIT * 8)")
HAVE=$(balance "$PAYER")
echo "payer $PAYER holds $HAVE; the run needs $NEED (deploy $DEPLOY_COST, scenario $RUN_COST, 5 x $FUND_EACH to signers, 25% margin, one reserve of $GAS_LIMIT x 8)"
if [ "$HAVE" -lt "$NEED" ]; then
  [ -n "${TOPUP_FROM:-}" ] || fail "short by $((NEED - HAVE)); set TOPUP_FROM to public accounts of this wallet to move it from"
  for src in $TOPUP_FROM; do
    [ "$src" = "$BUSY_ACCOUNT" ] && [ "$(date '+%Y-%m-%d %H:%M')" \< "$BUSY_UNTIL" ] && continue
    [ "$HAVE" -ge "$NEED" ] && break
    b=$(balance "$src"); amt=$(python3 -c "print(max(0, min($b - $FEE_RESERVE, $NEED - $HAVE)))")
    [ "$amt" -gt 0 ] || continue
    echo "top-up: $amt from $src"
    HAVE=$((HAVE + amt))
    [ "${DRY_RUN:-0}" = 1 ] && continue
    "$W" auth-transfer send --from "Public/$src" --to "Public/$PAYER" --amount "$amt" 2>&1 | grep -iE "hash|error" | sed 's/^/  /'
  done
  [ "${DRY_RUN:-0}" = 1 ] || { for _ in $(seq 60); do [ "$(balance "$PAYER")" -ge "$NEED" ] && break; sleep 10; done; }
  [ "${DRY_RUN:-0}" = 1 ] || [ "$(balance "$PAYER")" -ge "$NEED" ] || fail "the top-up did not reach $NEED"
fi
[ "${DRY_RUN:-0}" = 1 ] && { echo "DRY_RUN: preflight passed, nothing sent"; exit 0; }

mkdir -p "$OUTDIR"
DEPLOY=1 PAYER="$PAYER" FUND_EACH="$FUND_EACH" GAS_LIMIT="$GAS_LIMIT" TAG="$TAG" \
  MAX_AGE_A="${MAX_AGE_A:-600000}" BID_TTL="${BID_TTL:-300000}" OUT="$OUTDIR/testnet.tsv" \
  ./scripts/e2e-liquidation-v03.sh 2>&1 | tee "$OUTDIR/testnet-transcript.txt"
./scripts/verify-onchain.sh --manifest "$OUTDIR/testnet.tsv" --rpc "$RPC" | tee "$OUTDIR/testnet-verify.txt"
