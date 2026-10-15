//! Host tests of plan and apply, without a zkVM. The executor tests run the
//! same logic inside the committed binary through the LEE state machine.
//!
//! The scenario is the one the v0.2.4 program ran on the public testnet: a 2%
//! discount at open growing to 20% over an hour, ten units of collateral priced
//! at 2000, raising 20000. Its figures are pinned to the base unit, so the port
//! is checked against the deployment it replaces.

use super::*;
use std::panic::{catch_unwind, AssertUnwindSafe};

const WAD: u128 = antumbra::WAD;
const ME: AccountId = AccountId::new([7; 32]);
const ADMIN: AccountId = AccountId::new([1; 32]);
const BIDDER: AccountId = AccountId::new([2; 32]);
const CID: [u8; 32] = [3; 32];
const AID: [u8; 32] = [4; 32];
const MIN: u128 = WAD - WAD / 50;
const MAX: u128 = WAD - WAD / 5;
const WINDOW: u64 = 3600;
const T0: u64 = 1_700_000_000_000;

fn refusal<T>(f: impl FnOnce() -> T) -> String {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(_) => panic!("expected a refusal"),
        Err(e) => e
            .downcast_ref::<String>()
            .cloned()
            .or_else(|| e.downcast_ref::<&str>().map(|s| s.to_string()))
            .unwrap_or_default(),
    }
}

fn code(msg: &str) -> u32 {
    msg[1..5].parse().unwrap_or_else(|_| panic!("no code in {msg:?}"))
}

