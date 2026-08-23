#!/usr/bin/env bash
# Deploy the auction guest to the public LEZ testnet, and prove it landed.
#
#   WALLET=/path/to/lez/wallet ./scripts/deploy.sh
#
# The deploy transaction has no signer and no nonce, so its hash is a pure
# content hash: SHA256(u32_le(len) ‖ bytecode). That is computed *before*
# submitting, which is the whole verification strategy — the wallet's exit code
# and its output are both unreliable, so the only thing consulted afterwards is
# whether that hash resolves over RPC.
#
# Deploying is permissionless: a ProgramDeploymentTransaction carries only the
# bytecode, so no account, no funding and no authority are involved. It is also
# idempotent — redeploying identical bytes reproduces the same hash, and this
# script short-circuits on that rather than submitting again.
set -uo pipefail
cd "$(dirname "$0")/.."

BIN="${1:-artifacts/programs/antumbra_auction.bin}"
RPC="${SEQUENCER_URL:-https://testnet.lez.logos.co}"

if [ -z "${WALLET:-}" ]; then
  cat >&2 <<'EOF'
WALLET is not set. It must point at a LEZ wallet binary built from v0.2.4.

Version matters more than it looks: a v0.2.0 wallet reads this chain fine and
everything it submits is silently dropped — transactions come back with a hash
whose getTransaction is null, and balances never move. The two builds are told
apart by the environment variable they name: the old one says
NSSA_WALLET_HOME_DIR, the one that works says LEE_WALLET_HOME_DIR.
EOF
  exit 2
fi
[ -f "$BIN" ] || { echo "no such artifact: $BIN" >&2; exit 2; }

export LEE_WALLET_HOME_DIR="${LEE_WALLET_HOME_DIR:-$HOME/.lez-wallet}"

hash_of() {
  python3 -c "
import hashlib, struct, sys
b = open(sys.argv[1], 'rb').read()
print(hashlib.sha256(struct.pack('<I', len(b)) + b).hexdigest())" "$1"
}

# Test that the result is not null — never its *shape*. This node returns
# [transaction, block]; an older one returned a bare string, and a check written
# against either shape reads a landed deploy as dead.
landed() {
  curl -s -m 25 -X POST "$RPC" -H 'Content-Type: application/json' \
    -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"getTransaction\",\"params\":[\"$1\"]}" \
  | grep -q '"result":\['
}

TX=$(hash_of "$BIN")
echo "artifact   $BIN ($(wc -c < "$BIN" | tr -d ' ') bytes)"
echo "deploy tx  $TX"

if landed "$TX"; then
  echo "already on chain — identical bytes were deployed before, so nothing to do"
  exit 0
fi

for attempt in 1 2 3; do
  echo "submitting (attempt $attempt)…"
  # stdin is pinned because the wallet has been seen to abort while reading it,
  # and the framework path is set on the wallet's own exec because macOS strips
  # DYLD_* variables when bash execs another program — exporting it in a caller
  # never arrives.
  env DYLD_FALLBACK_FRAMEWORK_PATH=/Library/Developer/CommandLineTools/Library/Frameworks \
      "$WALLET" deploy-program "$BIN" </dev/null >/dev/null 2>&1
  for _ in $(seq 1 12); do
    sleep 5
    if landed "$TX"; then
      echo "on chain: $TX"
      echo
      echo "ImageID — the id the program is called by, a different digest entirely:"
      spel program-id "$BIN" 2>/dev/null | sed 's/^/  /'
      exit 0
    fi
  done
done

echo "not on chain after three attempts. The wallet's own output says nothing" >&2
echo "useful either way, so check the signer and the wallet version before the" >&2
echo "bytecode: $TX" >&2
exit 1
