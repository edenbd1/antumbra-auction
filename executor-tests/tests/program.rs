//! The committed binary, through the state machine the v0.3 sequencer applies.
//!
//! Each test drives `artifacts/programs/v0.3/antumbra_auction.bin` with signed
//! transactions at a block timestamp, and reads the auction back from the
//! state. A refusal is the guest panicking with its code, and the state is
//! checked to be byte-identical after it.

use antumbra::WAD;
use antumbra_auction_core::{errors::*, rows, Instruction, START_WINDOW_MS};
use antumbra_auction_executor_tests::{code, Chain, Key, AUCTION, GAS_CAP};

const CID: [u8; 32] = [3; 32];
const AID: [u8; 32] = [4; 32];
const MIN: u128 = WAD - WAD / 50;
const MAX: u128 = WAD - WAD / 5;
const WINDOW: u64 = 3600;

fn init(chain: &mut Chain, admin: &Key, config_id: [u8; 32], min: u128, max: u128, window: u64) -> Result<(), String> {
    chain.send(
        &Instruction::InitConfig {
            config_id,
            min_multiplier: min,
            max_multiplier: max,
            window,
        },
        rows::init_config(&AUCTION, &config_id, admin.id),
        &[admin],
    )
}

fn start(chain: &mut Chain, admin: &Key, auction_id: [u8; 32], now: u64, max: u128) -> Result<(), String> {
    chain.send(
        &Instruction::StartAuction {
            auction_id,
            config_id: CID,
            collateral: 10 * WAD,
            to_raise: 20_000 * WAD,
            price: 2_000 * WAD,
            now,
            min_multiplier: MIN,
            max_multiplier: max,
            window: WINDOW,
        },
        rows::start_auction(&AUCTION, &auction_id, &CID, admin.id),
        &[admin],
    )
}

fn bid(chain: &mut Chain, who: &Key, amount: u128, now: u64) -> Result<(), String> {
    chain.send(
        &Instruction::BuyCollateral {
            auction_id: AID,
            bid: amount,
            now,
        },
        rows::handle(&AUCTION, &AID, who.id),
        &[who],
    )
}

fn settle(chain: &mut Chain, who: &Key, now: u64) -> Result<(), String> {
    chain.send(
        &Instruction::SettleAuction { auction_id: AID, now },
        rows::handle(&AUCTION, &AID, who.id),
        &[who],
    )
}

/// An open auction at the chain's clock; returns the time it opened.
fn opened() -> (Chain, Key, Key, u64) {
    let admin = Key::new(1);
    let bidder = Key::new(2);
    let mut chain = Chain::new(&[admin.id, bidder.id]);
    init(&mut chain, &admin, CID, MIN, MAX, WINDOW).expect("config");
    let t0 = chain.now;
    start(&mut chain, &admin, AID, t0, MAX).expect("auction");
    (chain, admin, bidder, t0)
}

fn refused(r: Result<(), String>, want: u32) {
    let e = r.expect_err("expected a refusal");
    assert_eq!(code(&e), Some(want), "{e}");
}

#[test]
fn the_v024_testnet_run_reproduces_on_the_v03_state_machine() {
    let (mut chain, _admin, bidder, t0) = opened();
    bid(&mut chain, &bidder, 2_000 * WAD, t0).unwrap();
    let first = chain.auction(&AID).collateral_sold;
    assert_eq!(first, 1_020_408_163_265_306_122);
    chain.now = t0 + 1_800_000;
    bid(&mut chain, &bidder, 2_000 * WAD, t0 + 1_800_000).unwrap();
    assert_eq!(chain.auction(&AID).collateral_sold - first, 1_123_595_505_617_977_528, "the discount escalated (F2)");
    chain.now = t0 + 3_600_000;
    bid(&mut chain, &bidder, 16_000 * WAD, t0 + 3_600_000).unwrap();
    let s = chain.auction(&AID);
    assert_eq!(s.collateral_left, 0);
    assert_eq!(s.collateral_sold + s.collateral_left, s.collateral_seized, "R6 from the stored counters");
    assert_eq!(s.raised, 16_569_594_129_786_746_160_000, "re-struck, and the shortfall booked");
    assert_eq!(s.settled, 1, "an exhausted lot closes itself");
}

