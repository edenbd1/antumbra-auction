//! Host tests of liquidation, the feed and the debt auction, without a zkVM.
//! The executor tests run the same logic inside the committed binary.

use super::*;
use std::panic::{catch_unwind, AssertUnwindSafe};

const WAD: u128 = antumbra::WAD;
const ME: AccountId = AccountId::new([7; 32]);
const ADMIN: AccountId = AccountId::new([1; 32]);
const KEEPER: AccountId = AccountId::new([2; 32]);
const OTHER: AccountId = AccountId::new([5; 32]);
const CID: [u8; 32] = [3; 32];
const TID: [u8; 32] = [8; 32];
const PID: [u8; 32] = [9; 32];
const T0: u64 = 1_700_000_000_000;

fn refusal<T>(f: impl FnOnce() -> T) -> u32 {
    let msg = match catch_unwind(AssertUnwindSafe(f)) {
        Ok(_) => panic!("expected a refusal"),
        Err(e) => e
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default(),
    };
    msg[1..5].parse().unwrap_or_else(|_| panic!("no code in {msg:?}"))
}

fn meta(account_id: AccountId, signs: bool) -> AccountMeta {
    AccountMeta {
        account_id,
        program_account_id: ME,
        is_authorized: signs,
    }
}

fn input(accounts: Vec<AccountMeta>, ix: &Instruction) -> PlanInput {
    PlanInput {
        self_account_id: ME,
        caller_account_id: None,
        accounts,
        instruction_data: borsh::to_vec(ix).unwrap(),
    }
}

fn terms() -> CtypeTerms {
    CtypeTerms {
        config_id: CID,
        liquidation_ratio: WAD * 3 / 2,
        penalty: WAD + WAD * 13 / 100,
        tip: WAD / 50,
        max_age_ms: 300_000,
        min_multiplier: WAD - WAD / 50,
        max_multiplier: WAD - WAD / 5,
        window: 120,
    }
}

fn obs(v: &[(u128, u64)]) -> Vec<Observation> {
    v.iter().map(|&(p, at)| Observation { price: p * WAD, at }).collect()
}

fn ctype(o: &[(u128, u64)]) -> CollateralType {
    CollateralType {
        admin: ADMIN,
        terms: terms(),
        observations: obs(o),
    }
}

fn position() -> PositionState {
    PositionState {
        ctype_id: TID,
        owner: OTHER,
        collateral: 10 * WAD,
        debt: 12_000 * WAD,
        liquidated: 0,
        liquidator: OTHER,
        tip: 0,
        liquidated_at: 0,
    }
}

fn liquidate_ix(price: u128, debt: u128) -> Instruction {
    Instruction::Liquidate {
        position_id: PID,
        ctype_id: TID,
        collateral: 10 * WAD,
        debt,
        price,
        now: T0,
        terms: terms(),
    }
}

fn liquidate_rows(liquidator: AccountId, signs: bool) -> Vec<AccountMeta> {
    vec![
        meta(position_account(&ME, &PID), false),
        meta(ctype_account(&ME, &TID), false),
        meta(auction_account(&ME, &liquidation_auction_id(&PID)), false),
        meta(liquidator, signs),
    ]
}

// ------------------------------------------------------------------ the feed

#[test]
fn the_time_weighted_price_weighs_each_print_for_as_long_as_it_stood() {
    // 2000 for 60 s, then 1000 for 30 s: (2000·60 + 1000·30) / 90 = 1666.66…
    let o = obs(&[(2000, T0), (1000, T0 + 60_000)]);
    assert_eq!(twap(&o, T0 + 90_000), Some(1_666_666_666_666_666_666_666));
    // A print posted at `now` weighs nothing yet: one bad print alone moves nothing.
    assert_eq!(twap(&o, T0 + 60_000), Some(2000 * WAD));
    // One observation is its own average.
    assert_eq!(twap(&obs(&[(2000, T0)]), T0 + 5_000), Some(2000 * WAD));
    // No price, or a time before the last price: nothing to say.
    assert_eq!(twap(&[], T0), None);
    assert_eq!(twap(&o, T0 + 59_999), None);
}

