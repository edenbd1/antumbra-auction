//! Liquidation, the price feed and the debt auction: F1, F4, R2, R4 and R5 on
//! chain.
//!
//! WHAT THIS ADDS TO THE AUCTION HOUSE
//!
//! - A **collateral type** (`[id, "ctype"]`): the liquidation terms of one kind
//!   of collateral (ratio, penalty, keeper tip, the auction schedule copied
//!   from its config, the feed's maximum age) and its price feed, a buffer of
//!   the last [`OBSERVATIONS`] prices written by the config's admin.
//! - A **position** (`[id, "position"]`): collateral and debt in one type. This
//!   is the reference host's stand-in for a vault of the CDP RFP-014 plugs
//!   into; the program records it and never moves the tokens, as with the
//!   auction itself.
//! - **`liquidate`**, signed by anyone (F1). It seizes an unsafe position, books
//!   the keeper's tip, and opens an increasing-discount auction over the rest
//!   of the lot for debt plus penalty, at the feed's time-weighted price.
//! - A **debt auction** per collateral auction that closed short
//!   (`[auction_id, "debt"]`, F4): anyone books the shortfall, bidders accept a
//!   shrinking lot of a protocol token (a configurable address the program
//!   never dereferences, as the RFP allows) for covering it, and it settles
//!   once the standing bid expires.
//!
//! THE FEED (R2, R4, R5)
//!
//! Liquidation prices at the time-weighted average of the buffer up to `now`,
//! not at the last print: each observation weighs for as long as it stood. A
//! print posted at `now` weighs nothing yet, so one bad print cannot liquidate
//! a book on its own.
//!
//! A feed is stale for a liquidation when it would be older than `max_age_ms`
//! at the last instant the transaction may land (`now + START_WINDOW_MS`, the
//! end of its validity window). Judging age at the end of the window means a
//! liquidator cannot back-date `now` to make a stale feed look fresh: the pause
//! can fire early by up to the window, never late. A stale feed refuses
//! liquidation of **that type only** (R4); every other instruction, bids and
//! settlements included, never reads a feed (R5).
//!
//! WHY EVERYTHING IS RESTATED
//!
//! As with `StartAuction`: a plan sees rows, never data, and each apply sees
//! one shard. `liquidate` restates the type's terms, the price and the
//! position; the plan decides health and builds the auction from the restated
//! figures, and one effect per shard checks the claim against what is stored
//! (the type: terms, freshness and the time-weighted price; the position: its
//! collateral and debt, and that it is not already liquidated). A false claim
//! fails its check and the whole transaction with it.

use antumbra::{liquidation, mul_div_ceil, mul_div_floor, settlement, WAD};
use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::account::AccountId;

use crate::{combine_seeds, errors::*, seed_from_str, sha256, START_WINDOW_MS};
use lee_core::program::PdaSeed;

/// How many prices a collateral type remembers.
pub const OBSERVATIONS: usize = 4;

/// The liquidation terms of one collateral type. Restated by `liquidate`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CtypeTerms {
    /// The config whose schedule the auctions of this type copy, and whose
    /// debt auction terms take their shortfalls.
    pub config_id: [u8; 32],
    /// WAD, at least 1: 1.5e18 means 150% collateral is required.
    pub liquidation_ratio: u128,
    /// WAD, at least 1: the auction raises `debt · penalty`.
    pub penalty: u128,
    /// WAD, below 1: the share of the seized collateral booked to whoever
    /// triggered the liquidation (F1's incentive).
    pub tip: u128,
    /// Milliseconds a price stays usable for liquidation.
    pub max_age_ms: u64,
    pub min_multiplier: u128,
    pub max_multiplier: u128,
    pub window: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Observation {
    /// WAD. System coin per base unit of collateral.
    pub price: u128,
    /// Milliseconds.
    pub at: u64,
}

/// One collateral type, this program's shard of the ctype PDA.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CollateralType {
    /// The config's admin, who alone writes the feed.
    pub admin: AccountId,
    pub terms: CtypeTerms,
    /// Oldest first, at most [`OBSERVATIONS`].
    pub observations: Vec<Observation>,
}

/// One position in one collateral type: the reference host's vault.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PositionState {
    pub ctype_id: [u8; 32],
    pub owner: AccountId,
    pub collateral: u128,
    pub debt: u128,
    /// 0 open, 1 liquidated. Liquidated is terminal.
    pub liquidated: u8,
    /// Who triggered the liquidation; the owner until then.
    pub liquidator: AccountId,
    /// Base units of collateral booked to the liquidator.
    pub tip: u128,
    /// When it was liquidated, milliseconds; 0 until then.
    pub liquidated_at: u64,
}

