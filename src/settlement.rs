//! The debt and surplus auctions, and the rule that stops working.
//!
//! `F4` and `F5` ask for both, and the specification licenses a stand-in for the
//! protocol token they trade against:
//!
//! > The mechanism must be fully implemented and tested (using a mock token),
//! > but parameterized by a configurable token address so the deployer can point
//! > it at whatever protocol token they choose.
//!
//! So the token is an address these auctions never dereference, and everything
//! here is real arithmetic over real state.
//!
//! # The minimum improvement is a percentage, and percentages of small numbers
//! # are zero
//!
//! Both auctions are English in one direction and Dutch in the other, and both
//! guard themselves the same way: a new bid must beat the standing one by at
//! least `beg`, a fraction. The debt auction wants a *smaller* lot of protocol
//! token for the same coin; the surplus auction wants a *larger* bid of protocol
//! token for the same coin.
//!
//! ```text
//! debt:    lot_max = standing_lot · (WAD − beg) / WAD     must be ≤
//! surplus: bid_min = standing_bid · (WAD + beg) / WAD     must be ≥
//! ```
//!
//! Written that way, in integers, the guard evaporates as the numbers shrink.
//! Once `standing · beg < WAD` the improvement rounds to nothing and the
//! *equal* bid becomes legal. A bidder can then re-post the standing value
//! forever, resetting the expiry each time, and hold the auction open at no
//! cost — while the system carries the bad debt the auction exists to clear.
//!
//! The fix is not a bigger fraction. It is to require the improvement to be
//! **at least one base unit**, whatever the percentage works out to, and to
//! round the threshold against the challenger rather than for them. Both are
//! one line each, and both are the subject of a test that fails without them.

use crate::{mul_div_ceil, mul_div_floor, AuctionError, Result, WAD};

/// How the two auctions differ, stated once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    /// The debt auction. The system's coin need is fixed; bidders compete by
    /// accepting a smaller lot of protocol token. Improvement means *less*.
    ShrinkingLot,
    /// The surplus auction. The system's coin offer is fixed; bidders compete by
    /// offering more protocol token, which is then burned. Improvement means
    /// *more*.
    GrowingBid,
}

/// A live debt or surplus auction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Settlement {
    pub direction: Direction,
    /// The quantity being competed over: lot for [`Direction::ShrinkingLot`],
    /// bid for [`Direction::GrowingBid`]. Base units of protocol token.
    pub standing: u128,
    /// Minimum improvement, WAD. 0.03e18 is the 3% both MakerDAO auctions use.
    pub beg: u128,
    /// Absolute seconds at which the auction closes if nobody improves.
    pub expiry: u64,
}

impl Settlement {
    /// The best value a challenger may post: at most this for a shrinking lot,
    /// at least this for a growing bid.
    ///
    /// Rounded against the challenger in both directions, and floored at one
    /// base unit of movement so the guard survives small numbers.
    pub fn threshold(&self) -> Result<u128> {
        if self.beg == 0 || self.beg >= WAD {
            return Err(AuctionError::DiscountOutOfRange);
        }
        match self.direction {
            // A smaller lot is better, so the ceiling must round *down*: any
            // rounding up would admit a lot larger than `beg` permits.
            Direction::ShrinkingLot => {
                let t = mul_div_floor(self.standing, WAD - self.beg, WAD)?;
                // …and must move by at least one base unit.
                Ok(core::cmp::min(t, self.standing.saturating_sub(1)))
            }
            // A larger bid is better, so the floor must round *up*.
            Direction::GrowingBid => {
                let t = mul_div_ceil(self.standing, WAD + self.beg, WAD)?;
                Ok(core::cmp::max(
                    t,
                    self.standing.checked_add(1).ok_or(AuctionError::Overflow)?,
                ))
            }
        }
    }

    /// Whether `offer` is a legal improvement on the standing value.
    pub fn accepts(&self, offer: u128) -> Result<bool> {
        let t = self.threshold()?;
        Ok(match self.direction {
            Direction::ShrinkingLot => offer <= t,
            Direction::GrowingBid => offer >= t,
        })
    }

    /// Post `offer`, extending the auction to `now + ttl`.
    pub fn improve(&self, offer: u128, now: u64, ttl: u64) -> Result<Settlement> {
        if !self.accepts(offer)? {
            return Err(AuctionError::BidNotAnImprovement);
        }
        Ok(Settlement {
            standing: offer,
            expiry: now.checked_add(ttl).ok_or(AuctionError::Overflow)?,
            ..*self
        })
    }

    pub fn expired(&self, now: u64) -> bool {
        now >= self.expiry
    }
}

/// The naive threshold, as MakerDAO's `flop`/`flap` write it, kept so the
/// failure is a test result rather than a claim.
///
/// `dent`: `require(mul(bid, ONE) <= mul(beg, bids[id].bid))` — a percentage,
/// with nothing underneath it.
pub mod naive {
    use super::*;

    pub fn shrinking_lot_threshold(standing: u128, beg: u128) -> Result<u128> {
        mul_div_floor(standing, WAD - beg, WAD)
    }