#[test]
fn staleness_is_judged_at_the_end_of_the_transactions_window() {
    let o = obs(&[(2000, T0)]);
    let max = 300_000;
    // Fresh while now + window - at <= max_age.
    let last_fresh = T0 + max - START_WINDOW_MS;
    assert!(!feed_is_stale(&o, last_fresh, max));
    assert!(feed_is_stale(&o, last_fresh + 1, max));
    assert!(feed_is_stale(&[], T0, max), "a type with no price is paused");
}

#[test]
fn only_the_admin_writes_the_feed_forwards_and_it_keeps_four_prices() {
    assert_eq!(refusal(|| push_price(ctype(&[]), KEEPER, WAD, T0)), E_NOT_ADMIN);
    assert_eq!(refusal(|| push_price(ctype(&[(1, T0)]), ADMIN, WAD, T0)), E_TIME_WENT_BACKWARDS);
    assert_eq!(refusal(|| push_price(ctype(&[]), ADMIN, 0, T0)), E_BAD_STATE);
    let mut c = ctype(&[]);
    for i in 0..6u64 {
        c = push_price(c, ADMIN, (1000 + u128::from(i)) * WAD, T0 + i);
    }
    assert_eq!(c.observations.len(), OBSERVATIONS);
    assert_eq!(c.observations[0].price, 1002 * WAD, "the oldest are dropped");
}

#[test]
fn the_feed_check_refuses_stale_mismatched_and_misquoted() {
    let c = ctype(&[(2000, T0), (1000, T0 + 60_000)]);
    let now = T0 + 90_000;
    let avg = twap(&c.observations, now).unwrap();
    check_feed(&c, &terms(), avg, now);
    assert_eq!(refusal(|| check_feed(&c, &terms(), avg - 1, now)), E_PRICE_MISMATCH);
    assert_eq!(refusal(|| check_feed(&c, &terms(), 1000 * WAD, now)), E_PRICE_MISMATCH, "the last print is not the price");
    let lax = CtypeTerms { max_age_ms: 10_000_000, ..terms() };
    assert_eq!(refusal(|| check_feed(&c, &lax, avg, now)), E_CONFIG_MISMATCH);
    let late = T0 + 60_000 + 300_000;
    let avg_late = twap(&c.observations, late).unwrap();
    assert_eq!(refusal(|| check_feed(&c, &terms(), avg_late, late)), E_FEED_STALE);
}

#[test]
fn a_stale_feed_refuses_through_apply_and_writes_nothing() {
    let stored = borsh::to_vec(&ctype(&[(2000, T0)])).unwrap();
    let late = T0 + 1_000_000;
    let e = Effect::CheckFeed { terms: terms(), price: 2000 * WAD, now: late };
    assert_eq!(refusal(|| apply(e, &stored)), E_FEED_STALE);
    let ok = Effect::CheckFeed { terms: terms(), price: 2000 * WAD, now: T0 + 1_000 };
    assert_eq!(apply(ok, &stored), None, "the check never writes the type");
}

// --------------------------------------------------------------- liquidation

#[test]
fn any_signer_liquidates_an_unsafe_position_and_none_a_safe_one() {
    // 10 units owing 12000 at 150% needs a price of 1800.
    for who in [KEEPER, OTHER, ADMIN] {
        let ix = liquidate_ix(1799 * WAD, 12_000 * WAD);
        let p = plan(&input(liquidate_rows(who, true), &ix), ix);
        assert_eq!(p.output().effects.len(), 3, "feed check, seizure, auction");
    }
    let ix = liquidate_ix(1800 * WAD, 12_000 * WAD);
    assert_eq!(refusal(|| plan(&input(liquidate_rows(KEEPER, true), &ix), ix.clone())), E_POSITION_SAFE);
    let ix = liquidate_ix(1000 * WAD, 12_000 * WAD);
    assert_eq!(refusal(|| plan(&input(liquidate_rows(KEEPER, false), &ix), ix.clone())), E_NOT_SIGNED);
}

