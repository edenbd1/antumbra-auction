//! Cycles per instruction of the committed binary, through `lee::V03State`
//! with the 10,000,000 per-transaction cap. Deterministic: no signature in the
//! program's own logic depends on randomness.

use antumbra::WAD;
use antumbra_auction_core::{engine::*, rows, Instruction};
use antumbra_auction_executor_tests::{Chain, Key, AUCTION, GAS_CAP};

fn main() {
    let admin = Key::new(1);
    let bidder = Key::new(2);
    let mut chain = Chain::new(&[admin.id, bidder.id]);
    let (cid, aid) = ([3u8; 32], [4u8; 32]);
    let (min, max, window) = (WAD - WAD / 50, WAD - WAD / 5, 3600);
    let row = |chain: &mut Chain, name: &str, ix: Instruction, r, k: &Key| {
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

    // Liquidation, the feed and the debt auction (engine.rs), on the
    // scenario of scripts/e2e-liquidation-v03.sh.
    let keeper = Key::new(5);
    let mut chain2 = Chain::new(&[admin.id, bidder.id, keeper.id]);
    let chain = &mut chain2;
    let (cid, tid, pid) = ([3u8; 32], [0xA; 32], [0x11; 32]);
    let window = 120;
    chain.send(&Instruction::InitConfig { config_id: cid, min_multiplier: min, max_multiplier: max, window }, rows::init_config(&AUCTION, &cid, admin.id), &[&admin]).expect("config");
    let terms = CtypeTerms { config_id: cid, liquidation_ratio: WAD * 3 / 2, penalty: WAD + WAD * 13 / 100, tip: WAD / 50, max_age_ms: 300_000, min_multiplier: min, max_multiplier: max, window };
    row(chain, "init_collateral_type", Instruction::InitCollateralType { ctype_id: tid, terms: terms.clone() }, rows::init_ctype(&AUCTION, &tid, &cid, admin.id), &admin);
    let dterms = DebtTerms { protocol_token: [0xAB; 32], initial_lot: 1_000 * WAD, beg: WAD * 3 / 100, bid_ttl_ms: 60_000, auction_ttl_ms: 1_800_000 };
    row(chain, "init_debt_config", Instruction::InitDebtConfig { config_id: cid, terms: dterms.clone() }, rows::init_debt_config(&AUCTION, &cid, admin.id), &admin);
    row(chain, "open_position", Instruction::OpenPosition { position_id: pid, ctype_id: tid, collateral: 10 * WAD, debt: 12_000 * WAD }, rows::open_position(&AUCTION, &pid, &tid, bidder.id), &bidder);
    let t0 = chain.now;
    row(chain, "update_price (first)", Instruction::UpdatePrice { ctype_id: tid, price: 2_000 * WAD, now: t0 }, rows::update_price(&AUCTION, &tid, admin.id), &admin);
    chain.now = t0 + 60_000;
    row(chain, "update_price (second)", Instruction::UpdatePrice { ctype_id: tid, price: 1_000 * WAD, now: t0 + 60_000 }, rows::update_price(&AUCTION, &tid, admin.id), &admin);
    chain.now = t0 + 90_000;
    let obs = [Observation { price: 2_000 * WAD, at: t0 }, Observation { price: 1_000 * WAD, at: t0 + 60_000 }];
    let price = twap(&obs, t0 + 90_000).expect("a price");
    row(chain, "liquidate (opens the auction)", Instruction::Liquidate { position_id: pid, ctype_id: tid, collateral: 10 * WAD, debt: 12_000 * WAD, price, now: t0 + 90_000, terms }, rows::liquidate(&AUCTION, &pid, &tid, keeper.id), &keeper);
    let aid = liquidation_auction_id(&pid);
    chain.now = t0 + 210_000;
    row(chain, "buy_collateral (buy-out, closes short)", Instruction::BuyCollateral { auction_id: aid, bid: 20_000 * WAD, now: t0 + 210_000 }, rows::handle(&AUCTION, &aid, bidder.id), &bidder);
    let s = chain.auction(&aid);
    row(chain, "book_shortfall (opens the debt auction)", Instruction::BookShortfall { auction_id: aid, config_id: cid, shortfall: s.to_raise - s.raised, now: t0 + 210_000, terms: dterms }, rows::book_shortfall(&AUCTION, &aid, &cid, keeper.id), &keeper);
    row(chain, "debt_bid", Instruction::DebtBid { auction_id: aid, lot: 970 * WAD, now: t0 + 210_000 }, rows::debt_handle(&AUCTION, &aid, keeper.id), &keeper);
    chain.now = t0 + 270_000;
    row(chain, "settle_debt_auction", Instruction::SettleDebtAuction { auction_id: aid, now: t0 + 270_000 }, rows::debt_handle(&AUCTION, &aid, keeper.id), &keeper);
}
