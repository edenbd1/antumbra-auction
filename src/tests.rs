use super::*;

// ---------------------------------------------------------------------------
// What the reference costs on this platform
// ---------------------------------------------------------------------------

/// A collateral priced at 2000 system coins. Nothing exotic.
const PRICE_2000: u128 = 2_000 * WAD;

#[test]
fn reference_rdivide_overflows_at_an_ordinary_collateral_price() {
    // rdivide takes the product first: 2000e18 · 1e27 = 2e48.
    assert!(PRICE_2000.checked_mul(RAY).is_none());
    assert_eq!(reference::rdivide(PRICE_2000, WAD), None);
}

/// The threshold is not near any extreme, which is the point.
#[test]
fn the_reference_overflows_for_any_price_above_a_third_of_a_millionth() {
    // x · RAY fits only while x ≤ u128::MAX / RAY.
    let largest_that_fits = u128::MAX / RAY;
    assert!(reference::rdivide(largest_that_fits, WAD).is_some());
    assert_eq!(reference::rdivide(largest_that_fits + 1, WAD), None);

    // Expressed as a WAD price, that ceiling is about 3.4e-7 — so every price
    // from a third of a millionth upwards is out of reach.
    assert!(largest_that_fits < WAD / 1_000_000);
}

#[test]
fn reference_rpower_overflows_on_its_first_squaring() {
    // The discount compounds per second. Squaring a ray-scaled rate is 1e54.
    assert!(RAY.checked_mul(RAY).is_none());
    // Any exponent that reaches the squaring loop fails; n = 2 is enough.
    assert_eq!(reference::rpower(RAY, 2), None);
    assert_eq!(reference::rpower(RAY + RAY / 1_000_000, 3_600), None);
}

#[test]
fn reference_cannot_price_a_single_bid() {
    let bid = 2_000 * WAD;
    let five_percent_off = WAD - WAD / 20;
    assert_eq!(
        reference::bought_collateral(PRICE_2000, WAD, five_percent_off, bid),
        None,
        "the reference chain is expected to die at rdivide, before any auction logic"
    );
}

/// The transcription is faithful, not rigged: give it numbers small enough and
/// it computes, which is what makes the failures above about the width and not
/// about the code.
#[test]
fn the_transcription_still_works_where_a_uint256_is_not_needed() {
    let tiny_price = 1_000u128; // far below the 3.4e11 ceiling
    let ratio = reference::rdivide(tiny_price, WAD).expect("fits");
    assert_eq!(ratio, tiny_price * RAY / WAD);
    assert!(reference::rpower(RAY / 1_000_000_000, 2).is_some());
}

// ---------------------------------------------------------------------------
// What we do instead
// ---------------------------------------------------------------------------

fn flat() -> DiscountSchedule {
    DiscountSchedule {
        min: WAD,
        max: WAD,
        window: 0,
    }
}

fn escalating() -> DiscountSchedule {
    // 2% off at creation, 20% off after an hour.
    DiscountSchedule {
        min: WAD - WAD / 50,
        max: WAD - WAD / 5,
        window: 3_600,
    }
}

#[test]
fn we_price_the_bid_the_reference_could_not() {
    let a = Auction {
        collateral_left: 10 * WAD,
        to_raise: 20_000 * WAD,
        schedule: flat(),
    };
    let fill = a.quote(PRICE_2000, 2_000 * WAD, 0).expect("prices");
    // 2000 coins at 2000 per unit, no discount: exactly one unit.
    assert_eq!(fill.collateral_out, WAD);
    assert_eq!(fill.paid, 2_000 * WAD);
}

#[test]
fn the_discount_escalates_and_then_stops() {
    let s = escalating();
    assert_eq!(s.multiplier_at(0).unwrap(), WAD - WAD / 50);
    // Halfway: 2% has grown to 11%.
    assert_eq!(s.multiplier_at(1_800).unwrap(), WAD - WAD * 11 / 100);
    assert_eq!(s.multiplier_at(3_600).unwrap(), WAD - WAD / 5);
    // F2 says the schedule is configurable; it must not keep going past max.
    assert_eq!(s.multiplier_at(u64::MAX).unwrap(), WAD - WAD / 5);
}