/// What a config's debt auctions trade. Restated by `book_shortfall`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct DebtTerms {
    /// The protocol token's address. Configurable and never dereferenced:
    /// F4 asks for a mock token behind a configurable address.
    pub protocol_token: [u8; 32],
    /// Base units of protocol token offered before the first bid.
    pub initial_lot: u128,
    /// WAD, in (0, 1): the minimum improvement, at least one base unit.
    pub beg: u128,
    /// Milliseconds a standing bid has before it wins.
    pub bid_ttl_ms: u64,
    /// Milliseconds from booking to the auction's hard end.
    pub auction_ttl_ms: u64,
}

/// One config's debt auction terms, this program's shard of `[config_id, "debt-config"]`.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct DebtConfig {
    pub terms: DebtTerms,
}

/// One debt auction, this program's shard of `[auction_id, "debt"]`, where
/// `auction_id` is the collateral auction whose shortfall it covers. So a
/// shortfall is booked once.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct DebtAuction {
    pub source_auction: [u8; 32],
    pub config_id: [u8; 32],
    pub protocol_token: [u8; 32],
    /// The shortfall: system coin the winner covers. Fixed.
    pub coin: u128,
    /// Protocol token the winner receives. Only ever shrinks.
    pub lot: u128,
    pub beg: u128,
    pub bid_ttl_ms: u64,
    /// 0 until the first bid.
    pub has_bid: u8,
    pub bidder: AccountId,
    pub bid_expiry: u64,
    pub end: u64,
    pub last_seen: u64,
    pub settled: u8,
}

// ---------------------------------------------------------------- addresses

#[must_use]
pub fn ctype_seed(id: &[u8; 32]) -> PdaSeed {
    combine_seeds(&[id, &seed_from_str("ctype")])
}
#[must_use]
pub fn position_seed(id: &[u8; 32]) -> PdaSeed {
    combine_seeds(&[id, &seed_from_str("position")])
}
#[must_use]
pub fn debt_config_seed(config_id: &[u8; 32]) -> PdaSeed {
    combine_seeds(&[config_id, &seed_from_str("debt-config")])
}
#[must_use]
pub fn debt_auction_seed(auction_id: &[u8; 32]) -> PdaSeed {
    combine_seeds(&[auction_id, &seed_from_str("debt")])
}
#[must_use]
pub fn ctype_account(program: &AccountId, id: &[u8; 32]) -> AccountId {
    AccountId::for_public_pda(program, &ctype_seed(id))
}
#[must_use]
pub fn position_account(program: &AccountId, id: &[u8; 32]) -> AccountId {
    AccountId::for_public_pda(program, &position_seed(id))
}
#[must_use]
pub fn debt_config_account(program: &AccountId, config_id: &[u8; 32]) -> AccountId {
    AccountId::for_public_pda(program, &debt_config_seed(config_id))
}
#[must_use]
pub fn debt_auction_account(program: &AccountId, auction_id: &[u8; 32]) -> AccountId {
    AccountId::for_public_pda(program, &debt_auction_seed(auction_id))
}

/// The collateral auction a liquidation of `position_id` opens. Derived, so a
/// position's auction is found without asking anyone, and it is in its own
/// space: no auction an admin opens by id can sit where it will go.
#[must_use]
pub fn liquidation_auction_id(position_id: &[u8; 32]) -> [u8; 32] {
    sha256(&[b"antumbra_auction::liquidation", position_id])
}

// --------------------------------------------------------------- arithmetic

/// The time-weighted price of `obs` at `now`: each observation weighs for as
/// long as it stood, the last one until `now`. `None` with no observation, or
/// when `now` precedes the last one.
///
/// Each term is `price · duration / total`, taken in 256 bits and floored, so
/// the sum is at most the largest price and cannot overflow; the floors lose at
/// most one base unit per observation.
#[must_use]
pub fn twap(obs: &[Observation], now: u64) -> Option<u128> {
    let first = obs.first()?;
    let last = obs.last()?;
    if now < last.at {
        return None;
    }
    let total = now - first.at;
    if total == 0 {
        return Some(last.price);
    }
    let mut sum: u128 = 0;
    for (i, o) in obs.iter().enumerate() {
        let until = obs.get(i + 1).map_or(now, |n| n.at);
        let term = mul_div_floor(o.price, u128::from(until - o.at), u128::from(total)).ok()?;
        sum = sum.checked_add(term)?;
    }
    Some(sum)
}

