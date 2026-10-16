//! Liquidation, the feed and the debt auction, in the committed binary through
//! `lee::V03State`: F1 (three different signers liquidate), R2/R4 (a stale
//! feed pauses one collateral type while the other keeps liquidating), R5
//! (bids on the stale type's auction continue) and F4 (a closed auction's
//! shortfall booked into a debt auction, bid down and settled).

use antumbra::WAD;
use antumbra_auction_core::{
    engine::*, errors::*, rows, AuctionState, Instruction, START_WINDOW_MS,
};
use antumbra_auction_executor_tests::{code, Chain, Key, AUCTION};
use borsh::BorshDeserialize;

const CID: [u8; 32] = [3; 32];
const A: [u8; 32] = [0xA; 32];
const B: [u8; 32] = [0xB; 32];
const MIN: u128 = WAD - WAD / 50;
const MAX: u128 = WAD - WAD / 5;

fn terms(max_age_ms: u64) -> CtypeTerms {
    CtypeTerms {
        config_id: CID,
        liquidation_ratio: WAD * 3 / 2,
        penalty: WAD + WAD * 13 / 100,
        tip: WAD / 50,
        max_age_ms,
        min_multiplier: MIN,
        max_multiplier: MAX,
        window: 120,
    }
}

fn debt_terms() -> DebtTerms {
    DebtTerms {
        protocol_token: *b"mock-protocol-token/000000000000",
        initial_lot: 1_000 * WAD,
        beg: WAD * 3 / 100,
        bid_ttl_ms: 30_000,
        auction_ttl_ms: 600_000,
    }
}

struct World {
    chain: Chain,
    admin: Key,
    owner: Key,
    keepers: [Key; 3],
    bidder: Key,
}

fn read<T: BorshDeserialize>(chain: &Chain, id: lee::AccountId) -> T {
    T::try_from_slice(&chain.shard(id)).expect("decodes")
}

fn ctype(w: &World, id: [u8; 32]) -> CollateralType {
    read(&w.chain, ctype_account(&AUCTION, &id))
}

fn price(w: &mut World, id: [u8; 32], p: u128) -> Result<(), String> {
    let now = w.chain.now;
    w.chain.send(
        &Instruction::UpdatePrice { ctype_id: id, price: p, now },
        rows::update_price(&AUCTION, &id, w.admin.id),
        &[&w.admin],
    )
}

fn open(w: &mut World, pid: [u8; 32], ct: [u8; 32], collateral: u128, debt: u128) {
    w.chain
        .send(
            &Instruction::OpenPosition { position_id: pid, ctype_id: ct, collateral, debt },
            rows::open_position(&AUCTION, &pid, &ct, w.owner.id),
            &[&w.owner],
        )
        .expect("open position");
}

/// Liquidate as `who` at the chain's clock, at the type's time-weighted price.
fn liquidate(w: &mut World, who: usize, pid: [u8; 32], ct: [u8; 32]) -> Result<(), String> {
    let now = w.chain.now;
    let c = ctype(w, ct);
    let p: PositionState = read(&w.chain, position_account(&AUCTION, &pid));
    let ix = Instruction::Liquidate {
        position_id: pid,
        ctype_id: ct,
        collateral: p.collateral,
        debt: p.debt,
        price: twap(&c.observations, now).unwrap_or(1),
        now,
        terms: c.terms.clone(),
    };
    let k = &w.keepers[who];
    w.chain.send(&ix, rows::liquidate(&AUCTION, &pid, &ct, k.id), &[k])
}

fn auction(w: &World, pid: &[u8; 32]) -> AuctionState {
    w.chain.auction(&liquidation_auction_id(pid))
}

fn world() -> World {
    let (admin, owner, bidder) = (Key::new(1), Key::new(2), Key::new(3));
    let keepers = [Key::new(21), Key::new(22), Key::new(23)];
    let mut chain = Chain::new(&[admin.id, owner.id, bidder.id, keepers[0].id, keepers[1].id, keepers[2].id]);
    chain
        .send(
            &Instruction::InitConfig { config_id: CID, min_multiplier: MIN, max_multiplier: MAX, window: 120 },
            rows::init_config(&AUCTION, &CID, admin.id),
            &[&admin],
        )
        .unwrap();
    for (id, age) in [(A, 300_000), (B, 3_600_000)] {
        chain
            .send(
                &Instruction::InitCollateralType { ctype_id: id, terms: terms(age) },
                rows::init_ctype(&AUCTION, &id, &CID, admin.id),
                &[&admin],
            )
            .unwrap();
    }
    chain
        .send(
            &Instruction::InitDebtConfig { config_id: CID, terms: debt_terms() },
            rows::init_debt_config(&AUCTION, &CID, admin.id),
            &[&admin],
        )
        .unwrap();
    World { chain, admin, owner, keepers, bidder }
}