#[test]
fn the_multiplier_never_increases() {
    let s = escalating();
    let mut previous = u128::MAX;
    for t in (0..4_000).step_by(7) {
        let m = s.multiplier_at(t).unwrap();
        assert!(m <= previous, "the discount went backwards at t={t}");
        previous = m;
    }
}

#[test]
fn a_discount_outside_its_range_is_refused_rather_than_priced() {
    let bad = DiscountSchedule {
        min: 0,
        max: 0,
        window: 10,
    };
    assert_eq!(bad.multiplier_at(0), Err(AuctionError::DiscountOutOfRange));
    let inverted = DiscountSchedule {
        min: WAD / 2,
        max: WAD,
        window: 10,
    };
    assert_eq!(
        inverted.multiplier_at(0),
        Err(AuctionError::DiscountOutOfRange)
    );
}

// ---------------------------------------------------------------------------
// The rounding direction, and what the other one costs
// ---------------------------------------------------------------------------

/// F3 requires partial fills. That hands the bidder control of how many times
/// the quotient is rounded, and rounding it up turns each fill into a small
/// gift. Splitting one bid into 2000 does not extract 2000 dust — it extracts
/// 2000 whole base units where the honest price buys one.
#[test]
fn ceil_lets_a_bidder_split_a_bid_and_extract_collateral() {
    let seized = 10 * WAD;
    let start = Auction {
        collateral_left: seized,
        to_raise: 20_000 * WAD,
        schedule: flat(),
    };

    // One honest fill: 2000 base units of coin buys exactly 1 base unit.
    let whole = start
        .quote_with(PRICE_2000, 2_000, 0, Rounding::Down)
        .unwrap();
    assert_eq!(whole.collateral_out, 1);

    // The same 2000 base units, split into 2000 fills of one.
    let mut up = start;
    let mut got_up: u128 = 0;
    for _ in 0..2_000 {
        let f = up.quote_with(PRICE_2000, 1, 0, Rounding::Up).unwrap();
        got_up += f.collateral_out;
        up = up.settle(f).unwrap();
    }

    let mut down = start;
    let mut got_down: u128 = 0;
    for _ in 0..2_000 {
        let f = down.quote_with(PRICE_2000, 1, 0, Rounding::Down).unwrap();
        got_down += f.collateral_out;
        down = down.settle(f).unwrap();
    }

    assert_eq!(
        got_up, 2_000,
        "each fill rounded a 0.0005 quotient up to a whole unit"
    );
    assert_eq!(
        got_down, 0,
        "the honest quotient of a one-unit bid is below a base unit"
    );
    assert_eq!(
        got_up / whole.collateral_out,
        2_000,
        "splitting the bid multiplied the collateral received by the number of fills"
    );
}

/// The leak is bounded only by how small a bid the auction accepts, so it is
/// not dust: with no minimum, it drains everything that was seized.
#[test]
fn ceil_drains_the_whole_auction_if_bids_can_be_small_enough() {
    let seized = 1_000u128; // base units
    let mut a = Auction {
        collateral_left: seized,
        to_raise: 10_000_000 * WAD,
        schedule: flat(),
    };
    let mut paid_total = 0u128;
    let mut got = 0u128;
    while !a.finished() {
        let f = a.quote_with(PRICE_2000, 1, 0, Rounding::Up).unwrap();
        paid_total += f.paid;
        got += f.collateral_out;
        a = a.settle(f).unwrap();
    }
    assert_eq!(got, seized, "every seized unit left the auction");
    // Honest cost of that collateral: 1000 units × 2000 = 2,000,000.
    assert_eq!(paid_total, 1_000);
    assert!(paid_total * 2_000 < 2_000_001);
    assert_eq!(
        2_000_000 / paid_total,
        2_000,
        "paid one two-thousandth of what the collateral was worth"
    );
}