#[test]
fn a_liquidation_opens_its_own_auction_for_debt_plus_penalty_less_the_tip() {
    let ix = liquidate_ix(1500 * WAD, 12_000 * WAD);
    let mut rows = liquidate_rows(KEEPER, true);
    rows[2] = meta(auction_account(&ME, &PID), false);
    assert_eq!(refusal(|| plan(&input(rows, &ix), ix.clone())), E_NOT_ANCHORED, "only the derived auction");
    let (tip, lot, to_raise) = liquidation_split(10 * WAD, 12_000 * WAD, &terms());
    assert_eq!(tip, WAD / 5, "2% of 10 units");
    assert_eq!(lot, 10 * WAD - WAD / 5);
    assert_eq!(to_raise, 13_560 * WAD, "12000 plus 13%");
    let p = plan(&input(liquidate_rows(KEEPER, true), &ix), ix);
    let w = &p.output().timestamp_validity_window;
    assert!(w.is_valid_for(T0) && !w.is_valid_for(T0 - 1) && !w.is_valid_for(T0 + START_WINDOW_MS));
}

#[test]
fn a_position_is_seized_once_and_only_as_stored() {
    let p = seize(position(), TID, 10 * WAD, 12_000 * WAD, KEEPER, 7, T0);
    assert_eq!((p.liquidated, p.liquidator, p.tip, p.liquidated_at), (1, KEEPER, 7, T0));
    assert_eq!(refusal(|| seize(p.clone(), TID, 10 * WAD, 12_000 * WAD, OTHER, 7, T0)), E_ALREADY_LIQUIDATED);
    assert_eq!(refusal(|| seize(position(), TID, 10 * WAD, 11_999 * WAD, KEEPER, 7, T0)), E_POSITION_MISMATCH);
    assert_eq!(refusal(|| seize(position(), [0; 32], 10 * WAD, 12_000 * WAD, KEEPER, 7, T0)), E_POSITION_MISMATCH);
}

#[test]
fn bad_collateral_type_terms_are_refused_at_creation() {
    for bad in [
        CtypeTerms { liquidation_ratio: WAD - 1, ..terms() },
        CtypeTerms { penalty: WAD - 1, ..terms() },
        CtypeTerms { tip: WAD, ..terms() },
        CtypeTerms { max_age_ms: 0, ..terms() },
    ] {
        assert_eq!(refusal(|| validate_ctype_terms(&bad)), E_BAD_STATE);
    }
    assert_eq!(refusal(|| validate_ctype_terms(&CtypeTerms { min_multiplier: 0, ..terms() })), E_BAD_SCHEDULE);
}

// ---------------------------------------------------------------- debt auction

fn closed_short() -> AuctionState {
    AuctionState {
        config_id: CID,
        collateral_seized: 10 * WAD,
        collateral_left: 0,
        collateral_sold: 10 * WAD,
        to_raise: 20_000 * WAD,
        raised: 16_000 * WAD,
        price: 2000 * WAD,
        opened_at: T0,
        last_seen: T0,
        settled: 1,
        min_multiplier: WAD,
        max_multiplier: WAD,
        window: 0,
    }
}

fn debt(lot: u128) -> DebtAuction {
    DebtAuction {
        source_auction: [4; 32],
        config_id: CID,
        protocol_token: [0xAB; 32],
        coin: 4_000 * WAD,
        lot,
        beg: WAD * 3 / 100,
        bid_ttl_ms: 30_000,
        has_bid: 0,
        bidder: ADMIN,
        bid_expiry: T0 + 600_000,
        end: T0 + 600_000,
        last_seen: T0,
        settled: 0,
    }
}

