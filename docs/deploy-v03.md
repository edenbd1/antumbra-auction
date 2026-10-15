# antumbra_auction on LEZ v0.3

The program is rewritten for LEZ v0.3 (`programs/auction`, core crate in
`programs/auction/core`), against `lee_core` at the v0.3.0 tag
(`db66590a`), since SPEL has no v0.3 support. The v0.2.4 SPEL guest is kept
unchanged in `programs/auction-v024`, with its testnet evidence and its checker
`scripts/verify-onchain-v024.sh`, as history.

## What v0.3 changes for this program

| v0.2.4 | v0.3 | Consequence here |
|---|---|---|
| One call reads the auction and its config | `plan` sees rows, never data; each `apply` sees one shard | The config's schedule is copied into the auction at `start_auction`, and the copy is checked: the instruction restates the schedule, one effect asks the config shard (which it never writes) whether that is what it holds and whether the signer is its admin. A config is never written after creation, so the copy cannot go stale; a bid then touches only the auction's shard |
| `init` refused an existing account | Nothing is refused unless a program refuses it | Both creations refuse a non-empty shard (8011) |
| Configs and auctions share a seed space (`[id]`) | PDAs derive from the program's header account | Seeds are `[id, "config"]` and `[id, "auction"]`, so a config id cannot shadow an auction id |
| `now` is a caller argument, guarded only by monotonicity | A plan bounds the block timestamp it is valid at | A bid or a settle at `now` is valid only once the block timestamp reaches `now`, so a fill cannot be priced at a discount the auction has not reached; an opening is valid only in `[now, now + 2 min)`. The monotonic guard stays. Times are milliseconds (the block timestamp's unit); the discount window stays in seconds |
| Errors 8001 to 8010 | Refusal by panic, code first in the message | Same codes and meanings; 8011 to 8014 are what v0.3 adds (exists, accounts, not signed, config mismatch) |
| No fees | Every public transaction names an LGO fee payer and is charged even when the program refuses it | The CLI names a payer and pre-checks a bid, a settle and an opening before sending; `--force` sends a refusal on purpose |

What does not change: the `antumbra-auction` arithmetic crate (the quote, both
clamps, the re-strike, rounding down), its 41 tests and its differential
vectors. The program still moves no tokens, as on v0.2.4: it is the auction
house over collateral it was told was seized.

`settle_auction` has no successful path, on v0.3 as on v0.2.4: the fill that
meets the target or empties the lot settles the auction in the same apply (F2),
and a separate settle then refuses with 8005. It is kept as the explicit,
refused transition F6 asks for.

## The binary

`artifacts/programs/v0.3/antumbra_auction.bin`, built with
`scripts/build-guest.sh` in RISC Zero's pinned Docker builder (`r0.1.91.1`).
ImageID `dc3618e80b2645906b70c5aee9c6e1be50d985fc00f031c3dc03e0ad9fd9384f`.
A clean clone rebuilds it byte for byte, and the `LEZ v0.3` workflow checks it.

## Tests

```bash
cargo test --release --manifest-path programs/auction/Cargo.toml -p antumbra-auction-core   # 15 host tests
cargo test --release --manifest-path executor-tests/Cargo.toml                              # 16, the committed binary through lee::V03State
cargo run  --release --manifest-path executor-tests/Cargo.toml --bin measure                # docs/cycles-v03.txt
```

The executor tests reproduce the v0.2.4 testnet run to the base unit on the
v0.3 state machine (the same bid buys `1020408163265306122` at open and
`1123595505617977528` half way through the window; a bid of 16000 is re-struck
to `12569.594129786746160` and the auction closes 3430.41 short), and fire every
reachable refusal code on chain. 8006 and 8008 are guards that no sequence of
instructions reaches: the fill that empties the lot or meets the target settles
the auction first, and a schedule and price that pass creation cannot fail the
quote.

## A local run

A LEZ v0.3.1 standalone sequencer (the version the public testnet runs) and
the v0.3.0 wallet, whose config and storage format the CLI's wallet crate
reads:

```bash
git clone https://github.com/logos-blockchain/logos-execution-zone lez && cd lez
git checkout db66590ab821a4e142c211017a3866d007f6fa77 && cargo build --release -p wallet
git checkout 1a003bfbfb89fcf987f62975cd08a401169e1b8c && cargo build --release --features standalone -p sequencer_service
mkdir -p /tmp/seq && cp lez/sequencer/service/configs/debug/sequencer_config.json /tmp/seq/
(cd /tmp/seq && ../path/to/sequencer_service sequencer_config.json --listen-address 127.0.0.1 &)   # port 3040

export LEE_WALLET_HOME_DIR=/tmp/wallet-local
wallet check-health        # creates wallet_config.json
sed -i.bak -e 's|https://testnet.lez.logos.co/|http://127.0.0.1:3040/|' -e 's/"gas_limit": 2000000/"gas_limit": 10000000/' \
  "$LEE_WALLET_HOME_DIR/wallet_config.json"
wallet account import public --private-key 10a26a9aec7d34b82364eeae45c5294dbb0a764b000b94eeb9b58511dc487c4d

cargo build --release --manifest-path cli/Cargo.toml
DEPLOY=1 PAYER=6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV RPC=http://127.0.0.1:3040 ./scripts/e2e-v03.sh
./scripts/verify-onchain.sh --manifest evidence/v03/local-e2e.tsv --rpc http://127.0.0.1:3040
```

The CLI carries v0.3.1's block format (`cli/vendor/lez-common`, the `common`
crate with `src/` from `1a003bfb`): v0.3.1 dropped the block header's producer
key and signature, which a v0.3.0 client cannot decode.

## The public testnet: still to do

The program has not been deployed on the public testnet v0.3 yet. When it is:

```bash
export LEE_WALLET_HOME_DIR=<a wallet home holding PAYER> RPC=https://testnet.lez.logos.co
cargo build --release --manifest-path cli/Cargo.toml
DEPLOY=1 PAYER=<funded account> FUND_EACH=10000000 TAG=testnet1 OUT=evidence/v03/testnet.tsv \
  ./scripts/e2e-v03.sh 2>&1 | tee evidence/v03/testnet-transcript.txt
./scripts/verify-onchain.sh > evidence/v03/testnet-verify.txt
```

`deploy` prints its plan (header and segment accounts) before sending; if the
sequencer drops a segment, `deploy --header <h> --segment <s>...` resumes and
skips what is already on chain. Every other transaction is sent again if it is
not included within five minutes, up to three attempts, and every attempt is
recorded in the manifest; `scripts/reresolve.py` re-reads each transaction's
block at the end of the run, since the testnet has rewritten recent blocks.
