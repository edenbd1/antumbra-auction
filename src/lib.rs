//! Increasing-discount collateral auction arithmetic for the Logos Execution
//! Zone, in integers that exist on the target.
//!
//! # What this found
//!
//! RFP-014 names Reflexer's `IncreasingDiscountCollateralAuctionHouse` as the
//! pattern to follow, and links the GEB source. That code is Solidity, so every
//! intermediate is a `uint256` — 1.16e77. A SPEL program on LEZ computes in
//! `u128`, which stops at 3.4e38, and the reference chain does not fit.
//!
//! Two of its helpers overflow before any auction logic runs:
//!
//! ```text
//! rdivide(x, y) = x · RAY / y          RAY = 1e27
//! ```
//!
//! The product is taken first. A collateral price quoted in WAD is around 1e18
//! for a unit priced at 1, so `x · RAY` is about 1e45 — seven orders of
//! magnitude past `u128::MAX`. The first step of pricing a bid overflows for
//! *any* realistic price, not for an extreme one.
//!
//! ```text
//! rpower(x, n, RAY): xx = x · x        x ≈ RAY = 1e27
//! ```
//!
//! The discount compounds per second by exponentiation-by-squaring. The very
//! first squaring is 1e54 — sixteen orders of magnitude past `u128::MAX`,
//! before the exponent is even consulted.
//!
//! Neither is a rounding problem or an edge case. A faithful transcription of
//! the reference cannot price a single bid on this platform. [`reference`]
//! contains that transcription so the claim is executable rather than asserted,
//! and the tests exercise it at ordinary values.
//!
//! # The fix, and why it is not a bigger integer
//!
//! The quotient always fits; only the product does not. So the product is taken
//! in 256 bits and divided back down in one step, and nothing intermediate is
//! ever stored. [`mul_div_floor`] and [`mul_div_ceil`] do that with a widening
//! multiply and a restoring division — no `u256` type, no dependency.
//!
//! # The direction of that quotient is not cosmetic
//!
//! `collateral_out = bid · WAD / price` has to round *down*. Rounding up hands
//! the bidder a fraction of a base unit they did not pay for, and F3 requires
//! partial fills, so the bidder controls how many times that happens. Splitting
//! one bid into a thousand becomes a thousand roundings in their favour, taken
//! out of collateral the auction seized from someone else. `R6`'s invariant —
//! *seized collateral equals collateral in auction* — fails by exactly the
//! number of fills. See `ceil_lets_a_bidder_split_a_bid_and_extract_collateral`.

#![forbid(unsafe_code)]

/// 1e18. Prices and discounts are quoted in this scale.
pub const WAD: u128 = 1_000_000_000_000_000_000;
/// 1e27. The reference's ray scale, kept here only to reproduce its overflow.
pub const RAY: u128 = 1_000_000_000_000_000_000_000_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuctionError {
    /// The quotient itself needs more than 128 bits.
    Overflow,
    DivideByZero,
    /// A discount outside `(0, WAD]`: zero would price collateral at infinity,
    /// above WAD would be a premium rather than a discount.
    DiscountOutOfRange,
    NothingLeftToSell,
}

type Result<T> = core::result::Result<T, AuctionError>;

// ---------------------------------------------------------------------------
// 256-bit intermediates
//
// Lifted from the sibling `antumbra-lez` crate, where the differential vectors
// and the zkVM cycle counts for these two functions already run in CI. The
// restoring division is one bit at a time on purpose: it is the version whose
// top-bit carry is handled, and the first draft that shifted blindly mispriced
// about half the vectors.
// ---------------------------------------------------------------------------

fn wide_mul(a: u128, b: u128) -> (u128, u128) {
    let (a_hi, a_lo) = (a >> 64, a & u64::MAX as u128);
    let (b_hi, b_lo) = (b >> 64, b & u64::MAX as u128);

    let lo_lo = a_lo * b_lo;
    let hi_lo = a_hi * b_lo;
    let lo_hi = a_lo * b_hi;
    let hi_hi = a_hi * b_hi;

    let mid = (lo_lo >> 64) + (hi_lo & u64::MAX as u128) + (lo_hi & u64::MAX as u128);
    let lo = (mid << 64) | (lo_lo & u64::MAX as u128);
    let hi = hi_hi + (hi_lo >> 64) + (lo_hi >> 64) + (mid >> 64);
    (hi, lo)
}

