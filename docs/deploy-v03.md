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
ImageID `72c22843aa2c128f2e580ed7d96a478e6184b9d734dd09abe29199c1c4c1a433`: the
auction house plus liquidation, the feed and the debt auction (below). A clean
clone rebuilds it byte for byte, and the `LEZ v0.3` workflow checks it.

The binary deployed on the public testnet on 2026-10-15, the auction house
alone, is kept byte for byte at
`artifacts/programs/v0.3/deployed-dc3618e8/antumbra_auction.bin`, ImageID
`dc3618e80b2645906b70c5aee9c6e1be50d985fc00f031c3dc03e0ad9fd9384f`. The newer
binary only appends instructions (4 to 11), so every call the older one takes
encodes and behaves the same.

## Tests

```bash
cargo test --release --manifest-path programs/auction/Cargo.toml -p antumbra-auction-core   # 30 host tests
cargo test --release --manifest-path executor-tests/Cargo.toml                              # 18, the committed binary through lee::V03State
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

## The public testnet

Deployed on 2026-10-15 at header `ADLHPytqfLnzM2hfoJUx6ZXPLwFo6o4RmqXWRJGETMJ7` (ImageID `dc3618e8…`), and driven
through the whole lifecycle there: 8 applied, 12 refused as required, 0
failures, 26 checks ([`evidence/v03/testnet.tsv`](../evidence/v03/testnet.tsv)).
The first run that day failed only at the opening: the sequencer dropped it,
and the CLI resent it with the time it had first named, outside the two-minute
window an opening is valid in, so the program refused it, as it must. The CLI
now names a relative time (`now…`) afresh on every resend; that run's record is
kept in [`evidence/v03/testnet-run1/`](../evidence/v03/testnet-run1/). To run it again:

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

## Liquidation, the feed and the debt auction (F1, F4, R2, R4, R5)

Instructions 4 to 11, in `programs/auction/core/src/engine.rs`, appended to the
ones above so every earlier call encodes and behaves the same:

| Instruction | Signed by | What it does |
|---|---|---|
| `InitCollateralType` | the config's admin | liquidation ratio, penalty, keeper tip, the feed's maximum age, and the config's schedule (restated, checked as at `StartAuction`) |
| `UpdatePrice` | the config's admin | appends a price to the type's feed (the last 4 are kept); valid only in `[now, now + 2 min)` |
| `OpenPosition` | anyone | the reference host's vault: collateral and debt in one type, never gated on a feed (R5) |
| `Liquidate` | **anyone** (F1) | restates the type's terms, its time-weighted price at `now` and the position; seizes it, books the tip to the signer, opens the auction for `debt x penalty` over the rest of the lot; valid only in `[now, now + 2 min)` |
| `InitDebtConfig` | the config's admin | the debt auction's protocol token address, opening lot, minimum improvement, bid and auction durations |
| `BookShortfall` | anyone | an auction that closed with its lot gone and its target unmet opens a debt auction for exactly the shortfall, once (F4) |
| `DebtBid` | anyone | a protocol token lot smaller than the standing one by at least 3% and one base unit; valid only in `[now, now + 2 min)`, so it cannot be back-dated under an expiry that has passed |
| `SettleDebtAuction` | anyone | once the standing bid has expired |

The price a liquidation uses is the **time-weighted** average of the feed up to
`now` (R2): each print weighs for as long as it stood, so one print alone moves
nothing at the moment it lands. A feed is **stale** for a liquidation if it
would be older than its maximum age at the end of the transaction's validity
window, so back-dating `now` cannot make a stale feed look fresh; the pause can
come early by up to two minutes, never late. A stale feed refuses liquidation
of its own type only (R4, code 8015); bids and settlements never read a feed
(R5). Codes 8015 to 8023 are new; each fires in `executor-tests/tests/engine.rs`
and in the local run (`evidence/v03/liquidation/local-refusals.txt`).

The program still moves no tokens: the tip, the auction's proceeds and the
debt auction's lot are booked, not transferred, until a system coin and a
protocol token are pinned.

### On the public testnet

The build is a new ImageID (`72c22843…`), so it is a **new deployment** at a
new header; `ADLHPytq…` and its evidence stay as they are. Every refused public
transaction forfeits its whole reserve, `gas_limit x base fee`, so the run sets
the gas limit to 150,000, just above `liquidate` (98,382 cycles). Measured on
the local chain at the testnet's base fees: 3,688,992 units for the deploy and
23,626,728 for the scenario's 37 transactions
(`evidence/v03/liquidation/README.md`). One command, with a preflight that
sends nothing until the payer covers that plus 25%:

```bash
cargo build --release --manifest-path cli/Cargo.toml
DRY_RUN=1 PAYER=<funded public account> ./scripts/testnet-liquidation-v03.sh   # preflight only
PAYER=<funded public account> [TOPUP_FROM="<id> <id> ..."] ./scripts/testnet-liquidation-v03.sh
```

It deploys, runs `scripts/e2e-liquidation-v03.sh` with a 10-minute feed age and
5-minute debt bids (the testnet includes some transactions minutes late), and
writes `testnet.tsv`, `testnet-transcript.txt` and `testnet-verify.txt` next to
the local evidence.
