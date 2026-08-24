// Measures the zkVM cycle cost of every priced auction operation, inside the
// guest.
//
// The fixtures are compiled in rather than read from the host, so the run is
// reproducible from the binary alone: the same guest ELF always measures the
// same work. Each op is a `cycle_count()` delta with the measurement's own
// overhead subtracted, and `black_box` keeps the optimiser from hoisting a pure
// function out of the region being timed.
//
// What is measured is what RFP-014's performance requirements ask about. P2
// wants many concurrent auctions inside the compute limit, and the answer is
// that cost is flat in the number of auctions because each is its own account —
// so what matters is the cost of ONE bid, which is `quote` plus `settle`. P3
// wants a bid inside one block, and the same figure answers it.

use core::hint::black_box;
use risc0_zkvm::guest::env;

use antumbra_auction::{
    liquidation::{Position, Price},
    settlement::{Direction, Settlement},
    mul_div_ceil, mul_div_floor, Auction, DiscountSchedule, Rounding, WAD,
};

const SCHEDULE: DiscountSchedule = DiscountSchedule {
    min: 980_000_000_000_000_000,
    max: 800_000_000_000_000_000,
    window: 3_600,
};

/// The moments a real auction is quoted at: the instant it opens, the middle of
/// the window where the multiplier is interpolated, the deadline, and past it.
const ELAPSED: [u64; 6] = [0, 1, 900, 1_800, 3_600, 7_200];

/// (collateral_left, to_raise, price, bid). Ordinary sizes, then the two clamps:
/// a lot too small for the bid, and a target smaller than the bid.
const QUOTES: [(u128, u128, u128, u128); 6] = [
    (5_000 * WAD, 5_000 * WAD, 2 * WAD, 1_000 * WAD),
    (5_000 * WAD, 5_000 * WAD, 2 * WAD, 4_000 * WAD),
    (1_184 * WAD, 5_000 * WAD, 2 * WAD, 4_000 * WAD),
    (5_000 * WAD, 1_000 * WAD, 2 * WAD, 4_000 * WAD),
    (1, 5_000 * WAD, 2 * WAD, 4_000 * WAD),
    (u128::MAX / 4, u128::MAX / 4, WAD, u128::MAX / 8),
];

fn timed<T>(baseline: u64, f: impl FnOnce() -> T) -> u64 {
    let a = env::cycle_count();
    black_box(f());
    let b = env::cycle_count();
    (b - a).saturating_sub(baseline)
}

fn stats(v: &[u64]) -> (u64, u64, u64) {
    let mut s = v.to_vec();
    s.sort_unstable();
    (s[s.len() / 2], s[0], s[s.len() - 1])
}

fn main() {
    // Cost of the measurement itself, so every figure below is net.
    let baseline = {
        let a = env::cycle_count();
        black_box(0u128);
        let b = env::cycle_count();
        b - a
    };

    let mut out: Vec<(&str, u64, u64, u64, usize)> = Vec::new();

    let v: Vec<u64> = ELAPSED
        .iter()
        .map(|&e| timed(baseline, || SCHEDULE.multiplier_at(black_box(e))))
        .collect();
    let (m, lo, hi) = stats(&v);
    out.push(("multiplier_at", m, lo, hi, v.len()));

    let v: Vec<u64> = QUOTES
        .iter()
        .flat_map(|&(left, raise, price, bid)| {
            let a = Auction { collateral_left: left, to_raise: raise, schedule: SCHEDULE };
            ELAPSED.iter().map(move |&e| {
                timed(baseline, || {
                    a.quote_with(black_box(price), black_box(bid), black_box(e), Rounding::Down)
                })
            })
        })
        .collect();
    let (m, lo, hi) = stats(&v);
    out.push(("quote", m, lo, hi, v.len()));

    let v: Vec<u64> = QUOTES
        .iter()
        .map(|&(left, raise, price, bid)| {
            let a = Auction { collateral_left: left, to_raise: raise, schedule: SCHEDULE };
            let f = a.quote(price, bid, 900).unwrap();
            timed(baseline, || a.settle(black_box(f)))
        })
        .collect();
    let (m, lo, hi) = stats(&v);
    out.push(("settle", m, lo, hi, v.len()));

    let v: Vec<u64> = QUOTES
        .iter()
        .map(|&(_, _, price, _)| {
            let p = Position { collateral: 1_000 * WAD, debt: 500 * WAD, liquidation_ratio: 15 * WAD / 10 };
            let feed = Price { value: price, observed_at: 0 };
            timed(baseline, || p.health(black_box(feed), black_box(60), black_box(3_600)))
        })
        .collect();
    let (m, lo, hi) = stats(&v);
    out.push(("health", m, lo, hi, v.len()));

    let v: Vec<u64> = QUOTES
        .iter()
        .map(|&(_, _, price, _)| {
            let p = Position { collateral: 1_000 * WAD, debt: 500 * WAD, liquidation_ratio: 15 * WAD / 10 };
            let feed = Price { value: price, observed_at: 0 };
            timed(baseline, || p.liquidator_reward(black_box(feed), black_box(105 * WAD / 100)))
        })
        .collect();
    let (m, lo, hi) = stats(&v);
    out.push(("liquidator_reward", m, lo, hi, v.len()));

    let v: Vec<u64> = QUOTES
        .iter()
        .map(|&(_, _, _, bid)| {
            let s = Settlement {
                direction: Direction::ShrinkingLot,
                standing: bid / 2,
                beg: 103 * WAD / 100,
                expiry: 3_600,
            };
            timed(baseline, || s.accepts(black_box(bid)))
        })
        .collect();
    let (m, lo, hi) = stats(&v);
    out.push(("accepts", m, lo, hi, v.len()));

    let v: Vec<u64> = QUOTES
        .iter()
        .map(|&(left, _, price, _)| timed(baseline, || mul_div_floor(black_box(left), black_box(price), black_box(WAD))))
        .collect();
    let (m, lo, hi) = stats(&v);
    out.push(("mul_div_floor", m, lo, hi, v.len()));

    let v: Vec<u64> = QUOTES
        .iter()
        .map(|&(left, _, price, _)| timed(baseline, || mul_div_ceil(black_box(left), black_box(price), black_box(WAD))))
        .collect();
    let (m, lo, hi) = stats(&v);
    out.push(("mul_div_ceil", m, lo, hi, v.len()));

    // One whole bid, end to end, because that is the number P2 and P3 are about:
    // quote the fill and apply it, exactly as the program does per bid.
    let whole_bid = {
        let a = Auction {
            collateral_left: 5_000 * WAD,
            to_raise: 5_000 * WAD,
            schedule: SCHEDULE,
        };
        timed(baseline, || {
            let f = a.quote(black_box(2 * WAD), black_box(4_000 * WAD), black_box(900)).unwrap();
            a.settle(f)
        })
    };

    env::commit(&(out, baseline, whole_bid));
}