#[test]
fn only_the_configs_admin_opens_a_type_or_writes_its_feed() {
    let mut w = world();
    let r = w.chain.send(
        &Instruction::InitCollateralType { ctype_id: [0xC; 32], terms: terms(1) },
        rows::init_ctype(&AUCTION, &[0xC; 32], &CID, w.owner.id),
        &[&w.owner],
    );
    assert_eq!(code(&r.unwrap_err()), Some(E_NOT_ADMIN));
    let now = w.chain.now;
    let r = w.chain.send(
        &Instruction::UpdatePrice { ctype_id: A, price: WAD, now },
        rows::update_price(&AUCTION, &A, w.owner.id),
        &[&w.owner],
    );
    assert_eq!(code(&r.unwrap_err()), Some(E_NOT_ADMIN));
    // A price dated ahead of the chain is not included at all.
    let r = w.chain.send(
        &Instruction::UpdatePrice { ctype_id: A, price: WAD, now: now + 1 },
        rows::update_price(&AUCTION, &A, w.admin.id),
        &[&w.admin],
    );
    assert_eq!(code(&r.unwrap_err()), None, "refused by the validity window");
    let r = w.chain.send(
        &Instruction::UpdatePrice { ctype_id: A, price: WAD, now: now - START_WINDOW_MS },
        rows::update_price(&AUCTION, &A, w.admin.id),
        &[&w.admin],
    );
    assert_eq!(code(&r.unwrap_err()), None, "nor one dated a window behind it");
}

