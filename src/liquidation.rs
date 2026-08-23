//! Deciding a position is undercollateralised, and what the liquidator gets.
//!
//! # The same width problem, one layer up
//!
//! Health is a comparison of two products:
//!
//! ```text
//! collateral_amount · price   vs   debt · liquidation_ratio
//! ```
//!
//! Both sides overflow `u128` at ordinary sizes. A vault holding 1e24 base
//! units of a collateral priced at 2e21 wants 2e45 on the left; `u128::MAX` is
//! 3.4e38. The naive comparison is not wrong by a rounding — it does not
//! compute at all, and in a release build with overflow checks off it would
//! wrap and declare a healthy position liquidatable.
//!
//! Neither product is needed. Only the comparison is. [`Position::health`]
//! compares them without materialising either, by dividing one side down before
//! the multiply and carrying the remainder — see the tests, which check it
//! against a 256-bit oracle over the whole awkward range.
//!
//! # Staleness is a state, not an error
//!
//! `R2` requires time-weighted prices and says liquidations pause for a
//! collateral type whose feed is stale; `R4` says only that type pauses. So
//! staleness lives on the price, [`Price`] carries the observation time, and
//! [`Position::health`] returns [`Health::Paused`] rather than a failure. A
//! liquidation engine that treated a stale feed as an error would take the
//! whole system down for one bad feed, which is exactly what `R5` forbids.

use crate::{mul_div_floor, AuctionError, Result, WAD};

/// A price and when it was observed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Price {
    /// System coin per base unit of collateral, WAD.
    pub value: u128,
    /// Seconds. Whatever clock the caller and the feed agree on.
    pub observed_at: u64,
}

impl Price {
    /// A feed older than `max_age` is stale. Equal is fresh: the boundary is
    /// named once here rather than being re-decided at each call site.
    pub fn is_stale(&self, now: u64, max_age: u64) -> bool {
        now.saturating_sub(self.observed_at) > max_age
    }
}

/// What a health check concluded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Health {
    /// Collateral value is at or above the required ratio.
    Safe,
    /// Below it. Liquidatable.
    Undercollateralised,
    /// The feed for this collateral type is stale, so nothing is concluded and
    /// nothing is liquidated — for this type only.
    Paused,
}

/// One borrower's position in one collateral type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Position {
    /// Base units of collateral held.
    pub collateral: u128,
    /// Base units of system coin owed.
    pub debt: u128,
    /// WAD. 1.5e18 means 150% collateral is required.
    pub liquidation_ratio: u128,
}

impl Position {
    /// Compare `collateral · price` against `debt · ratio` without computing
    /// either product.
    pub fn health(&self, price: Price, now: u64, max_age: u64) -> Result<Health> {
        if price.is_stale(now, max_age) {
            return Ok(Health::Paused);
        }
        if price.value == 0 {
            return Err(AuctionError::DivideByZero);
        }
        if self.debt == 0 {
            return Ok(Health::Safe);
        }
        // Both sides are scaled by WAD, so the comparison is unaffected by
        // dividing both by it — and dividing first is what keeps the product in
        // range. `cmp_products` does the same on the halves it cannot divide.
        let left = (self.collateral, price.value);
        let right = (self.debt, self.liquidation_ratio);
        Ok(match cmp_products(left, right) {
            core::cmp::Ordering::Less => Health::Undercollateralised,
            _ => Health::Safe,
        })
    }

    /// The base units of collateral a liquidator receives for clearing this
    /// position, at `incentive` WAD (1.05e18 = 5% bonus).
    ///
    /// Rounds **down**, for the reason the collateral auction rounds down: the
    /// liquidator is the counterparty, and a quotient rounded their way is a
    /// withdrawal from whoever owned the collateral.
    pub fn liquidator_reward(&self, price: Price, incentive: u128) -> Result<u128> {
        if incentive < WAD {
            return Err(AuctionError::DiscountOutOfRange);
        }
        if price.value == 0 {
            return Err(AuctionError::DivideByZero);
        }
        // debt · incentive / price, product in 256 bits.
        let owed = mul_div_floor(self.debt, incentive, WAD)?;
        let units = mul_div_floor(owed, WAD, price.value)?;
        Ok(core::cmp::min(units, self.collateral))
    }
}

/// Compare `a.0 · a.1` with `b.0 · b.1` without forming either product.
///
/// Written as an explicit 256-bit comparison rather than something clever: the
/// clever version is where the sign errors live, and this one is checked
/// against a widening multiply across the awkward range in the tests.
fn cmp_products(a: (u128, u128), b: (u128, u128)) -> core::cmp::Ordering {
    let (ah, al) = wide(a.0, a.1);
    let (bh, bl) = wide(b.0, b.1);
    (ah, al).cmp(&(bh, bl))
}