/// And the direction we ship: over any sequence of fills, the bidder never
/// receives more than the price says. That is `R6` stated as an assertion.
#[test]
fn floor_keeps_the_invariant_across_a_long_sequence_of_partial_fills() {
    let seized = 5 * WAD;
    let mut a = Auction {
        collateral_left: seized,
        to_raise: 30_000 * WAD,
        schedule: escalating(),
    };
    let mut out = 0u128;
    let mut paid = 0u128;
    let mut t = 0u64;
    // Bids of awkward sizes so the quotient rarely divides exactly.
    for step in [7_777u128, 131, 999_999, 3, 1_000_000_007, 42]
        .into_iter()
        .cycle()
        .take(600)
    {
        if a.finished() {
            break;
        }
        let f = a.quote(PRICE_2000, step, t).unwrap();
        out += f.collateral_out;
        paid += f.paid;
        a = a.settle(f).unwrap();
        t += 11;
    }
    assert_eq!(out + a.collateral_left, seized, "collateral is conserved");
    assert!(out <= seized);
    // Every unit handed over was paid for at or above the discounted price.
    let worst = a.schedule.multiplier_at(t).unwrap();
    let owed = mul_div_floor(out, mul_div_floor(PRICE_2000, worst, WAD).unwrap(), WAD).unwrap();
    assert!(
        paid >= owed,
        "paid {paid} for collateral worth at least {owed}"
    );
}

// ---------------------------------------------------------------------------
// Clamps
// ---------------------------------------------------------------------------

#[test]
fn a_bid_larger_than_what_is_left_to_raise_is_clamped() {
    let a = Auction {
        collateral_left: 100 * WAD,
        to_raise: 500,
        schedule: flat(),
    };
    let f = a.quote(PRICE_2000, 10_000 * WAD, 0).unwrap();
    assert_eq!(
        f.paid, 500,
        "the bidder is not allowed to overpay a finished auction"
    );
}

#[test]
fn a_bid_that_would_buy_more_than_was_seized_is_clamped() {
    let a = Auction {
        collateral_left: 3,
        to_raise: u128::MAX,
        schedule: flat(),
    };
    let f = a.quote(PRICE_2000, 1_000_000 * WAD, 0).unwrap();
    assert_eq!(
        f.collateral_out, 3,
        "the auction cannot hand out collateral it does not hold"
    );
}

#[test]
fn a_finished_auction_refuses_rather_than_returning_nothing() {
    let empty = Auction {
        collateral_left: 0,
        to_raise: 10,
        schedule: flat(),
    };
    assert_eq!(
        empty.quote(PRICE_2000, 1, 0),
        Err(AuctionError::NothingLeftToSell)
    );
    let raised = Auction {
        collateral_left: 10,
        to_raise: 0,
        schedule: flat(),
    };
    assert_eq!(
        raised.quote(PRICE_2000, 1, 0),
        Err(AuctionError::NothingLeftToSell)
    );
}

#[test]
fn a_zero_price_is_refused_rather_than_dividing() {
    let a = Auction {
        collateral_left: WAD,
        to_raise: WAD,
        schedule: flat(),
    };
    assert_eq!(a.quote(0, 1, 0), Err(AuctionError::DivideByZero));
}

// ---------------------------------------------------------------------------
// The wide arithmetic itself
// ---------------------------------------------------------------------------

#[test]
fn mul_div_survives_products_the_platform_cannot_hold() {
    // 2e21 · 1e27 is 2e48; the quotient is 2e21 and fits.
    assert_eq!(mul_div_floor(PRICE_2000, RAY, RAY).unwrap(), PRICE_2000);
    assert!(PRICE_2000.checked_mul(RAY).is_none());
}