#[test]
fn a_bid_cannot_be_priced_at_a_time_the_chain_has_not_reached() {
    let (mut chain, _admin, bidder, t0) = opened();
    let before = chain.auction_bytes(&AID);
    // Dated an hour ahead, where the discount is deepest: the sequencer will
    // not include it in a block stamped now.
    let e = bid(&mut chain, &bidder, 2_000 * WAD, t0 + 3_600_000).expect_err("a future-dated bid");
    assert_eq!(code(&e), None, "refused by the validity window, before the program: {e}");
    assert_eq!(chain.auction_bytes(&AID), before);
}

#[test]
fn an_opening_cannot_be_back_dated_beyond_its_window() {
    let admin = Key::new(1);
    let mut chain = Chain::new(&[admin.id]);
    init(&mut chain, &admin, CID, MIN, MAX, WINDOW).unwrap();
    let now = chain.now;
    let e = start(&mut chain, &admin, AID, now - START_WINDOW_MS, MAX).expect_err("back-dated opening");
    assert_eq!(code(&e), None, "{e}");
    start(&mut chain, &admin, AID, now - START_WINDOW_MS + 1, MAX).expect("inside the window");
}

#[test]
fn only_the_configs_admin_opens_an_auction() {
    let (mut chain, _admin, bidder, t0) = opened();
    refused(start(&mut chain, &bidder, [5; 32], t0, MAX), E_NOT_ADMIN);
}

#[test]
fn an_auction_cannot_carry_a_schedule_its_config_does_not_hold() {
    let (mut chain, admin, _bidder, t0) = opened();
    refused(start(&mut chain, &admin, [5; 32], t0, MAX - 1), E_CONFIG_MISMATCH);
}

#[test]
fn ids_are_written_once() {
    let (mut chain, admin, _bidder, t0) = opened();
    refused(init(&mut chain, &admin, CID, MIN, MAX, WINDOW), E_EXISTS);
    let before = chain.auction_bytes(&AID);
    refused(start(&mut chain, &admin, AID, t0, MAX), E_EXISTS);
    assert_eq!(chain.auction_bytes(&AID), before);
}

#[test]
fn time_never_goes_backwards() {
    let (mut chain, _admin, bidder, t0) = opened();
    chain.now = t0 + 1_800_000;
    bid(&mut chain, &bidder, 2_000 * WAD, t0 + 1_800_000).unwrap();
    let before = chain.auction_bytes(&AID);
    refused(bid(&mut chain, &bidder, WAD, t0 + 60_000), E_TIME_WENT_BACKWARDS);
    refused(settle(&mut chain, &bidder, t0 + 60_000), E_TIME_WENT_BACKWARDS);
    assert_eq!(chain.auction_bytes(&AID), before);
}

#[test]
fn transitions_are_strict() {
    let (mut chain, _admin, bidder, t0) = opened();
    bid(&mut chain, &bidder, 2_000 * WAD, t0).unwrap();
    refused(settle(&mut chain, &bidder, t0), E_NOT_FINISHED);
    chain.now = t0 + 3_600_000;
    bid(&mut chain, &bidder, 1_000_000 * WAD, t0 + 3_600_000).unwrap();
    assert_eq!(chain.auction(&AID).settled, 1);
    refused(settle(&mut chain, &bidder, t0 + 3_600_000), E_ALREADY_SETTLED);
    refused(bid(&mut chain, &bidder, WAD, t0 + 3_600_000), E_ALREADY_SETTLED);
}

#[test]
fn a_bid_that_buys_nothing_is_refused() {
    let (mut chain, _admin, bidder, t0) = opened();
    refused(bid(&mut chain, &bidder, 1, t0), E_BID_TOO_SMALL);
}

#[test]
fn a_bid_on_an_auction_that_does_not_exist_is_refused() {
    let bidder = Key::new(2);
    let mut chain = Chain::new(&[bidder.id]);
    let now = chain.now;
    refused(bid(&mut chain, &bidder, WAD, now), E_NOT_ANCHORED);
}

#[test]
fn a_bad_schedule_is_refused() {
    let admin = Key::new(1);
    let mut chain = Chain::new(&[admin.id]);
    refused(init(&mut chain, &admin, CID, MAX, MIN, WINDOW), E_BAD_SCHEDULE);
}

#[test]
fn an_empty_lot_is_refused() {
    let admin = Key::new(1);
    let mut chain = Chain::new(&[admin.id]);
    init(&mut chain, &admin, CID, MIN, MAX, WINDOW).unwrap();
    let now = chain.now;
    let r = chain.send(
        &Instruction::StartAuction {
            auction_id: AID,
            config_id: CID,
            collateral: 0,
            to_raise: 20_000 * WAD,
            price: 2_000 * WAD,
            now,
            min_multiplier: MIN,
            max_multiplier: MAX,
            window: WINDOW,
        },
        rows::start_auction(&AUCTION, &AID, &CID, admin.id),
        &[&admin],
    );
    refused(r, E_BAD_STATE);
}

