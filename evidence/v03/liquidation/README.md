# Liquidation, the price feed and the debt auction: on the public testnet, and locally

## On the public testnet (2026-10-17)

The same binary (ImageID `72c22843…`) deployed immutable at header
`J3TXJsr7U31F8G9YEVFCebeeCjd7Ro7f8t4KU4eB3iMy` on `https://testnet.lez.logos.co`
(sequencer v0.3.1) by `scripts/testnet-liquidation-v03.sh`, and the same
scenario as the local run below, blocks 28,593 to 28,857.

**Result: 26 transactions applied, 11 refused exactly where they must be, 22
state checks read back from the chain, 0 failures; the verifier re-checks the
manifest at 52 checks, 0 failures.** The deploy cost 3,688,992 units and the
37 transactions 23,626,768 (40 more than locally). Two transactions were moved
by a block rewrite after the run saw them: `init-debt-terms`, which the run
re-read at its end (the `# moved` line in the manifest), and the refused
`book-wrong-amount`, which also lost its hash index (`getTransaction` returns null),
and the verifier finds it in block 28,718 by its hash, SHA-256 over the
transaction's bytes after its variant tag.

| File | What it is |
|---|---|
| `testnet-transcript.txt` | the run's output, one JSON line per transaction and one line per check |
| `testnet.tsv` | the manifest, as for the local run |
| `testnet-verify.txt` | `scripts/verify-onchain.sh --manifest evidence/v03/liquidation/testnet.tsv` |

## On a local sequencer (2026-10-16)

F1, F4, R2, R4 and R5 of RFP-014, on chain. A fresh local LEZ v0.3.1
standalone sequencer (the version the public testnet runs), the v0.3.0 wallet,
the committed `artifacts/programs/v0.3/antumbra_auction.bin` (ImageID
`72c22843aa2c128f2e580ed7d96a478e6184b9d734dd09abe29199c1c4c1a433`) deployed
through `program_loader` at header `Ez3z28CUaJDvFTvtSVceUPAaZWmFX7ymHQJJJmFu6fJQ`,
and `scripts/e2e-liquidation-v03.sh` driving it through the `antumbra-auction`
CLI, blocks 171 to 214, every transaction at a gas limit of 150,000 (the one
the testnet run uses).

**Result: 26 transactions applied, 11 refused exactly where they must be, each
with the code it must carry, 22 state checks read back from the chain, 0
failures; the verifier re-checks the manifest at 52 checks, 0 failures.**

| File | What it is |
|---|---|
| `local-transcript.txt` | the run's output, one JSON line per transaction and one line per check |
| `local-e2e.tsv` | the manifest: every transaction with its expectation, verdict, block and hash; the signers; the payer's balance around the deploy and the run; the final state of both collateral types, the five positions, their five auctions and the debt auction |
| `local-verify.txt` | `scripts/verify-onchain.sh --manifest evidence/v03/liquidation/local-e2e.tsv --rpc http://127.0.0.1:3046` |
| `local-refusals.txt` | each refused transaction and the refusal code the sequencer logged for it |

## What the run shows

One config (2% off at open to 20% at 120 s), two collateral types under it, A
and B, each with its own feed written by the config's admin; ratio 150%,
penalty 13%, keeper tip 2%; A's feed usable for 300 s, B's for an hour. The
config's debt auction terms: 1000 units of a mock protocol token (an address
the program never dereferences, as F4 allows), 3% minimum improvement, 60 s
for a standing bid. One owner opens five positions, safe at the opening
prices (A at 2000, B at 100).

**F1, permissionless liquidation.** Liquidating a safe position is refused
(8017). Both feeds fall (A to 1000, B to 50). The new print weighs nothing when
it lands; once A's time-weighted price is below 1650 (it was 1571.45), three
accounts made by the run a moment earlier, each a different key and none
holding any balance (the payer pays every fee), liquidate one position each:
keeper 1 position 1 (type A), keeper 2 position 2 (type A), keeper 3 position 3
(type B). The position records who liquidated it and the tip booked to them
(2% of the collateral); a second liquidation of position 1 is refused (8018).
Each liquidation opens its own auction for debt plus penalty (13,560 for a debt
of 12,000) over the lot less the tip (9.8 of 10 units), at the time-weighted
price.

**F4, the debt auction.** Position 1's lot is bought out at the deepest
discount and runs out 1,239.81 short of its target, so the auction closes
itself with `sold + left == seized` (R6). Anyone books the shortfall into a
debt auction: naming the wrong amount is refused (8020), booking it a second
time is refused (8011), and the debt auction covers exactly the shortfall.
Settling with no bid is refused (8023); a bid that improves the lot by less
than 3% is refused (8021), as is an unchanged one; keeper 1 bids 970 and keeper
2 900; settling before the standing bid expires is refused (8010); once it
expires the auction settles to keeper 2 for 900.

**R2, R4, R5, one stale feed.** A's feed is left alone while B's is refreshed.
Once A's age at the end of a transaction's validity window passes 300 s, a
liquidation of a type A position is refused (8015) and the position is
untouched, while a type B position is liquidated in the next block (R4), and a
bid on type A's open auction still lands (R5: bids and settlements never read a
feed). A's feed resumes and the paused position is liquidated.

## What it cost, and what a testnet run costs

Fees on this chain are the testnet's: base fee 8 for execution and 8 for
storage, read from the fee-state account on both on 2026-10-16. An applied
transaction pays the cycles it used; **a refused one forfeits its whole
reserve**, `gas_limit x 8` plus storage (measured: 2,406,744 at a gas limit of
300,000, 80,006,744 at 10,000,000, for the same refusal). So the run sets the
gas limit just above the heaviest instruction (`liquidate`, 98,382 cycles,
`docs/cycles-v03.txt`): 150,000.

| | units |
|---|---:|
| deploy (5 segments and the header) | 3,688,992 |
| the scenario, 37 transactions (11 deliberate refusals at about 1.21M each) | 23,626,728 |
| total | 27,315,720 |

`scripts/testnet-liquidation-v03.sh` asks for that plus 25% and one reserve
(35,344,650) before it sends anything.

## Reproduce

```bash
DEPLOY=1 GAS_LIMIT=150000 PAYER=6iArKUXxhUJqS7kCaPNhwMWt3ro71PDyBj7jwAyE2VQV RPC=http://127.0.0.1:3040 \
  OUT=evidence/v03/liquidation/local-e2e.tsv ./scripts/e2e-liquidation-v03.sh
./scripts/verify-onchain.sh --manifest evidence/v03/liquidation/local-e2e.tsv --rpc http://127.0.0.1:3040
```

with the sequencer and wallet set up as in [`docs/deploy-v03.md`](../../../docs/deploy-v03.md).
