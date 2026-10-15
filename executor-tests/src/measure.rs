//! Cycles per instruction of the committed binary, through `lee::V03State`
//! with the 10,000,000 per-transaction cap. Deterministic: no signature in the
//! program's own logic depends on randomness.

use antumbra::WAD;
use antumbra_auction_core::{rows, Instruction};
use antumbra_auction_executor_tests::{Chain, Key, AUCTION, GAS_CAP};

fn main() {
    let admin = Key::new(1);
    let bidder = Key::new(2);
    let mut chain = Chain::new(&[admin.id, bidder.id]);
    let (cid, aid) = ([3u8; 32], [4u8; 32]);
    let (min, max, window) = (WAD - WAD / 50, WAD - WAD / 5, 3600);
    let mut row = |chain: &mut Chain, name: &str, ix: Instruction, r, k: &Key| {
        chain.send(&ix, r, &[k]).unwrap_or_else(|e| panic!("{name}: {e}"));
        let c = chain.last_cycles;
        println!("| {name} | {c} | {:.2}% |", c as f64 * 100.0 / GAS_CAP as f64);
    };
    println!("| op | cycles | share of the 10M gas cap |\n|---|---:|---:|");
    row(&mut chain, "init_config", Instruction::InitConfig { config_id: cid, min_multiplier: min, max_multiplier: max, window }, rows::init_config(&AUCTION, &cid, admin.id), &admin);
    let t0 = chain.now;
    row(&mut chain, "start_auction", Instruction::StartAuction { auction_id: aid, config_id: cid, collateral: 10 * WAD, to_raise: 20_000 * WAD, price: 2_000 * WAD, now: t0, min_multiplier: min, max_multiplier: max, window }, rows::start_auction(&AUCTION, &aid, &cid, admin.id), &admin);
    row(&mut chain, "buy_collateral (partial fill)", Instruction::BuyCollateral { auction_id: aid, bid: 2_000 * WAD, now: t0 }, rows::handle(&AUCTION, &aid, bidder.id), &bidder);
    chain.now = t0 + 3_600_000;
    row(&mut chain, "buy_collateral (re-struck, lot exhausted)", Instruction::BuyCollateral { auction_id: aid, bid: 16_000 * WAD, now: t0 + 3_600_000 }, rows::handle(&AUCTION, &aid, bidder.id), &bidder);
    // settle_auction has no successful run to measure: the fill that meets the
    // target or empties the lot settles the auction in the same apply (F2), and
    // a separate settle then refuses with 8005, as it did on v0.2.4.
}