/// Whether the feed is unusable for a liquidation dated `now`: empty, or older
/// than `max_age_ms` at the end of the transaction's validity window.
#[must_use]
pub fn feed_is_stale(obs: &[Observation], now: u64, max_age_ms: u64) -> bool {
    match obs.last() {
        None => true,
        Some(o) => now.saturating_add(START_WINDOW_MS).saturating_sub(o.at) > max_age_ms,
    }
}

/// Whether a position is below its required ratio at `price`. The comparison is
/// the library's, without forming either product.
#[must_use]
pub fn is_unsafe(collateral: u128, debt: u128, liquidation_ratio: u128, price: u128) -> bool {
    let p = liquidation::Position {
        collateral,
        debt,
        liquidation_ratio,
    };
    let fresh = liquidation::Price {
        value: price,
        observed_at: 0,
    };
    matches!(
        p.health(fresh, 0, u64::MAX),
        Ok(liquidation::Health::Undercollateralised)
    )
}

/// What a liquidation books: (tip to the liquidator, lot auctioned, coin to raise).
/// The tip rounds down (towards the owner); the target rounds up (towards the
/// system), so neither rounding is taken from the side that did not choose it.
#[must_use]
pub fn liquidation_split(collateral: u128, debt: u128, terms: &CtypeTerms) -> (u128, u128, u128) {
    let tip = mul_div_floor(collateral, terms.tip, WAD).unwrap_or_else(|_| refuse(E_ARITHMETIC, "tip"));
    let to_raise = mul_div_ceil(debt, terms.penalty, WAD).unwrap_or_else(|_| refuse(E_ARITHMETIC, "debt times penalty"));
    (tip, collateral - tip, to_raise)
}

pub fn validate_ctype_terms(t: &CtypeTerms) {
    refuse_unless(
        t.liquidation_ratio >= WAD && t.penalty >= WAD && t.tip < WAD && t.max_age_ms > 0,
        E_BAD_STATE,
        "a collateral type needs ratio >= 1, penalty >= 1, tip < 1 and a positive max age",
    );
    crate::validate_schedule(t.min_multiplier, t.max_multiplier, t.window);
}

pub fn validate_debt_terms(t: &DebtTerms) {
    refuse_unless(
        t.initial_lot > 0 && t.beg > 0 && t.beg < WAD && t.bid_ttl_ms > 0 && t.auction_ttl_ms > 0,
        E_BAD_STATE,
        "debt auction terms need a lot, 0 < beg < 1 and positive durations",
    );
}

// -------------------------------------------------------------------- apply

#[must_use]
pub fn load_ctype(pre: &[u8]) -> CollateralType {
    refuse_unless(!pre.is_empty(), E_NOT_ANCHORED, "no collateral type at this id");
    CollateralType::try_from_slice(pre).unwrap_or_else(|_| refuse(E_BAD_STATE, "collateral type failed to deserialize"))
}
#[must_use]
pub fn load_position(pre: &[u8]) -> PositionState {
    refuse_unless(!pre.is_empty(), E_NOT_ANCHORED, "no position at this id");
    PositionState::try_from_slice(pre).unwrap_or_else(|_| refuse(E_BAD_STATE, "position failed to deserialize"))
}
#[must_use]
pub fn load_debt_config(pre: &[u8]) -> DebtConfig {
    refuse_unless(!pre.is_empty(), E_NOT_ANCHORED, "no debt auction terms for this config");
    DebtConfig::try_from_slice(pre).unwrap_or_else(|_| refuse(E_BAD_STATE, "debt config failed to deserialize"))
}
#[must_use]
pub fn load_debt_auction(pre: &[u8]) -> DebtAuction {
    refuse_unless(!pre.is_empty(), E_NOT_ANCHORED, "no debt auction at this id");
    DebtAuction::try_from_slice(pre).unwrap_or_else(|_| refuse(E_BAD_STATE, "debt auction failed to deserialize"))
}

/// The feed's admin posts a price at `now`.
#[must_use]
pub fn push_price(mut c: CollateralType, admin: AccountId, price: u128, now: u64) -> CollateralType {
    refuse_unless(c.admin == admin, E_NOT_ADMIN, "only the config's admin writes this feed");
    refuse_unless(price > 0, E_BAD_STATE, "a price of zero");
    if let Some(last) = c.observations.last() {
        refuse_unless(now > last.at, E_TIME_WENT_BACKWARDS, "a price no later than the last one");
    }
    c.observations.push(Observation { price, at: now });
    if c.observations.len() > OBSERVATIONS {
        c.observations.remove(0);
    }
    c
}