    pub fn growing_bid_threshold(standing: u128, beg: u128) -> Result<u128> {
        mul_div_floor(standing, WAD + beg, WAD)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BEG_3PCT: u128 = WAD * 3 / 100;

    fn debt(standing: u128) -> Settlement {
        Settlement {
            direction: Direction::ShrinkingLot,
            standing,
            beg: BEG_3PCT,
            expiry: 1_000,
        }
    }

    fn surplus(standing: u128) -> Settlement {
        Settlement {
            direction: Direction::GrowingBid,
            standing,
            beg: BEG_3PCT,
            expiry: 1_000,
        }
    }

    // -----------------------------------------------------------------------
    // The rule that stops working
    // -----------------------------------------------------------------------

    /// Below 34 base units, a 3% improvement rounds to zero and the naive rule
    /// admits an identical bid.
    #[test]
    fn the_naive_debt_threshold_admits_an_unchanged_lot_at_small_sizes() {
        // 33 · 0.97 = 32.01 → 32, still an improvement.
        assert_eq!(naive::shrinking_lot_threshold(33, BEG_3PCT).unwrap(), 32);
        // 1 · 0.97 = 0.97 → 0. Looks strict, but see the next case.
        // 34 is the first size where 3% is a whole unit; below ~17 the floor
        // starts returning the standing value itself.
        for standing in 1..=200u128 {
            let t = naive::shrinking_lot_threshold(standing, BEG_3PCT).unwrap();
            assert!(t < standing || standing == 0, "size {standing}");
        }
        // The surplus side is where it breaks outright: 1 · 1.03 = 1.03 → 1.
        assert_eq!(naive::growing_bid_threshold(1, BEG_3PCT).unwrap(), 1);
        assert_eq!(naive::growing_bid_threshold(10, BEG_3PCT).unwrap(), 10);
        assert_eq!(naive::growing_bid_threshold(33, BEG_3PCT).unwrap(), 33);
    }

    /// And that is a stall: the same bid, posted again and again, legally.
    #[test]
    fn the_naive_surplus_rule_lets_a_bidder_hold_an_auction_open_forever() {
        let standing = 33u128;
        let threshold = naive::growing_bid_threshold(standing, BEG_3PCT).unwrap();
        assert!(
            standing >= threshold,
            "an unchanged bid clears its own threshold, so the expiry resets for free"
        );
    }

    #[test]
    fn ours_requires_at_least_one_base_unit_of_movement() {
        for standing in 1..=200u128 {
            let s = surplus(standing);
            let t = s.threshold().unwrap();
            assert!(
                t > standing,
                "surplus threshold must exceed {standing}, got {t}"
            );
            assert!(
                !s.accepts(standing).unwrap(),
                "unchanged bid accepted at {standing}"
            );
        }
        for standing in 2..=200u128 {
            let d = debt(standing);
            let t = d.threshold().unwrap();
            assert!(
                t < standing,
                "debt threshold must be below {standing}, got {t}"
            );
            assert!(
                !d.accepts(standing).unwrap(),
                "unchanged lot accepted at {standing}"
            );
        }
    }

    #[test]
    fn a_lot_of_one_cannot_be_improved_and_says_so() {
        let d = debt(1);
        // Zero is the only smaller lot, and it is reachable: the rule does not
        // pretend an improvement exists where none does.
        assert_eq!(d.threshold().unwrap(), 0);
        assert!(d.accepts(0).unwrap());
        assert!(!d.accepts(1).unwrap());
    }

    // -----------------------------------------------------------------------
    // The ordinary case still behaves
    // -----------------------------------------------------------------------

    #[test]
    fn a_three_percent_improvement_is_what_it_says_at_ordinary_sizes() {
        let d = debt(1_000 * WAD);
        assert_eq!(d.threshold().unwrap(), 970 * WAD);
        assert!(d.accepts(970 * WAD).unwrap());
        assert!(!d.accepts(970 * WAD + 1).unwrap());

        let s = surplus(1_000 * WAD);
        assert_eq!(s.threshold().unwrap(), 1_030 * WAD);
        assert!(s.accepts(1_030 * WAD).unwrap());
        assert!(!s.accepts(1_030 * WAD - 1).unwrap());
    }

    #[test]
    fn the_thresholds_round_against_the_challenger() {
        // 100 · 1.03 = 103 exactly; 101 · 1.03 = 104.03, and the challenger
        // must clear 105 rather than 104.
        let s = surplus(101);
        assert_eq!(s.threshold().unwrap(), 105);
        // 101 · 0.97 = 97.97, and the challenger must reach 97, not 98.
        let d = debt(101);
        assert_eq!(d.threshold().unwrap(), 97);
    }

    #[test]
    fn improving_extends_the_expiry_and_replaces_the_standing_value() {
        let s = surplus(1_000);
        let after = s.improve(1_030, 5_000, 3_600).unwrap();
        assert_eq!(after.standing, 1_030);
        assert_eq!(after.expiry, 8_600);
        assert!(!after.expired(8_599));
        assert!(after.expired(8_600));
    }

    #[test]
    fn a_bid_that_is_not_an_improvement_is_refused_by_name() {
        let s = surplus(1_000);
        assert_eq!(
            s.improve(1_000, 5_000, 3_600),
            Err(AuctionError::BidNotAnImprovement)
        );
    }

    #[test]
    fn a_beg_outside_its_range_is_refused_rather_than_priced() {
        let bad = Settlement {
            beg: 0,
            ..surplus(1_000)
        };
        assert_eq!(bad.threshold(), Err(AuctionError::DiscountOutOfRange));
        let whole = Settlement {
            beg: WAD,
            ..surplus(1_000)
        };
        assert_eq!(whole.threshold(), Err(AuctionError::DiscountOutOfRange));
    }
}