fn wide_div(hi: u128, lo: u128, d: u128) -> Result<(u128, u128)> {
    if d == 0 {
        return Err(AuctionError::DivideByZero);
    }
    if hi >= d {
        return Err(AuctionError::Overflow);
    }
    let mut rem: u128 = hi;
    let mut quo: u128 = 0;
    let mut i = 128;
    while i > 0 {
        i -= 1;
        let carried = rem >> 127;
        rem = (rem << 1) | ((lo >> i) & 1);
        quo <<= 1;
        if carried == 1 || rem >= d {
            rem = rem.wrapping_sub(d);
            quo |= 1;
        }
    }
    Ok((quo, rem))
}

/// `floor(a·b / d)`, with the product taken in 256 bits.
pub fn mul_div_floor(a: u128, b: u128, d: u128) -> Result<u128> {
    let (hi, lo) = wide_mul(a, b);
    Ok(wide_div(hi, lo, d)?.0)
}

/// `ceil(a·b / d)`, with the product taken in 256 bits.
pub fn mul_div_ceil(a: u128, b: u128, d: u128) -> Result<u128> {
    let (hi, lo) = wide_mul(a, b);
    let (q, r) = wide_div(hi, lo, d)?;
    if r == 0 {
        Ok(q)
    } else {
        q.checked_add(1).ok_or(AuctionError::Overflow)
    }
}

// ---------------------------------------------------------------------------
// The reference, transcribed
// ---------------------------------------------------------------------------

/// GEB's helpers, written as the Solidity writes them, in the width this
/// platform actually has.
///
/// Each returns `None` where the Solidity would have kept going in a `uint256`.
/// Nothing here is meant to be called by a program: it exists so the overflow
/// is a test result rather than a claim in a paragraph.
pub mod reference {
    use super::{RAY, WAD};

    /// `rdivide(x, y) = x · RAY / y` — the product first, as written.
    pub fn rdivide(x: u128, y: u128) -> Option<u128> {
        x.checked_mul(RAY)?.checked_div(y)
    }

    /// `wmultiply(x, y) = x · y / WAD` — the product first, as written.
    pub fn wmultiply(x: u128, y: u128) -> Option<u128> {
        x.checked_mul(y).map(|p| p / WAD)
    }

    /// `wdivide(x, y) = x · WAD / y` — the product first, as written.
    pub fn wdivide(x: u128, y: u128) -> Option<u128> {
        x.checked_mul(WAD)?.checked_div(y)
    }

    /// `rpower(x, n, RAY)` — exponentiation by squaring, ray-scaled.
    ///
    /// The reference reverts on overflow rather than wrapping, which is what
    /// `None` stands in for here.
    ///
    /// `n % 2` is kept rather than `is_multiple_of`, because the Solidity reads
    /// `mod(n, 2)` and the value of this module is that it can be diffed
    /// against the original line by line. Idiomatic Rust here would cost the
    /// only thing it is for.
    #[allow(clippy::manual_is_multiple_of)]
    pub fn rpower(x: u128, mut n: u64) -> Option<u128> {
        let mut z = if n % 2 == 0 { RAY } else { x };
        let half = RAY / 2;
        let mut x = x;
        n /= 2;
        while n != 0 {
            let xx = x.checked_mul(x)?; // <- 1e54 for x ≈ RAY
            x = xx.checked_add(half)? / RAY;
            if n % 2 != 0 {
                let zx = z.checked_mul(x)?;
                z = zx.checked_add(half)? / RAY;
            }
            n /= 2;
        }
        Some(z)
    }

    /// The reference's pricing chain for one bid, start to finish.
    ///
    /// `base_price` and `system_coin_price` are WAD, `discount` is WAD.
    pub fn bought_collateral(
        base_price: u128,
        system_coin_price: u128,
        discount: u128,
        bid: u128,
    ) -> Option<u128> {
        let ratio = rdivide(base_price, system_coin_price)?;
        let discounted = wmultiply(ratio, discount)?;
        wdivide(bid, discounted)
    }
}

// ---------------------------------------------------------------------------
// The auction
// ---------------------------------------------------------------------------

/// A discount that grows linearly from `min` to `max` across `window` seconds.
///
/// The reference compounds per second at a ray-scaled rate. Compounding is what
/// forces `rpower`, and `rpower` is what does not fit — so the schedule here is
/// linear in the discount itself. It is monotonic, it reaches `max` at the
/// deadline and stays there, and it costs one `mul_div` instead of a loop whose
/// length is the age of the auction. A stepwise or exponential schedule can be
/// tabulated into the same shape if a deployment wants one; what cannot be done
/// is compounding a 1e27 rate in a 128-bit register.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DiscountSchedule {
    /// WAD. The multiplier applied at `t = 0`, e.g. 0.98e18 for 2% off.
    pub min: u128,
    /// WAD. The multiplier at and after the deadline, e.g. 0.80e18.
    pub max: u128,
    /// Seconds from creation to reaching `max`.
    pub window: u64,
}