#[test]
fn only_a_closed_auction_short_of_its_target_has_a_shortfall_to_book() {
    check_shortfall(&closed_short(), &CID, 4_000 * WAD);
    assert_eq!(refusal(|| check_shortfall(&closed_short(), &CID, 3_999 * WAD)), E_NO_SHORTFALL);
    assert_eq!(refusal(|| check_shortfall(&closed_short(), &[0; 32], 4_000 * WAD)), E_CONFIG_MISMATCH);
    let open = AuctionState { settled: 0, collateral_left: 1, ..closed_short() };
    assert_eq!(refusal(|| check_shortfall(&open, &CID, 4_000 * WAD)), E_NO_SHORTFALL);
    let met = AuctionState { raised: 20_000 * WAD, ..closed_short() };
    assert_eq!(refusal(|| check_shortfall(&met, &CID, 0)), E_NO_SHORTFALL);
}

#[test]
fn a_shortfall_is_booked_once() {
    let d = apply(Effect::CreateDebtAuction(debt(1000 * WAD)), &[]).unwrap();
    assert_eq!(refusal(|| apply(Effect::CreateDebtAuction(debt(1000 * WAD)), &d)), E_EXISTS);
}

#[test]
fn a_debt_bid_must_shrink_the_lot_by_the_minimum_and_before_expiry() {
    let d = debt(1000 * WAD);
    assert_eq!(refusal(|| debt_bid(d.clone(), KEEPER, 971 * WAD, T0 + 1)), E_NOT_AN_IMPROVEMENT);
    let d = debt_bid(d, KEEPER, 970 * WAD, T0 + 1);
    assert_eq!((d.lot, d.bidder, d.has_bid, d.bid_expiry), (970 * WAD, KEEPER, 1, T0 + 30_001));
    assert_eq!(refusal(|| debt_bid(d.clone(), OTHER, 970 * WAD, T0 + 2)), E_NOT_AN_IMPROVEMENT, "an unchanged lot");
    assert_eq!(refusal(|| debt_bid(d.clone(), OTHER, 900 * WAD, T0)), E_TIME_WENT_BACKWARDS);
    assert_eq!(refusal(|| debt_bid(d.clone(), OTHER, 900 * WAD, T0 + 30_001)), E_CLOSED, "the standing bid has won");
    assert_eq!(refusal(|| debt_settle(d.clone(), T0 + 30_000)), E_NOT_FINISHED);
    let s = debt_settle(d, T0 + 30_001);
    assert_eq!(s.settled, 1);
    assert_eq!(refusal(|| debt_settle(s.clone(), T0 + 40_000)), E_ALREADY_SETTLED);
    assert_eq!(refusal(|| debt_bid(s, OTHER, WAD, T0 + 40_000)), E_ALREADY_SETTLED);
}

#[test]
fn a_debt_auction_with_no_bid_does_not_settle_and_a_late_bid_is_refused() {
    let d = debt(1000 * WAD);
    assert_eq!(refusal(|| debt_settle(d.clone(), T0 + 600_000)), E_NO_BID);
    assert_eq!(refusal(|| debt_bid(d, KEEPER, 900 * WAD, T0 + 600_000)), E_CLOSED);
}

#[test]
fn a_debt_bid_cannot_be_back_dated_past_its_window() {
    let ix = Instruction::DebtBid { auction_id: [4; 32], lot: WAD, now: T0 };
    let rows = vec![meta(debt_auction_account(&ME, &[4; 32]), false), meta(KEEPER, true)];
    let p = plan(&input(rows, &ix), ix);
    let w = &p.output().timestamp_validity_window;
    assert!(w.is_valid_for(T0) && !w.is_valid_for(T0 + START_WINDOW_MS));
}

#[test]
fn the_new_accounts_have_their_own_seed_spaces() {
    let id = [4; 32];
    let all = [
        config_account(&ME, &id),
        auction_account(&ME, &id),
        ctype_account(&ME, &id),
        position_account(&ME, &id),
        debt_config_account(&ME, &id),
        debt_auction_account(&ME, &id),
    ];
    for i in 0..all.len() {
        for j in i + 1..all.len() {
            assert_ne!(all[i], all[j]);
        }
    }
    assert_ne!(liquidation_auction_id(&id), id);
}