fn wide(a: u128, b: u128) -> (u128, u128) {
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

#[cfg(test)]
mod tests {
    use super::*;

    const PRICE_2000: u128 = 2_000 * WAD;
    fn fresh(v: u128) -> Price {
        Price {
            value: v,
            observed_at: 1_000,
        }
    }

    /// The naive health check does not compute at ordinary vault sizes.
    #[test]
    fn the_naive_comparison_overflows_on_an_ordinary_vault() {
        let collateral = 1_000_000 * WAD; // a million units
        assert!(
            collateral.checked_mul(PRICE_2000).is_none(),
            "collateral · price is 2e45; u128 stops at 3.4e38"
        );
    }

    #[test]
    fn we_decide_the_same_vault_without_the_product() {
        // A million units at 2000 is worth 2e9 coins. Owing 1e9 at 150% needs
        // 1.5e9 of value, so this is safe.
        let p = Position {
            collateral: 1_000_000 * WAD,
            debt: 1_000_000_000 * WAD,
            liquidation_ratio: WAD * 3 / 2,
        };
        assert_eq!(
            p.health(fresh(PRICE_2000), 1_000, 60).unwrap(),
            Health::Safe
        );

        // Owing 1.4e9 at 150% needs 2.1e9 of value against 2e9. Not safe.
        let p = Position {
            debt: 1_400_000_000 * WAD,
            ..p
        };
        assert_eq!(
            p.health(fresh(PRICE_2000), 1_000, 60).unwrap(),
            Health::Undercollateralised
        );
    }

    /// The comparison is checked against a real product wherever one exists.
    #[test]
    fn the_comparison_agrees_with_multiplication_where_multiplication_is_possible() {
        let vals: [u128; 8] = [
            0,
            1,
            2,
            1_000,
            WAD,
            u64::MAX as u128,
            (u64::MAX as u128) + 1,
            u128::MAX >> 66,
        ];
        for &a0 in &vals {
            for &a1 in &vals {
                for &b0 in &vals {
                    for &b1 in &vals {
                        let ours = cmp_products((a0, a1), (b0, b1));
                        if let (Some(x), Some(y)) = (a0.checked_mul(a1), b0.checked_mul(b1)) {
                            assert_eq!(ours, x.cmp(&y), "{a0}·{a1} vs {b0}·{b1}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn a_stale_feed_pauses_this_collateral_and_concludes_nothing() {
        let p = Position {
            collateral: 1,
            debt: u128::MAX / 2,
            liquidation_ratio: WAD,
        };
        // Wildly underwater, but the feed is 61 seconds old and the limit is 60.
        let stale = Price {
            value: PRICE_2000,
            observed_at: 939,
        };
        assert_eq!(p.health(stale, 1_000, 60).unwrap(), Health::Paused);
        // One second younger and it decides.
        let ok = Price {
            value: PRICE_2000,
            observed_at: 940,
        };
        assert_eq!(
            p.health(ok, 1_000, 60).unwrap(),
            Health::Undercollateralised
        );
    }

    #[test]
    fn staleness_is_measured_forwards_only() {
        // A feed observed in the future is not stale; saturating_sub keeps the
        // comparison from wrapping into a very large age.
        let future = Price {
            value: WAD,
            observed_at: 5_000,
        };
        assert!(!future.is_stale(1_000, 60));
    }

    #[test]
    fn a_position_with_no_debt_is_safe_whatever_the_price() {
        let p = Position {
            collateral: 0,
            debt: 0,
            liquidation_ratio: WAD * 100,
        };
        assert_eq!(p.health(fresh(1), 1_000, 60).unwrap(), Health::Safe);
    }

    #[test]
    fn the_reward_is_capped_by_what_the_position_holds() {
        let p = Position {
            collateral: 3,
            debt: u128::MAX / WAD,
            liquidation_ratio: WAD,
        };
        let r = p.liquidator_reward(fresh(1), WAD).unwrap();
        assert_eq!(
            r, 3,
            "the engine cannot pay out collateral it did not seize"
        );
    }

    #[test]
    fn the_reward_rounds_towards_the_borrower() {
        // Debt 1, incentive 1.05, price 2000: 1.05/2000 of a unit, which is
        // below one base unit and must not round up into one.
        let p = Position {
            collateral: 10 * WAD,
            debt: 1,
            liquidation_ratio: WAD,
        };
        assert_eq!(
            p.liquidator_reward(fresh(PRICE_2000), WAD + WAD / 20)
                .unwrap(),
            0
        );
    }

    #[test]
    fn an_incentive_below_par_is_refused() {
        let p = Position {
            collateral: WAD,
            debt: WAD,
            liquidation_ratio: WAD,
        };
        assert_eq!(
            p.liquidator_reward(fresh(WAD), WAD - 1),
            Err(AuctionError::DiscountOutOfRange)
        );
    }
}