/// The type shard's half of a liquidation: the restated terms are the stored
/// ones, the feed is fresh (R2, R4), and the restated price is its
/// time-weighted average at `now`.
pub fn check_feed(c: &CollateralType, terms: &CtypeTerms, price: u128, now: u64) {
    refuse_unless(c.terms == *terms, E_CONFIG_MISMATCH, "the terms named are not this collateral type's");
    refuse_unless(
        !feed_is_stale(&c.observations, now, terms.max_age_ms),
        E_FEED_STALE,
        "this collateral type's feed is stale: its liquidations are paused",
    );
    let avg = twap(&c.observations, now).unwrap_or_else(|| refuse(E_TIME_WENT_BACKWARDS, "a liquidation dated before the feed's last price"));
    refuse_unless(avg == price, E_PRICE_MISMATCH, "the price named is not the feed's time-weighted price at this time");
}

/// The position shard's half: it is the one restated, and it is still open.
#[must_use]
pub fn seize(mut p: PositionState, ctype_id: [u8; 32], collateral: u128, debt: u128, liquidator: AccountId, tip: u128, now: u64) -> PositionState {
    refuse_unless(p.liquidated == 0, E_ALREADY_LIQUIDATED, "this position is already liquidated");
    refuse_unless(
        p.ctype_id == ctype_id && p.collateral == collateral && p.debt == debt,
        E_POSITION_MISMATCH,
        "the position named is not the one stored",
    );
    p.liquidated = 1;
    p.liquidator = liquidator;
    p.tip = tip;
    p.liquidated_at = now;
    p
}

/// The collateral auction's half of booking a shortfall: it closed with its
/// lot gone and its target not met, by exactly `shortfall`, under `config_id`.
pub fn check_shortfall(s: &crate::AuctionState, config_id: &[u8; 32], shortfall: u128) {
    refuse_unless(s.config_id == *config_id, E_CONFIG_MISMATCH, "this auction runs under another config");
    refuse_unless(
        s.settled == 1 && s.collateral_left == 0 && s.raised < s.to_raise,
        E_NO_SHORTFALL,
        "only an auction closed with its lot gone and its target unmet has a shortfall",
    );
    refuse_unless(s.to_raise - s.raised == shortfall, E_NO_SHORTFALL, "the shortfall named is not this auction's");
}

/// A bid on a debt auction: a lot smaller than the standing one by `beg`, at
/// least one base unit, before the auction or the standing bid expires.
#[must_use]
pub fn debt_bid(mut d: DebtAuction, bidder: AccountId, lot: u128, now: u64) -> DebtAuction {
    refuse_unless(d.settled == 0, E_ALREADY_SETTLED, "this debt auction is settled");
    refuse_unless(now >= d.last_seen, E_TIME_WENT_BACKWARDS, "a bid earlier than one already honoured");
    refuse_unless(now < d.end, E_CLOSED, "this debt auction has ended");
    refuse_unless(d.has_bid == 0 || now < d.bid_expiry, E_CLOSED, "the standing bid has already won");
    let standing = settlement::Settlement {
        direction: settlement::Direction::ShrinkingLot,
        standing: d.lot,
        beg: d.beg,
        expiry: d.bid_expiry,
    };
    let next = standing
        .improve(lot, now, d.bid_ttl_ms)
        .unwrap_or_else(|_| refuse(E_NOT_AN_IMPROVEMENT, "the lot does not improve on the standing one by the minimum"));
    d.lot = next.standing;
    d.bid_expiry = next.expiry.min(d.end);
    d.has_bid = 1;
    d.bidder = bidder;
    d.last_seen = now;
    d
}

/// Close a debt auction whose standing bid has expired or whose end has passed.
#[must_use]
pub fn debt_settle(mut d: DebtAuction, now: u64) -> DebtAuction {
    refuse_unless(d.settled == 0, E_ALREADY_SETTLED, "this debt auction is settled");
    refuse_unless(now >= d.last_seen, E_TIME_WENT_BACKWARDS, "a settlement earlier than a bid already honoured");
    refuse_unless(d.has_bid == 1, E_NO_BID, "nobody has bid: nothing to settle");
    refuse_unless(now >= d.bid_expiry || now >= d.end, E_NOT_FINISHED, "the standing bid can still be beaten");
    d.settled = 1;
    d.last_seen = now;
    d
}
