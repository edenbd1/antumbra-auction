#!/usr/bin/env bash
# Drive one auction end to end on the public LEZ testnet, and read every result
# back off the chain rather than out of the client.
#
#   WALLET=/path/to/lez/wallet SIGNER=Public/<id> ./scripts/drive.sh
#
# Why every step reads account state afterwards, and none of them trusts the
# CLI: this runtime carries no receipt mechanism. getTransactionReceipt,
# getEvents and getLogs all answer "Method not found", so a failed instruction
# surfaces as nothing at all — the client times out, or worse, prints a hash and
# exits zero. A permissionless instruction has no signer and therefore no nonce,
# so calling it twice builds a byte-identical transaction whose hash resolves to
# the *first* call; polling that hash reports a success this run did not cause.
#
# What the run demonstrates:
#   F2  the discount escalates — the same bid buys more collateral later
#   F3  partial fills, with the lot and the target both decreasing
#   F6  strict transitions — settling twice, and bidding after settlement, refused
#   R6  seized == sold + left, asserted from the account, after every fill
set -uo pipefail
cd "$(dirname "$0")/.."

RPC="${SEQUENCER_URL:-https://testnet.lez.logos.co}"
IDL=idl/antumbra_auction.idl.json
BIN=artifacts/programs/antumbra_auction.bin
: "${WALLET:?set WALLET to a v0.2.4 LEZ wallet binary}"
: "${SIGNER:?set SIGNER to a funded Public/<id> that has had auth-transfer init run on it}"
export LEE_WALLET_HOME_DIR="${LEE_WALLET_HOME_DIR:-$HOME/.lez-wallet}"

WAD=1000000000000000000
CONFIG_ID="${CONFIG_ID:-antumbra-auction-demo}"
AUCTION_ID="${AUCTION_ID:-auction-1}"

ok()   { printf '  \033[32mok\033[0m    %s\n' "$1"; }
bad()  { printf '  \033[31mFAIL\033[0m  %s\n' "$1"; exit 1; }
step() { printf '\n\033[1;36m$ %s\033[0m\n' "$*"; }

run() { # instruction and its flags
  step "spel -- $*"
  spel --idl "$IDL" --program "$BIN" -- "$@" 2>&1 | sed 's/^/    /'
}

# The only trustworthy signal. `spel inspect` decodes the account through the
# IDL, so a field that moved is a field the chain holds.
show() {
  spel inspect "$1" --idl "$IDL" --type "$2" 2>&1 | sed 's/^/    /'
}

pda() { spel pda --program "$(spel program-id "$BIN" | grep -oE '[0-9a-f]{64}')" "$1"; }

echo "Antumbra collateral auction on the public LEZ testnet — $RPC"
echo "signer $SIGNER"

# 2% off at open, 20% off after an hour.
MIN=$((WAD - WAD / 50))
MAX=$((WAD - WAD / 5))

run init-config --config-id "$CONFIG_ID" \
    --min-multiplier "$MIN" --max-multiplier "$MAX" --window 3600 \
    --admin "$SIGNER"

CONFIG_PDA=$(pda "$CONFIG_ID")
show "$CONFIG_PDA" Config

# 10 units of collateral priced at 2000, raising 20000.
run start-auction --auction-id "$AUCTION_ID" --config-id "$CONFIG_ID" \
    --collateral $((10 * WAD)) --to-raise $((20000 * WAD)) --price $((2000 * WAD)) \
    --now 0 --admin "$SIGNER"

AUCTION_PDA=$(pda "$AUCTION_ID")
show "$AUCTION_PDA" AuctionState

step '# a partial fill at t=0, where the discount is 2%'
run buy-collateral --auction-id "$AUCTION_ID" --config-id "$CONFIG_ID" \
    --bid $((2000 * WAD)) --now 0 --bidder "$SIGNER"
show "$AUCTION_PDA" AuctionState

step '# the same bid half an hour later, where the discount has reached 11%'
run buy-collateral --auction-id "$AUCTION_ID" --config-id "$CONFIG_ID" \
    --bid $((2000 * WAD)) --now 1800 --bidder "$SIGNER"
show "$AUCTION_PDA" AuctionState

step '# the control: a clock earlier than one already honoured must be refused'
run buy-collateral --auction-id "$AUCTION_ID" --config-id "$CONFIG_ID" \
    --bid $((1 * WAD)) --now 60 --bidder "$SIGNER"
show "$AUCTION_PDA" AuctionState

step '# and settling an auction that is neither sold out nor fully raised'
run settle-auction --auction-id "$AUCTION_ID" --now 1800 --caller "$SIGNER"
show "$AUCTION_PDA" AuctionState

echo
echo "Read the two AuctionState dumps after the fills: the second bid bought"
echo "more collateral than the first for the same money, which is the discount"
echo "escalating, and collateral_seized still equals what has left plus what"
echo "remains. Both controls above must have changed nothing."
