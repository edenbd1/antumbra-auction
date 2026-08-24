# antumbra-auction

Increasing-discount collateral auction arithmetic for the Logos Execution Zone,
in integers that exist on the target.

```
cargo test --release
```

41 tests green, no dependencies, `#![forbid(unsafe_code)]`, `overflow-checks = true`
in release — the profile that ships, because a debug run exercises different
arithmetic from the one that executes.

One of those tests is differential: `tests/gen_auction_vectors.py` computes the
quote a second time, in Python's arbitrary-precision integers, and 3,011 vectors
compare the two. What it covers is the composition — the discount multiplier,
both clamps, and the re-strike that fires when the lot rather than the target
runs out — because that is where a real defect lived. CI regenerates the vectors
and diffs them, so the file cannot be edited to match a broken implementation.

`zkvm/CYCLES.md` is measured, not asserted: the guest runs under the RISC0
executor and CI fails if a published figure has drifted. One whole bid — quote
then settle — is **27,753** cycles, 0.083 % of the session budget.

## What this found

[RFP-014](https://github.com/logos-co/rfp/blob/master/RFPs/RFP-014-liquidation-auction-engine.md)
names Reflexer's `IncreasingDiscountCollateralAuctionHouse` as the pattern to
follow and links
[the GEB source](https://github.com/reflexer-labs/geb). That code is Solidity,
so every intermediate is a `uint256` — 1.16e77. A SPEL program on LEZ computes
in `u128`, which stops at 3.4e38.

**Two of the reference's helpers overflow before any auction logic runs.**

### `rdivide` — the first step of pricing any bid

```solidity
function rdivide(uint256 x, uint256 y) internal pure returns (uint256 z) {
    z = multiply(x, RAY) / y;          // RAY = 1e27
}
```

The product is taken first. A collateral price quoted in WAD is about 1e18 for
a unit priced at 1, so `x · RAY` is around 1e45 — seven orders of magnitude past
`u128::MAX`.

The exact ceiling is `u128::MAX / RAY`, which is **3.4 × 10¹¹**. As a WAD price
that is 0.00000034. Every price from a third of a millionth upwards is out of
reach, so this is not an edge case reached by an extreme input: it is every
input.

### `rpower` — the compounding discount

```solidity
let xx := mul(x, x)                    // x ≈ RAY = 1e27
```

The discount grows per second at `perSecondDiscountUpdateRate`, compounded by
exponentiation-by-squaring in ray scale. The **first** squaring is 1e54 —
sixteen orders of magnitude past `u128::MAX`, before the exponent is consulted.

### It is executable, not asserted

`src/lib.rs` carries the transcription in a `reference` module, written the way
the Solidity writes it, returning `None` where a `uint256` would have carried
on. The tests exercise it at ordinary values:

| test | what it shows |
|---|---|
| `reference_rdivide_overflows_at_an_ordinary_collateral_price` | dies on a collateral priced at 2000 |
| `the_reference_overflows_for_any_price_above_a_third_of_a_millionth` | pins the exact ceiling |
| `reference_rpower_overflows_on_its_first_squaring` | dies at n = 2 |
| `reference_cannot_price_a_single_bid` | the whole chain, start to finish |
| `the_transcription_still_works_where_a_uint256_is_not_needed` | the transcription is faithful, not rigged |

That last one matters. A transcription that failed everywhere would prove
nothing about the width; this one computes correctly below the ceiling and only
fails above it.

## The fix is not a bigger integer

The quotient always fits — only the product does not. So the product is taken in
256 bits and divided back down in one step, and nothing intermediate is stored.
`mul_div_floor` and `mul_div_ceil` do that with a widening multiply and a
restoring division: no `u256` type, no dependency, and the same two functions
whose differential vectors and zkVM cycle counts already run in CI in
[`edenbd1/antumbra-lez`](https://github.com/edenbd1/antumbra-lez).

The discount schedule is linear in the multiplier rather than compounded per
second. Compounding is what forces `rpower`, and `rpower` is what does not fit.
The schedule is monotonic, reaches its maximum at the deadline and stays there,
and costs one `mul_div` instead of a loop whose length is the age of the
auction. A stepwise or exponential curve tabulates into the same shape; what
cannot be done is compounding a 1e27 rate in a 128-bit register.

## The direction of that quotient is not cosmetic

```
collateral_out = bid · WAD / discounted_price
```

This has to round **down**. Rounding up hands the bidder a fraction of a base
unit they did not pay for — and F3 requires partial fills, so the bidder
controls how many times it happens.

`ceil_lets_a_bidder_split_a_bid_and_extract_collateral` makes it concrete.
Collateral priced at 2000, no discount, so 2000 base units of coin buy exactly
one base unit:

| how the same 2000 units are spent | collateral received |
|---|---|
| one bid of 2000 | 1 |
| 2000 bids of 1, rounding down | 0 |
| 2000 bids of 1, **rounding up** | **2000** |

Splitting the bid multiplies the collateral by the number of fills. It is not
dust: `ceil_drains_the_whole_auction_if_bids_can_be_small_enough` runs it to
completion and the bidder takes everything that was seized for one
two-thousandth of its value.

`R6`'s invariant — *seized collateral equals collateral in auction* — is
asserted directly in `floor_keeps_the_invariant_across_a_long_sequence_of_partial_fills`,
over 600 awkward partial fills against an escalating discount.

## And the two defects this found in itself

Both came out of reading a live account on the testnet rather than out of a
design review, which is the argument for driving a thing rather than describing
it. Both are fixed, and the first build is still deployed so the evidence stays
fetchable.

**A bid larger than the lot was charged in full.** `quote` clamped the
collateral handed out to what the lot still held, and went on charging a bid
that had been priced against a larger lot. The bidder overpaid — and worse, the
auction recorded `raised == to_raise` and reported itself **fully raised** while
the collateral it actually sold was worth far less. A shortfall that is never
booked is one F4's debt auction never hears about. `quote` now re-strikes the
price when it is the lot that ran out: the bidder pays for the collateral they
receive, rounded towards the auction, capped at the bid it replaces.
`a_bid_bigger_than_the_lot_pays_only_for_the_lot` and
`the_restrike_never_charges_more_than_the_bid_it_replaces` both fail without it.

**And the on-chain R6 assertion was a tautology.** The guest checked
`left + (seized - left) != seized`, which is an identity — true for every value
of every field, including the ones a leak would produce. It could never have
fired. `AuctionState` now carries `collateral_sold`, moved independently of
`collateral_left`, and the guest compares the two. That also means anyone
holding the account can check R6 without trusting the assertion:
`collateral_sold + collateral_left == collateral_seized`.

## The program, on the public testnet

The auction house is deployed and driven on the public Logos Execution Zone
testnet. Two digests, and they are not the same number:

| | |
|---|---|
| **ImageID** | `38ab5886d20784c8322d1c6a3595cc8a666568946acb63248bac8ecf1149bd57` |
| **Deploy transaction** | `ac43ac1a8833f87f623b3346be551ece27da0e3a0c049de3ed86ae4b9607a75b` |
| **Block** | 20377 |

The first build is at ImageID `5dc0e088…` and deploy `c00b9698…`, block 20265.
It is left deployed on purpose: one of its accounts holds the output of the
defect described above, and a finding you can fetch beats a finding you
describe.

The ImageID is what the program is *called* by — RISC0 derives it from the guest
ELF, and it is what a driven transaction carries as its `program_id`. The deploy
transaction hash is `SHA256(u32_le(len) ‖ bytecode)` over the packaged binary,
which is a different thing entirely.

That second one is knowable **before** submitting: a deployment carries only the
bytecode, with no signer and no nonce, so it is a pure content hash. Which is the
whole verification strategy — compute it, deploy, then ask the sequencer whether
that hash resolves. The wallet's exit code and its output say nothing useful
either way.

```bash
WALLET=/path/to/lez/wallet ./scripts/deploy.sh   # deploy
./scripts/verify-onchain.sh                      # re-check everything, no setup
```

The script counts its own checks rather than stating a number here, because
this sentence has already been wrong twice. Some of them are things that must
**not** resolve — refused instructions, a hash never deployed, an address nobody
wrote — and they are what make the rest mean anything. The whole lifecycle, the
refusals and the timings are in [`DEPLOYMENTS.md`](DEPLOYMENTS.md).

Deployment is permissionless and idempotent: identical bytes reproduce the same
hash, and the script short-circuits rather than submitting again.

### What the program does, and what it leaves to the host

Four instructions: `init_config`, `start_auction`, `buy_collateral`,
`settle_auction`. Two account types: `Config` and `AuctionState`. The IDL is
generated from the program source by `spel generate-idl` rather than maintained
by hand — a field added to an account and forgotten in the IDL decodes every
later field at the wrong offset, and prints confident nonsense rather than
failing.

It is the auction house — the state machine and the arithmetic over collateral
it has been *told* was seized, at a price it was *handed*. It is not the
liquidation trigger, which needs a host CDP with positions, and it does not move
tokens yet, which needs a payer and a pinned transfer program. Both are the next
instruction rather than this one, and the README says so rather than letting a
reader infer otherwise.

### One deployer, one config, because the id is the code

On LEZ a program's id **is** the ImageID of its guest, so two deployers of
byte-identical code get the same program id. RFP-014's *"Each product deploys its
own configured instance of the program"* therefore cannot mean a separate
deployment unless the code differs.

It means a distinct config account under one shared program, which is what
`init_config` creates. That is also the better reading: it is what makes the
RFP's own stated rationale — shared audits, shared liquidator tooling — actually
true rather than aspirational.

### Time is an argument, so it is guarded

`now` is supplied by the caller. A caller who can rewind it can re-price a fill
at an earlier, smaller discount, so every instruction that reads the clock
refuses a `now` below one already honoured, and `AuctionState` carries
`last_seen` for that alone.

## What this is not, yet

The auction house is deployed and driven. What is **not** here, and is not
claimed to be:

- **The liquidation trigger.** `src/liquidation.rs` decides health and the
  liquidator's reward, and both are tested, but nothing on chain seizes a
  position — that needs a host CDP with positions in it.
- **Value movement.** The program accounts for collateral and coin; it does not
  yet debit a payer or credit a bidder. That needs a payer account and a pinned
  transfer program, and it is the next instruction rather than this one.
- **The debt and surplus auctions on chain.** `src/settlement.rs` implements
  both and the spec licenses a mock token for them, but they are not yet
  instructions.
- **A price source.** `Price` carries an observation time and staleness pauses
  one collateral type, which is what RFP-014 actually owns. Where the number
  comes from is RFP-019's or RFP-020's problem, and neither has delivered a
  milestone.

Each of those is a stand-in away rather than a wall, and the distinction is
worth keeping: a stand-in written against a published interface is a conformance
test of that interface, and a claim that a real counterparty will behave as
assumed is not.

## Licence

MIT OR Apache-2.0, at your option:
[`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE).