#[test]
fn mul_div_refuses_a_quotient_that_does_not_fit() {
    assert_eq!(
        mul_div_floor(u128::MAX, u128::MAX, 1),
        Err(AuctionError::Overflow)
    );
}

#[test]
fn floor_and_ceil_differ_by_exactly_one_when_they_differ() {
    for (a, b, d) in [(7u128, 3u128, 2u128), (WAD, 3, 7), (u128::MAX / 3, 3, 5)] {
        let f = mul_div_floor(a, b, d).unwrap();
        let c = mul_div_ceil(a, b, d).unwrap();
        assert!(c == f || c == f + 1);
    }
}

// ---------------------------------------------------------------------------
// What the lot running out is allowed to cost
//
// These two came out of reading the live testnet auction's account rather than
// out of a design review: auction #2 sat with 3.947368421052631579 units left
// and 4,000 still to raise, and the arithmetic said a bid of 4,000 would take
// all three of those things — the lot, the whole bid, and the shortfall.
// ---------------------------------------------------------------------------

#[test]
fn a_bid_bigger_than_the_lot_pays_only_for_the_lot() {
    // The live second auction: 40 % off at t=600 on a price of 1000, so the
    // remaining 3.947368421052631579 units are worth 2368.4210526315789474.
    let a = Auction {
        collateral_left: 3_947_368_421_052_631_579,
        to_raise: 4_000 * WAD,
        schedule: DiscountSchedule {
            min: 950_000_000_000_000_000,
            max: 600_000_000_000_000_000,
            window: 600,
        },
    };
    let fill = a.quote(1_000 * WAD, 4_000 * WAD, 600).unwrap();
    assert_eq!(fill.collateral_out, 3_947_368_421_052_631_579);
    assert_eq!(fill.paid, 2_368_421_052_631_578_947_400);

    // And the point of it: the auction closes short, which is what F4 exists
    // to notice. Charging the full bid would have closed it at exactly the
    // target and reported no shortfall at all.
    let after = a.settle(fill).unwrap();
    assert_eq!(after.collateral_left, 0);
    assert_eq!(after.to_raise, 4_000 * WAD - 2_368_421_052_631_578_947_400);
    assert!(after.to_raise > 0, "the shortfall must survive the fill");
}

#[test]
fn the_restrike_never_charges_more_than_the_bid_it_replaces() {
    // Sweep the whole boundary: for every lot size, a bid large enough to clamp
    // must cost no more than the same bid unclamped, and must still buy the lot.
    for left in 1u128..400 {
        let a = Auction {
            collateral_left: left,
            to_raise: u128::MAX / 4,
            schedule: DiscountSchedule {
                min: WAD,
                max: WAD,
                window: 1,
            },
        };
        let bid = 1_000_000u128;
        let fill = a.quote(3 * WAD, bid, 0).unwrap();
        assert!(fill.paid <= bid);
        // Every one of these clamps on the lot — 1e6 buys 333,333 units at a
        // price of 3 and the lot is under 400 — so every one must be charged at
        // the struck price and not at the bid. Asserting `paid <= bid` alone
        // would pass on the unfixed code, which charges exactly `bid`.
        assert_eq!(fill.collateral_out, left);
        assert_eq!(fill.paid, left * 3);
    }
}

#[test]
fn a_target_clamp_is_not_a_lot_clamp_and_charges_the_whole_of_it() {
    // The other clamp must keep its old behaviour: when it is the *target* that
    // runs out, the bidder pays what is left to raise, in full.
    let a = Auction {
        collateral_left: 1_000 * WAD,
        to_raise: 500 * WAD,
        schedule: DiscountSchedule {
            min: WAD,
            max: WAD,
            window: 1,
        },
    };
    let fill = a.quote(WAD, 900 * WAD, 0).unwrap();
    assert_eq!(fill.paid, 500 * WAD);
    assert_eq!(fill.collateral_out, 500 * WAD);
}