/// The whole scenario of scripts/e2e-liquidation-v03.sh, block for block.
#[test]
fn three_signers_liquidate_a_stale_feed_pauses_one_type_and_a_shortfall_is_auctioned() {
    let mut w = world();
    let t0 = w.chain.now;
    price(&mut w, A, 2_000 * WAD).unwrap();
    price(&mut w, B, 100 * WAD).unwrap();
    let (p1, p2, p3, p4, p5) = ([0x11; 32], [0x12; 32], [0x13; 32], [0x14; 32], [0x15; 32]);
    open(&mut w, p1, A, 10 * WAD, 12_000 * WAD);
    open(&mut w, p2, A, 10 * WAD, 12_500 * WAD);
    open(&mut w, p4, A, 10 * WAD, 12_000 * WAD);
    open(&mut w, p3, B, 100 * WAD, 6_000 * WAD);
    open(&mut w, p5, B, 100 * WAD, 6_000 * WAD);

    // Safe at 2000: nobody can liquidate.
    assert_eq!(code(&liquidate(&mut w, 0, p1, A).unwrap_err()), Some(E_POSITION_SAFE));

    // The price falls. The new print weighs nothing at the moment it lands.
    w.chain.now = t0 + 60_000;
    price(&mut w, A, 1_000 * WAD).unwrap();
    price(&mut w, B, 50 * WAD).unwrap();
    assert_eq!(code(&liquidate(&mut w, 0, p1, A).unwrap_err()), Some(E_POSITION_SAFE), "still 2000 time-weighted");
    w.chain.now = t0 + 90_000;
    // (2000·60 + 1000·30)/90 = 1666: 10 units are worth 16666 < 18000 required.
    let before = w.chain.shard(position_account(&AUCTION, &p1));
    let mut liar = Instruction::Liquidate {
        position_id: p1,
        ctype_id: A,
        collateral: 10 * WAD,
        debt: 12_000 * WAD,
        price: 1_000 * WAD,
        now: w.chain.now,
        terms: terms(300_000),
    };
    let k = &w.keepers[0];
    let r = w.chain.send(&liar, rows::liquidate(&AUCTION, &p1, &A, k.id), &[k]);
    assert_eq!(code(&r.unwrap_err()), Some(E_PRICE_MISMATCH), "the last print is not the price");
    if let Instruction::Liquidate { price, .. } = &mut liar {
        *price = twap(&ctype(&w, A).observations, w.chain.now).unwrap();
    }
    if let Instruction::Liquidate { debt, .. } = &mut liar {
        *debt = 11_500 * WAD;
    }
    let r = w.chain.send(&liar, rows::liquidate(&AUCTION, &p1, &A, k.id), &[k]);
    assert_eq!(code(&r.unwrap_err()), Some(E_POSITION_MISMATCH), "a position restated smaller");
    assert_eq!(w.chain.shard(position_account(&AUCTION, &p1)), before);

    // F1: three different signers, three liquidations.
    liquidate(&mut w, 0, p1, A).expect("keeper 1 liquidates p1");
    assert_eq!(code(&liquidate(&mut w, 1, p1, A).unwrap_err()), Some(E_ALREADY_LIQUIDATED));
    liquidate(&mut w, 1, p2, A).expect("keeper 2 liquidates p2");
    liquidate(&mut w, 2, p3, B).expect("keeper 3 liquidates p3 in the other type");
    for (pid, who) in [(p1, 0), (p2, 1), (p3, 2)] {
        let p: PositionState = read(&w.chain, position_account(&AUCTION, &pid));
        assert_eq!((p.liquidated, p.liquidator), (1, w.keepers[who].id));
    }
    let a1 = auction(&w, &p1);
    assert_eq!(a1.collateral_seized, 10 * WAD - WAD / 5, "the tip stays with the keeper");
    assert_eq!(a1.to_raise, 13_560 * WAD, "debt plus 13%");
    assert_eq!(a1.price, 1_666_666_666_666_666_666_666);

    // At the deepest discount (0.8 of 1666) the 9.8 units left are worth
    // 13066: the lot of p1 is bought out and closes short of 13560.
    let aid1 = liquidation_auction_id(&p1);
    w.chain.now = t0 + 90_000 + 120_000;
    let now = w.chain.now;
    w.chain
        .send(&Instruction::BuyCollateral { auction_id: aid1, bid: 20_000 * WAD, now }, rows::handle(&AUCTION, &aid1, w.bidder.id), &[&w.bidder])
        .unwrap();
    let a1 = auction(&w, &p1);
    assert_eq!((a1.collateral_left, a1.settled), (0, 1));
    let shortfall = a1.to_raise - a1.raised;
    assert!(shortfall > 0);

    // R2/R4: A's feed goes stale; B's is refreshed and keeps liquidating.
    w.chain.now = t0 + 60_000 + 300_000 - START_WINDOW_MS + 1;
    price(&mut w, B, 50 * WAD).unwrap();
    let stale_before = w.chain.shard(position_account(&AUCTION, &p4));
    assert_eq!(code(&liquidate(&mut w, 0, p4, A).unwrap_err()), Some(E_FEED_STALE));
    assert_eq!(w.chain.shard(position_account(&AUCTION, &p4)), stale_before);
    liquidate(&mut w, 1, p5, B).expect("the other type is unaffected");
    // R5: bidding on the stale type's open auction continues.
    let aid2 = liquidation_auction_id(&p2);
    let now = w.chain.now;
    w.chain
        .send(&Instruction::BuyCollateral { auction_id: aid2, bid: 1_000 * WAD, now }, rows::handle(&AUCTION, &aid2, w.bidder.id), &[&w.bidder])
        .expect("a bid on type A's auction while A's feed is stale");
    assert!(auction(&w, &p2).raised > 0);
    // The feed resumes, and so do A's liquidations.
    price(&mut w, A, 1_000 * WAD).unwrap();
    liquidate(&mut w, 2, p4, A).expect("feed fresh again");

    // F4: anyone books the shortfall, once.
    let book = |w: &mut World, who: usize, amount: u128| {
        let now = w.chain.now;
        let k = &w.keepers[who];
        w.chain.send(
            &Instruction::BookShortfall { auction_id: aid1, config_id: CID, shortfall: amount, now, terms: debt_terms() },
            rows::book_shortfall(&AUCTION, &aid1, &CID, k.id),
            &[k],
        )
    };
    assert_eq!(code(&book(&mut w, 0, shortfall - 1).unwrap_err()), Some(E_NO_SHORTFALL));
    book(&mut w, 0, shortfall).expect("book the shortfall");
    assert_eq!(code(&book(&mut w, 1, shortfall).unwrap_err()), Some(E_EXISTS));
    let d: DebtAuction = read(&w.chain, debt_auction_account(&AUCTION, &aid1));
    assert_eq!((d.coin, d.lot, d.protocol_token), (shortfall, 1_000 * WAD, debt_terms().protocol_token));

    let bid = |w: &mut World, who: usize, lot: u128| {
        let now = w.chain.now;
        let k = &w.keepers[who];
        w.chain.send(&Instruction::DebtBid { auction_id: aid1, lot, now }, rows::debt_handle(&AUCTION, &aid1, k.id), &[k])
    };
    let settle = |w: &mut World| {
        let now = w.chain.now;
        let k = &w.keepers[0];
        w.chain.send(&Instruction::SettleDebtAuction { auction_id: aid1, now }, rows::debt_handle(&AUCTION, &aid1, k.id), &[k])
    };
    assert_eq!(code(&settle(&mut w).unwrap_err()), Some(E_NO_BID));
    assert_eq!(code(&bid(&mut w, 0, 971 * WAD).unwrap_err()), Some(E_NOT_AN_IMPROVEMENT));
    bid(&mut w, 0, 970 * WAD).unwrap();
    w.chain.now += 10_000;
    assert_eq!(code(&bid(&mut w, 1, 970 * WAD).unwrap_err()), Some(E_NOT_AN_IMPROVEMENT));
    bid(&mut w, 1, 900 * WAD).unwrap();
    assert_eq!(code(&settle(&mut w).unwrap_err()), Some(E_NOT_FINISHED));
    w.chain.now += 30_000;
    assert_eq!(code(&bid(&mut w, 2, 800 * WAD).unwrap_err()), Some(E_CLOSED));
    settle(&mut w).unwrap();
    let d: DebtAuction = read(&w.chain, debt_auction_account(&AUCTION, &aid1));
    assert_eq!((d.settled, d.lot, d.bidder), (1, 900 * WAD, w.keepers[1].id));
}