#[test]
fn a_signer_row_without_a_signature_is_refused() {
    let (mut chain, admin, bidder, t0) = opened();
    // The bidder's row is named, but only the admin signs.
    let r = chain.send(
        &Instruction::BuyCollateral {
            auction_id: AID,
            bid: WAD,
            now: t0,
        },
        rows::handle(&AUCTION, &AID, bidder.id),
        &[&admin],
    );
    refused(r, E_NOT_SIGNED);
}

#[test]
fn the_wrong_rows_are_refused() {
    let (mut chain, _admin, bidder, t0) = opened();
    let mut r = rows::handle(&AUCTION, &AID, bidder.id);
    r.pop();
    refused(
        chain.send(&Instruction::SettleAuction { auction_id: AID, now: t0 }, r, &[&bidder]),
        E_ACCOUNTS,
    );
}

#[test]
fn every_reachable_refusal_code_fires_on_chain() {
    // 8006 (nothing left) and 8008 (arithmetic) are defensive: a fill that
    // empties the lot or meets the target settles the auction in the same
    // apply, so 8005 fires first, and a schedule and price that pass creation
    // cannot make the quote fail. Both stay as guards; every other code is
    // fired by one of the tests above, and this one fires them again in a row.
    let (mut chain, admin, bidder, t0) = opened();
    let mut fired = vec![];
    let mut hit = |r: Result<(), String>| fired.push(code(&r.expect_err("refusal")).unwrap());
    hit(bid(&mut chain, &bidder, 1, t0));
    hit(start(&mut chain, &bidder, [5; 32], t0, MAX));
    hit(start(&mut chain, &admin, [5; 32], t0, MAX - 1));
    hit(start(&mut chain, &admin, AID, t0, MAX));
    hit(init(&mut chain, &admin, [6; 32], MAX, MIN, WINDOW));
    hit(settle(&mut chain, &bidder, t0));
    hit(chain.send(
        &Instruction::BuyCollateral {
            auction_id: AID,
            bid: WAD,
            now: t0,
        },
        rows::handle(&AUCTION, &AID, bidder.id),
        &[&admin],
    ));
    let mut short = rows::handle(&AUCTION, &AID, bidder.id);
    short.pop();
    hit(chain.send(&Instruction::SettleAuction { auction_id: AID, now: t0 }, short, &[&bidder]));
    hit(chain.send(
        &Instruction::BuyCollateral {
            auction_id: [9; 32],
            bid: WAD,
            now: t0,
        },
        rows::handle(&AUCTION, &[9; 32], bidder.id),
        &[&bidder],
    ));
    hit(chain.send(
        &Instruction::StartAuction {
            auction_id: [7; 32],
            config_id: CID,
            collateral: 0,
            to_raise: 1,
            price: 1,
            now: t0,
            min_multiplier: MIN,
            max_multiplier: MAX,
            window: WINDOW,
        },
        rows::start_auction(&AUCTION, &[7; 32], &CID, admin.id),
        &[&admin],
    ));
    chain.now = t0 + 1_800_000;
    bid(&mut chain, &bidder, 2_000 * WAD, t0 + 1_800_000).unwrap();
    hit(bid(&mut chain, &bidder, WAD, t0 + 60_000));
    chain.now = t0 + 3_600_000;
    bid(&mut chain, &bidder, 1_000_000 * WAD, t0 + 3_600_000).unwrap();
    hit(settle(&mut chain, &bidder, t0 + 3_600_000));
    fired.sort_unstable();
    fired.dedup();
    let reachable: Vec<u32> = ALL
        .iter()
        .map(|(c, _)| *c)
        .filter(|c| ![E_NOTHING_LEFT, E_ARITHMETIC].contains(c))
        // 8015 onwards belong to liquidation, the feed and the debt auction;
        // tests/engine.rs fires each of them by name.
        .filter(|c| *c < E_FEED_STALE)
        .collect();
    assert_eq!(fired, reachable);
}

#[test]
fn every_instruction_fits_far_under_the_gas_cap() {
    let (mut chain, _admin, bidder, t0) = opened();
    bid(&mut chain, &bidder, 2_000 * WAD, t0).unwrap();
    assert!(chain.last_cycles > 0 && chain.last_cycles < GAS_CAP / 10, "{}", chain.last_cycles);
}