fn meta(account_id: AccountId, program: AccountId, signs: bool) -> AccountMeta {
    AccountMeta {
        account_id,
        program_account_id: program,
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

fn opened() -> AuctionState {
    AuctionState {
        config_id: CID,
        collateral_seized: 10 * WAD,
        collateral_left: 10 * WAD,
        collateral_sold: 0,
        to_raise: 20_000 * WAD,
        raised: 0,
        price: 2_000 * WAD,
        opened_at: T0,
        last_seen: T0,
        settled: 0,
        min_multiplier: MIN,
        max_multiplier: MAX,
        window: WINDOW,
    }
}

fn config() -> Config {
    Config {
        admin: ADMIN,
        min_multiplier: MIN,
        max_multiplier: MAX,
        window: WINDOW,
    }
}

fn start_ix(now: u64) -> Instruction {
    Instruction::StartAuction {
        auction_id: AID,
        config_id: CID,
        collateral: 10 * WAD,
        to_raise: 20_000 * WAD,
        price: 2_000 * WAD,
        now,
        min_multiplier: MIN,
        max_multiplier: MAX,
        window: WINDOW,
    }
}

fn start_rows(admin_signs: bool) -> Vec<AccountMeta> {
    vec![
        meta(auction_account(&ME, &AID), ME, false),
        meta(config_account(&ME, &CID), ME, false),
        meta(ADMIN, ME, admin_signs),
    ]
}

#[test]
fn the_v024_testnet_run_reproduces_to_the_base_unit() {
    let s = opened();
    let a = buy(s, 2_000 * WAD, T0);
    assert_eq!(a.collateral_sold, 1_020_408_163_265_306_122);
    let b = buy(a.clone(), 2_000 * WAD, T0 + 1_800_000);
    assert_eq!(b.collateral_sold - a.collateral_sold, 1_123_595_505_617_977_528);
    let c = buy(b, 16_000 * WAD, T0 + 3_600_000);
    assert_eq!(c.collateral_sold, 10 * WAD);
    assert_eq!(c.collateral_left, 0);
    assert_eq!(c.raised, 16_569_594_129_786_746_160_000);
    assert_eq!(c.settled, 1, "an exhausted lot settles itself (F2)");
    assert_eq!(c.collateral_sold + c.collateral_left, c.collateral_seized, "R6");
}

#[test]
fn the_same_bid_later_buys_more_collateral() {
    let early = buy(opened(), 2_000 * WAD, T0).collateral_sold;
    let late = buy(opened(), 2_000 * WAD, T0 + 1_800_000).collateral_sold;
    assert!(late > early);
}

#[test]
fn reaching_the_target_settles_early_and_never_overshoots() {
    let s = buy(opened(), 1_000_000 * WAD, T0 + 3_600_000);
    assert_eq!(s.settled, 1);
    assert!(s.raised <= s.to_raise);
    let mut small = opened();
    small.collateral_seized = 1_000 * WAD;
    small.collateral_left = 1_000 * WAD;
    let t = buy(small, 25_000 * WAD, T0);
    assert_eq!(t.raised, t.to_raise, "the bid is clamped to what is left to raise");
    assert_eq!(t.settled, 1);
}

#[test]
fn a_clock_earlier_than_one_honoured_is_refused() {
    let s = buy(opened(), 2_000 * WAD, T0 + 1_800_000);
    assert_eq!(code(&refusal(|| buy(s.clone(), WAD, T0 + 60_000))), E_TIME_WENT_BACKWARDS);
    assert_eq!(code(&refusal(|| settle(s, T0 + 60_000))), E_TIME_WENT_BACKWARDS);
}

#[test]
fn settling_an_unfinished_auction_is_refused_and_settled_is_terminal() {
    let s = buy(opened(), 2_000 * WAD, T0);
    assert_eq!(code(&refusal(|| settle(s, T0))), E_NOT_FINISHED);
    let done = buy(opened(), 1_000_000 * WAD, T0 + 3_600_000);
    assert_eq!(code(&refusal(|| settle(done.clone(), T0 + 3_600_000))), E_ALREADY_SETTLED);
    assert_eq!(code(&refusal(|| buy(done, WAD, T0 + 3_600_000))), E_ALREADY_SETTLED);
}

#[test]
fn a_bid_that_buys_nothing_is_refused() {
    assert_eq!(code(&refusal(|| buy(opened(), 1, T0))), E_BID_TOO_SMALL);
}

#[test]
fn a_bad_schedule_is_refused_at_creation() {
    for (min, max) in [(0, 0), (WAD + 1, MAX), (MAX, MIN)] {
        let ix = Instruction::InitConfig {
            config_id: CID,
            min_multiplier: min,
            max_multiplier: max,
            window: WINDOW,
        };
        let rows = vec![meta(config_account(&ME, &CID), ME, false), meta(ADMIN, ME, true)];
        assert_eq!(code(&refusal(|| plan(&input(rows, &ix), ix.clone()))), E_BAD_SCHEDULE);
    }
}

#[test]
fn an_id_is_created_once() {
    let bytes = apply(Effect::CreateConfig(config()), &[]).unwrap();
    assert_eq!(code(&refusal(|| apply(Effect::CreateConfig(config()), &bytes))), E_EXISTS);
    let a = apply(Effect::CreateAuction(opened()), &[]).unwrap();
    assert_eq!(code(&refusal(|| apply(Effect::CreateAuction(opened()), &a))), E_EXISTS);
}

#[test]
fn the_config_check_never_writes_and_refuses_a_mismatch() {
    let stored = borsh::to_vec(&config()).unwrap();
    let ok = Effect::CheckConfig {
        admin: ADMIN,
        min_multiplier: MIN,
        max_multiplier: MAX,
        window: WINDOW,
    };
    assert_eq!(apply(ok, &stored), None);
    let other_admin = Effect::CheckConfig {
        admin: BIDDER,
        min_multiplier: MIN,
        max_multiplier: MAX,
        window: WINDOW,
    };
    assert_eq!(code(&refusal(|| apply(other_admin, &stored))), E_NOT_ADMIN);
    let deeper = Effect::CheckConfig {
        admin: ADMIN,
        min_multiplier: MIN,
        max_multiplier: MAX - 1,
        window: WINDOW,
    };
    assert_eq!(code(&refusal(|| apply(deeper, &stored))), E_CONFIG_MISMATCH);
    let none = Effect::CheckConfig {
        admin: ADMIN,
        min_multiplier: MIN,
        max_multiplier: MAX,
        window: WINDOW,
    };
    assert_eq!(code(&refusal(|| apply(none, &[]))), E_NOT_ANCHORED);
}

#[test]
fn a_bid_on_an_auction_that_does_not_exist_is_refused() {
    assert_eq!(
        code(&refusal(|| apply(Effect::Buy { bid: WAD, now: T0 }, &[]))),
        E_NOT_ANCHORED
    );
}

#[test]
fn start_needs_a_signer_the_right_rows_and_sets_a_bounded_window() {
    let ix = start_ix(T0);
    assert_eq!(code(&refusal(|| plan(&input(start_rows(false), &ix), ix.clone()))), E_NOT_SIGNED);
    let mut swapped = start_rows(true);
    swapped.swap(0, 1);
    assert_eq!(code(&refusal(|| plan(&input(swapped, &ix), ix.clone()))), E_NOT_ANCHORED);
    let p = plan(&input(start_rows(true), &ix), ix.clone());
    let w = &p.output().timestamp_validity_window;
    assert!(w.is_valid_for(T0) && w.is_valid_for(T0 + START_WINDOW_MS - 1));
    assert!(!w.is_valid_for(T0 - 1) && !w.is_valid_for(T0 + START_WINDOW_MS));
}

#[test]
fn a_bid_cannot_be_dated_after_the_chains_clock() {
    let ix = Instruction::BuyCollateral {
        auction_id: AID,
        bid: WAD,
        now: T0 + 3_600_000,
    };
    let rows = vec![meta(auction_account(&ME, &AID), ME, false), meta(BIDDER, ME, true)];
    let p = plan(&input(rows, &ix), ix);
    let w = &p.output().timestamp_validity_window;
    assert!(!w.is_valid_for(T0), "a block before the bid's time cannot include it");
    assert!(w.is_valid_for(T0 + 3_600_000) && w.is_valid_for(u64::MAX));
}

#[test]
fn the_wrong_number_of_rows_is_refused() {
    let ix = Instruction::SettleAuction { auction_id: AID, now: T0 };
    let rows = vec![meta(auction_account(&ME, &AID), ME, false)];
    assert_eq!(code(&refusal(|| plan(&input(rows, &ix), ix.clone()))), E_ACCOUNTS);
}

#[test]
fn configs_and_auctions_do_not_share_a_seed_space() {
    assert_ne!(config_account(&ME, &AID), auction_account(&ME, &AID));
}

#[test]
fn every_code_is_listed_once() {
    let mut codes: Vec<u32> = errors::ALL.iter().map(|(c, _)| *c).collect();
    codes.sort_unstable();
    codes.dedup();
    assert_eq!(codes.len(), errors::ALL.len());
}
