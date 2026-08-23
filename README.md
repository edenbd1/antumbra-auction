# antumbra-auction

Increasing-discount collateral auction arithmetic for the Logos Execution Zone,
in integers that exist on the target.

```
cargo test --release
```

19 tests, no dependencies, `#![forbid(unsafe_code)]`, `overflow-checks = true`
in release — the profile that ships, because a debug run exercises different
arithmetic from the one that executes.

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
whose differential vectors and zkVM cycle counts already run in CI in the
sibling crate.

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

## What this is not, yet

This is the arithmetic, not the program. There is no SPEL program, no deployed
instance, no host CDP interface, no debt or surplus auction, and no price
source. The auction house is the piece that can be built and proven before any
of RFP-014's platform dependencies land, which is why it is the piece that
exists first.

## Licence

MIT OR Apache-2.0, at your option:
[`LICENSE-MIT`](LICENSE-MIT) and [`LICENSE-APACHE`](LICENSE-APACHE).