impl DiscountSchedule {
    /// The multiplier at `elapsed` seconds, WAD.
    ///
    /// `min` and `max` are multipliers, so a *larger* discount is a *smaller*
    /// number: the schedule descends. Naming them after the discount rather
    /// than the multiplier is how the reference reads, and it is worth the
    /// sentence to say which one this is.
    pub fn multiplier_at(&self, elapsed: u64) -> Result<u128> {
        if self.min == 0 || self.min > WAD || self.max == 0 || self.max > self.min {
            return Err(AuctionError::DiscountOutOfRange);
        }
        if self.window == 0 || elapsed >= self.window {
            return Ok(self.max);
        }
        let span = self.min - self.max;
        let travelled = mul_div_floor(span, elapsed as u128, self.window as u128)?;
        Ok(self.min - travelled)
    }
}

/// How a quotient that does not divide exactly is resolved.
///
/// Only [`Rounding::Down`] is sound for `collateral_out`. The other variant
/// exists so the test suite can demonstrate what the other choice costs, and so
/// that a reviewer does not have to take the claim on faith.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rounding {
    Down,
    Up,
}

/// One live auction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Auction {
    /// Base units of collateral seized and still unsold.
    pub collateral_left: u128,
    /// Base units of system coin still to raise.
    pub to_raise: u128,
    pub schedule: DiscountSchedule,
}

/// What a bid would do, without doing it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fill {
    /// Base units of system coin actually taken. Never more than `to_raise`.
    pub paid: u128,
    /// Base units of collateral handed to the bidder.
    pub collateral_out: u128,
    /// The multiplier used, WAD — quoted back so a caller can show it.
    pub multiplier: u128,
}

impl Auction {
    /// Price a bid at `elapsed` seconds against a collateral price of
    /// `price` (WAD, system coin per base unit of collateral).
    ///
    /// The bid is clamped to what is left to raise, then to what is left to
    /// sell. Both clamps are the reference's, and both matter: without the
    /// first a bidder overpays into a finished auction, without the second the
    /// auction hands out collateral it does not hold.
    pub fn quote(&self, price: u128, bid: u128, elapsed: u64) -> Result<Fill> {
        self.quote_with(price, bid, elapsed, Rounding::Down)
    }

    /// [`Auction::quote`] with the rounding named, for the tests that show why
    /// it is named.
    pub fn quote_with(
        &self,
        price: u128,
        bid: u128,
        elapsed: u64,
        rounding: Rounding,
    ) -> Result<Fill> {
        if self.collateral_left == 0 || self.to_raise == 0 {
            return Err(AuctionError::NothingLeftToSell);
        }
        if price == 0 {
            return Err(AuctionError::DivideByZero);
        }
        let multiplier = self.schedule.multiplier_at(elapsed)?;
        let paid = core::cmp::min(bid, self.to_raise);

        // collateral = paid / (price · multiplier / WAD)
        //            = paid · WAD / (price · multiplier / WAD)
        //
        // Folded into one mul_div so `price · multiplier` is never stored: that
        // product is 1e36 for an ordinary price, which still fits, but the
        // reference's ray-scaled equivalent does not, and the folding is what
        // makes the difference disappear.
        let discounted = mul_div_floor(price, multiplier, WAD)?;
        if discounted == 0 {
            return Err(AuctionError::DivideByZero);
        }
        let raw = match rounding {
            Rounding::Down => mul_div_floor(paid, WAD, discounted)?,
            Rounding::Up => mul_div_ceil(paid, WAD, discounted)?,
        };
        let collateral_out = core::cmp::min(raw, self.collateral_left);
        Ok(Fill {
            paid,
            collateral_out,
            multiplier,
        })
    }

    /// Apply a fill. Returns the auction after it.
    pub fn settle(&self, fill: Fill) -> Result<Auction> {
        Ok(Auction {
            collateral_left: self
                .collateral_left
                .checked_sub(fill.collateral_out)
                .ok_or(AuctionError::Overflow)?,
            to_raise: self
                .to_raise
                .checked_sub(fill.paid)
                .ok_or(AuctionError::Overflow)?,
            schedule: self.schedule,
        })
    }

    /// The auction is over when there is nothing left to raise or nothing left
    /// to sell. F2 asks for early termination on the first of those.
    pub fn finished(&self) -> bool {
        self.to_raise == 0 || self.collateral_left == 0
    }
}

#[cfg(test)]
mod tests;
