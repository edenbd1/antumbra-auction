//! antumbra_auction on LEZ v0.3: the wire types and the whole program logic.
//!
//! The guest binary is two lines that hand [`plan`] and [`apply`] to
//! `lee_core::program::run_program`. Everything else lives here so the CLI and
//! the host tests use the same types, seeds and checks as the program.
//!
//! WHAT THIS PROGRAM IS
//!
//! The increasing-discount auction house of RFP-014, and since instruction 4
//! ([`engine`]) what feeds it: collateral types with a time-weighted feed that
//! pauses only a stale type (R2, R4, R5), positions standing in for a host CDP's
//! vaults, a permissionless `liquidate` that opens the auction (F1), and a debt
//! auction for a shortfall (F4). It moves no tokens (that needs a pinned system
//! coin); the quote, clamps, re-strike and rounding are the `antumbra` library's.
//!
//! THE SPLIT v0.3 FORCES
//!
//! A v0.3 program runs twice per call. `plan` sees account ids, signer flags
//! and which shard each row selects, never data; it decides the effects, the
//! validity window and the events. Then each effect is applied one shard at a
//! time: `apply` sees that shard's current bytes and may replace them.
//!
//! On v0.2.4 a bid read the auction and its config in one call. Here an apply
//! sees one shard, so the config's discount schedule is copied into the auction
//! when it opens, and the copy is checked rather than trusted: `StartAuction`
//! carries the schedule the caller claims, one effect asks the config shard
//! whether that is what it holds (and whether the signer is its admin), and a
//! mismatch panics, which fails the whole transaction. A config is never
//! written again after creation, so the copy cannot go stale. From then on a
//! bid touches only the auction's own shard.
//!
//! WHERE TIME COMES FROM
//!
//! A plan cannot read the clock. It can bound the block timestamp at which the
//! transaction is valid, and the sequencer enforces that bound. Times are
//! milliseconds, the unit of the block timestamp; the discount window is in
//! seconds, as on v0.2.4.
//!
//! - A bid or a settlement at `now` is valid only once the block timestamp has
//!   reached `now`. A deeper discount is a later time, so a bidder cannot price
//!   a fill at a discount the auction has not yet reached.
//! - An auction opened at `now` is valid only in `[now, now + START_WINDOW_MS)`,
//!   so an opening cannot be back-dated to start the auction deeper into its
//!   discount than the chain's clock allows.
//! - The v0.2.4 guard stays: every instruction refuses a `now` earlier than one
//!   the auction has already honoured.

use borsh::{BorshDeserialize, BorshSerialize};
use lee_core::{
    account::{AccountId, ProgramShardSelector},
    program::{AccountMeta, PdaSeed, Plan, PlanInput, ProgramEvent, TimestampValidityWindow},
};
use risc0_zkvm::sha::{Impl, Sha256 as _};

pub mod engine;
pub mod errors;
use engine::*;
use errors::*;

/// How far past the time it names an auction's opening may still land.
pub const START_WINDOW_MS: u64 = 120_000;

/// The program's instruction. The variant order is the ABI.
///
/// A "signer handle" is a row naming the signing account with this program's
/// own shard selected. The program never writes it; the row exists so the
/// signature marks the account authorised.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Instruction {
    /// Open a configuration. Accounts: `[config, admin (signer handle)]`.
    InitConfig {
        config_id: [u8; 32],
        min_multiplier: u128,
        max_multiplier: u128,
        window: u64,
    },
    /// Open an auction over collateral already seized, under a config whose
    /// schedule the caller restates. Valid only while the block timestamp is
    /// in `[now, now + START_WINDOW_MS)`.
    ///
    /// Accounts: `[auction, config, admin (signer handle)]`.
    StartAuction {
        auction_id: [u8; 32],
        config_id: [u8; 32],
        collateral: u128,
        to_raise: u128,
        price: u128,
        now: u64,
        min_multiplier: u128,
        max_multiplier: u128,
        window: u64,
    },
    /// Buy part of the lot at the discount the schedule has reached by `now`.
    /// Valid only once the block timestamp reaches `now`.
    ///
    /// Accounts: `[auction, bidder (signer handle)]`.
    BuyCollateral {
        auction_id: [u8; 32],
        bid: u128,
        now: u64,
    },
    /// Close an auction whose target is met or whose lot is exhausted. Valid
    /// only once the block timestamp reaches `now`.
    ///
    /// Accounts: `[auction, caller (signer handle)]`.
    SettleAuction { auction_id: [u8; 32], now: u64 },

    // -------- 4 onwards: liquidation, the feed and the debt auction (engine.rs)
    /// Open a collateral type under a config, restating the config's schedule
    /// (checked, as at `StartAuction`). Its feed writer is the config's admin.
    ///
    /// Accounts: `[ctype, config, admin (signer handle)]`.
    InitCollateralType { ctype_id: [u8; 32], terms: CtypeTerms },
    /// Post a price to a type's feed. Valid only while the block timestamp is
    /// in `[now, now + START_WINDOW_MS)`, so a price cannot be dated ahead of
    /// the chain or far behind it.
    ///
    /// Accounts: `[ctype, admin (signer handle)]`.
    UpdatePrice { ctype_id: [u8; 32], price: u128, now: u64 },
    /// The reference host's vault: record collateral and debt in a type. Not
    /// gated on the feed (R5).
    ///
    /// Accounts: `[position, ctype, owner (signer handle)]`.
    OpenPosition {
        position_id: [u8; 32],
        ctype_id: [u8; 32],
        collateral: u128,
        debt: u128,
    },
    /// Liquidate an unsafe position, signed by anyone (F1). Restates the type's
    /// terms, its time-weighted price at `now` and the position; opens the
    /// auction at `liquidation_auction_id(position_id)`. Valid only while the
    /// block timestamp is in `[now, now + START_WINDOW_MS)`.
    ///
    /// Accounts: `[position, ctype, auction, liquidator (signer handle)]`.
    Liquidate {
        position_id: [u8; 32],
        ctype_id: [u8; 32],
        collateral: u128,
        debt: u128,
        price: u128,
        now: u64,
        terms: CtypeTerms,
    },
    /// Set a config's debt auction terms, once, by its admin.
    ///
    /// Accounts: `[debt_config, config, admin (signer handle)]`.
    InitDebtConfig { config_id: [u8; 32], terms: DebtTerms },
    /// Book a closed auction's shortfall into a debt auction (F4), signed by
    /// anyone. Valid only while the block timestamp is in
    /// `[now, now + START_WINDOW_MS)`.
    ///
    /// Accounts: `[auction, debt_config, debt_auction, caller (signer handle)]`.
    BookShortfall {
        auction_id: [u8; 32],
        config_id: [u8; 32],
        shortfall: u128,
        now: u64,
        terms: DebtTerms,
    },
    /// Offer to take a smaller lot of protocol token for covering the
    /// shortfall. Valid only while the block timestamp is in
    /// `[now, now + START_WINDOW_MS)`, so a bid cannot be back-dated under an
    /// expiry that has passed.
    ///
    /// Accounts: `[debt_auction, bidder (signer handle)]`.
    DebtBid { auction_id: [u8; 32], lot: u128, now: u64 },
    /// Close a debt auction whose standing bid has expired. Valid only once the
    /// block timestamp reaches `now`.
    ///
    /// Accounts: `[debt_auction, caller (signer handle)]`.
    SettleDebtAuction { auction_id: [u8; 32], now: u64 },
}

/// One deployer's configuration, this program's shard of the config PDA.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Config {
    /// The only account allowed to open auctions under this configuration.
    pub admin: AccountId,
    /// WAD. The price multiplier at the moment an auction opens.
    pub min_multiplier: u128,
    /// WAD. The multiplier at and after `window` seconds; below `min_multiplier`.
    pub max_multiplier: u128,
    /// Seconds from open to reaching `max_multiplier`.
    pub window: u64,
}

/// One auction, this program's shard of the auction PDA. The first ten fields
/// are the v0.2.4 layout in order; the schedule copy is appended.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AuctionState {
    pub config_id: [u8; 32],
    /// Never changes, so `sold + left == seized` is checkable at any point (R6).
    pub collateral_seized: u128,
    pub collateral_left: u128,
    /// Accumulated independently of `collateral_left`, so the R6 check compares
    /// two counters that are moved separately.
    pub collateral_sold: u128,
    pub to_raise: u128,
    pub raised: u128,
    /// WAD. System coin per base unit of collateral, as handed in at open.
    pub price: u128,
    /// Milliseconds.
    pub opened_at: u64,
    /// The largest `now` this auction has honoured, milliseconds.
    pub last_seen: u64,
    /// 0 open, 1 settled. Settled is terminal (F6).
    pub settled: u8,
    pub min_multiplier: u128,
    pub max_multiplier: u128,
    pub window: u64,
}

/// What `plan` asks `apply` to do to one shard.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub enum Effect {
    CreateConfig(Config),
    /// Applied to the config shard, which it never writes: the claimed schedule
    /// must be the stored one, and the signer the stored admin.
    CheckConfig {
        admin: AccountId,
        min_multiplier: u128,
        max_multiplier: u128,
        window: u64,
    },
    CreateAuction(AuctionState),
    Buy { bid: u128, now: u64 },
    Settle { now: u64 },
    CreateCtype(CollateralType),
    PushPrice { admin: AccountId, price: u128, now: u64 },
    /// Applied to a ctype shard, never written: it exists.
    CheckCtype,
    CreatePosition(PositionState),
    CheckFeed { terms: CtypeTerms, price: u128, now: u64 },
    Seize {
        ctype_id: [u8; 32],
        collateral: u128,
        debt: u128,
        liquidator: AccountId,
        tip: u128,
        now: u64,
    },
    /// Applied to a config shard, never written: the signer is its admin.
    CheckAdmin { admin: AccountId },
    CreateDebtConfig(DebtConfig),
    CheckShortfall { config_id: [u8; 32], shortfall: u128 },
    CheckDebtConfig(DebtTerms),
    CreateDebtAuction(DebtAuction),
    DebtBid { bidder: AccountId, lot: u128, now: u64 },
    DebtSettle { now: u64 },
}

// ---------------------------------------------------------------- addresses

fn sha256(parts: &[&[u8]]) -> [u8; 32] {
    let mut buf = Vec::with_capacity(parts.iter().map(|p| p.len()).sum());
    for p in parts {
        buf.extend_from_slice(p);
    }
    Impl::hash_bytes(&buf)
        .as_bytes()
        .try_into()
        .expect("a SHA-256 digest is 32 bytes")
}

/// A string seed, zero-padded to 32 bytes: SPEL's `seed_from_str`.
#[must_use]
pub fn seed_from_str(s: &str) -> [u8; 32] {
    let src = s.as_bytes();
    assert!(src.len() <= 32, "seed string exceeds 32 bytes");
    let mut out = [0u8; 32];
    out[..src.len()].copy_from_slice(src);
    out
}

/// SPEL's multi-seed rule: several seeds are hashed together with SHA-256.
#[must_use]
pub fn combine_seeds(seeds: &[&[u8; 32]]) -> PdaSeed {
    match seeds {
        [one] => PdaSeed::new(**one),
        many => {
            let parts: Vec<&[u8]> = many.iter().map(|s| s.as_slice()).collect();
            PdaSeed::new(sha256(&parts))
        }
    }
}

/// `[config_id, "config"]`. On v0.2.4 configs and auctions shared one seed
/// space, so a config id could shadow an auction id; the tag separates them.
#[must_use]
pub fn config_seed(config_id: &[u8; 32]) -> PdaSeed {
    combine_seeds(&[config_id, &seed_from_str("config")])
}

/// `[auction_id, "auction"]`.
#[must_use]
pub fn auction_seed(auction_id: &[u8; 32]) -> PdaSeed {
    combine_seeds(&[auction_id, &seed_from_str("auction")])
}

#[must_use]
pub fn config_account(program: &AccountId, config_id: &[u8; 32]) -> AccountId {
    AccountId::for_public_pda(program, &config_seed(config_id))
}

#[must_use]
pub fn auction_account(program: &AccountId, auction_id: &[u8; 32]) -> AccountId {
    AccountId::for_public_pda(program, &auction_seed(auction_id))
}

// ------------------------------------------------------------------- events

/// `sha256("antumbra_auction::<Name>")[..8]`, the convention `ProgramEvent`
/// documents.
#[must_use]
pub fn event_selector(name: &str) -> [u8; 8] {
    let full = sha256(&[b"antumbra_auction::", name.as_bytes()]);
    full[..8].try_into().expect("8 of 32 bytes")
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ConfigInitialized {
    pub config_id: [u8; 32],
    pub admin: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AuctionStarted {
    pub auction_id: [u8; 32],
    pub config_id: [u8; 32],
    pub collateral: u128,
    pub to_raise: u128,
    pub price: u128,
    pub at: u64,
}

/// What the bid asked for. The fill itself is a function of the auction's
/// stored state, which the plan cannot see; it is read back from the account.
#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct BidPlaced {
    pub auction_id: [u8; 32],
    pub bidder: AccountId,
    pub bid: u128,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct AuctionSettled {
    pub auction_id: [u8; 32],
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct CollateralTypeInitialized {
    pub ctype_id: [u8; 32],
    pub config_id: [u8; 32],
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PriceUpdated {
    pub ctype_id: [u8; 32],
    pub price: u128,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct PositionOpened {
    pub position_id: [u8; 32],
    pub ctype_id: [u8; 32],
    pub owner: AccountId,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct Liquidated {
    pub position_id: [u8; 32],
    pub ctype_id: [u8; 32],
    pub auction_id: [u8; 32],
    pub liquidator: AccountId,
    pub tip: u128,
    pub lot: u128,
    pub to_raise: u128,
    pub price: u128,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct ShortfallBooked {
    pub auction_id: [u8; 32],
    pub shortfall: u128,
    pub lot: u128,
    pub at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, BorshSerialize, BorshDeserialize)]
pub struct DebtBidPlaced {
    pub auction_id: [u8; 32],
    pub bidder: AccountId,
    pub lot: u128,
    pub at: u64,
}

/// Every event name, in the order the IDL lists them.
pub const EVENT_NAMES: [&str; 11] = [
    "ConfigInitialized",
    "AuctionStarted",
    "BidPlaced",
    "AuctionSettled",
    "CollateralTypeInitialized",
    "PriceUpdated",
    "PositionOpened",
    "Liquidated",
    "ShortfallBooked",
    "DebtBidPlaced",
    "DebtAuctionSettled",
];

fn event<T: BorshSerialize>(name: &str, body: &T) -> ProgramEvent {
    ProgramEvent {
        selector: event_selector(name),
        data: borsh::to_vec(body).expect("borsh serialization is infallible"),
    }
}

// --------------------------------------------------------------- arithmetic

/// The discount schedule a config or an auction carries.
#[must_use]
pub fn schedule(min: u128, max: u128, window: u64) -> antumbra::DiscountSchedule {
    antumbra::DiscountSchedule { min, max, window }
}

/// Refuse a schedule that cannot be evaluated: validated at creation rather
/// than at the first bid, after collateral had been committed against it.
pub fn validate_schedule(min: u128, max: u128, window: u64) {
    if schedule(min, max, window).multiplier_at(0).is_err() {
        refuse(E_BAD_SCHEDULE, "discount schedule is out of range");
    }
}

/// Seconds since the auction opened, at `now` milliseconds.
#[must_use]
pub fn elapsed_secs(s: &AuctionState, now: u64) -> u64 {
    now.saturating_sub(s.opened_at) / 1000
}

/// The fill a bid of `bid` at `now` would get: the library's quote against the
/// auction's remaining lot and target. The CLI shows it before sending.
pub fn quote(s: &AuctionState, bid: u128, now: u64) -> Result<antumbra::Fill, antumbra::AuctionError> {
    antumbra::Auction {
        collateral_left: s.collateral_left,
        to_raise: s.to_raise - s.raised.min(s.to_raise),
        schedule: schedule(s.min_multiplier, s.max_multiplier, s.window),
    }
    .quote(s.price, bid, elapsed_secs(s, now))
}

// --------------------------------------------------------------------- plan

fn rows<const N: usize>(input: &PlanInput) -> [AccountMeta; N] {
    <[AccountMeta; N]>::try_from(input.accounts.clone())
        .unwrap_or_else(|_| refuse(E_ACCOUNTS, "wrong number of accounts for this instruction"))
}

fn expect_row(row: &AccountMeta, id: AccountId, me: &AccountId, what: &str) {
    refuse_unless(
        row.account_id == id && row.program_account_id == *me,
        E_NOT_ANCHORED,
        what,
    );
}

fn expect_signer_handle(row: &AccountMeta, me: &AccountId) {
    refuse_unless(row.is_authorized, E_NOT_SIGNED, "the signer row carries no signature");
    refuse_unless(
        row.program_account_id == *me,
        E_ACCOUNTS,
        "a signer handle selects this program's shard",
    );
}

/// The plan entry point.
#[must_use]
pub fn plan(input: &PlanInput, ix: Instruction) -> Plan {
    let me = input.self_account_id;
    let mut plan = Plan::new(input);
    match ix {
        Instruction::InitConfig {
            config_id,
            min_multiplier,
            max_multiplier,
            window,
        } => {
            let [config, admin] = rows::<2>(input);
            expect_row(&config, config_account(&me, &config_id), &me, "the config row is not this program's shard of the config PDA");
            expect_signer_handle(&admin, &me);
            validate_schedule(min_multiplier, max_multiplier, window);
            plan.effect(
                &config,
                &Effect::CreateConfig(Config {
                    admin: admin.account_id,
                    min_multiplier,
                    max_multiplier,
                    window,
                }),
            );
            plan.event(event(
                "ConfigInitialized",
                &ConfigInitialized {
                    config_id,
                    admin: admin.account_id,
                },
            ));
        }
        Instruction::StartAuction {
            auction_id,
            config_id,
            collateral,
            to_raise,
            price,
            now,
            min_multiplier,
            max_multiplier,
            window,
        } => {
            let [auction, config, admin] = rows::<3>(input);
            expect_row(&auction, auction_account(&me, &auction_id), &me, "the auction row is not this program's shard of the auction PDA");
            expect_row(&config, config_account(&me, &config_id), &me, "no config at this id");
            expect_signer_handle(&admin, &me);
            refuse_unless(
                collateral > 0 && to_raise > 0 && price > 0,
                E_BAD_STATE,
                "an auction needs collateral, a target and a price",
            );
            validate_schedule(min_multiplier, max_multiplier, window);
            let until = now
                .checked_add(START_WINDOW_MS)
                .unwrap_or_else(|| refuse(E_TIME_WENT_BACKWARDS, "opening time overflows"));
            plan.timestamp_window(
                TimestampValidityWindow::try_from(now..until).expect("a positive window is not empty"),
            );
            plan.effect(
                &config,
                &Effect::CheckConfig {
                    admin: admin.account_id,
                    min_multiplier,
                    max_multiplier,
                    window,
                },
            );
            plan.effect(
                &auction,
                &Effect::CreateAuction(AuctionState {
                    config_id,
                    collateral_seized: collateral,
                    collateral_left: collateral,
                    collateral_sold: 0,
                    to_raise,
                    raised: 0,
                    price,
                    opened_at: now,
                    last_seen: now,
                    settled: 0,
                    min_multiplier,
                    max_multiplier,
                    window,
                }),
            );
            plan.event(event(
                "AuctionStarted",
                &AuctionStarted {
                    auction_id,
                    config_id,
                    collateral,
                    to_raise,
                    price,
                    at: now,
                },
            ));
        }
        Instruction::BuyCollateral { auction_id, bid, now } => {
            let [auction, bidder] = rows::<2>(input);
            expect_row(&auction, auction_account(&me, &auction_id), &me, "no auction at this id");
            expect_signer_handle(&bidder, &me);
            plan.timestamp_window(now..);
            plan.effect(&auction, &Effect::Buy { bid, now });
            plan.event(event(
                "BidPlaced",
                &BidPlaced {
                    auction_id,
                    bidder: bidder.account_id,
                    bid,
                    at: now,
                },
            ));
        }
        Instruction::SettleAuction { auction_id, now } => {
            let [auction, caller] = rows::<2>(input);
            expect_row(&auction, auction_account(&me, &auction_id), &me, "no auction at this id");
            expect_signer_handle(&caller, &me);
            plan.timestamp_window(now..);
            plan.effect(&auction, &Effect::Settle { now });
            plan.event(event("AuctionSettled", &AuctionSettled { auction_id, at: now }));
        }
        Instruction::InitCollateralType { ctype_id, terms } => {
            let [ctype, config, admin] = rows::<3>(input);
            expect_row(&ctype, ctype_account(&me, &ctype_id), &me, "the ctype row is not this program's shard of the ctype PDA");
            expect_row(&config, config_account(&me, &terms.config_id), &me, "no config at this id");
            expect_signer_handle(&admin, &me);
            validate_ctype_terms(&terms);
            plan.effect(
                &config,
                &Effect::CheckConfig {
                    admin: admin.account_id,
                    min_multiplier: terms.min_multiplier,
                    max_multiplier: terms.max_multiplier,
                    window: terms.window,
                },
            );
            let config_id = terms.config_id;
            plan.effect(
                &ctype,
                &Effect::CreateCtype(CollateralType {
                    admin: admin.account_id,
                    terms,
                    observations: vec![],
                }),
            );
            plan.event(event("CollateralTypeInitialized", &CollateralTypeInitialized { ctype_id, config_id }));
        }
        Instruction::UpdatePrice { ctype_id, price, now } => {
            let [ctype, admin] = rows::<2>(input);
            expect_row(&ctype, ctype_account(&me, &ctype_id), &me, "no collateral type at this id");
            expect_signer_handle(&admin, &me);
            plan.timestamp_window(bounded(now));
            plan.effect(&ctype, &Effect::PushPrice { admin: admin.account_id, price, now });
            plan.event(event("PriceUpdated", &PriceUpdated { ctype_id, price, at: now }));
        }
        Instruction::OpenPosition {
            position_id,
            ctype_id,
            collateral,
            debt,
        } => {
            let [position, ctype, owner] = rows::<3>(input);
            expect_row(&position, position_account(&me, &position_id), &me, "the position row is not this program's shard of the position PDA");
            expect_row(&ctype, ctype_account(&me, &ctype_id), &me, "no collateral type at this id");
            expect_signer_handle(&owner, &me);
            refuse_unless(collateral > 0, E_BAD_STATE, "a position needs collateral");
            plan.effect(&ctype, &Effect::CheckCtype);
            plan.effect(
                &position,
                &Effect::CreatePosition(PositionState {
                    ctype_id,
                    owner: owner.account_id,
                    collateral,
                    debt,
                    liquidated: 0,
                    liquidator: owner.account_id,
                    tip: 0,
                    liquidated_at: 0,
                }),
            );
            plan.event(event(
                "PositionOpened",
                &PositionOpened {
                    position_id,
                    ctype_id,
                    owner: owner.account_id,
                },
            ));
        }
        Instruction::Liquidate {
            position_id,
            ctype_id,
            collateral,
            debt,
            price,
            now,
            terms,
        } => {
            let [position, ctype, auction, liquidator] = rows::<4>(input);
            let auction_id = liquidation_auction_id(&position_id);
            expect_row(&position, position_account(&me, &position_id), &me, "no position at this id");
            expect_row(&ctype, ctype_account(&me, &ctype_id), &me, "no collateral type at this id");
            expect_row(&auction, auction_account(&me, &auction_id), &me, "the auction row is not this position's liquidation auction");
            // F1: any signer. The row must carry a signature, so the liquidator
            // is named and booked, but nothing compares it to a list.
            expect_signer_handle(&liquidator, &me);
            refuse_unless(price > 0, E_BAD_STATE, "a price of zero");
            refuse_unless(
                is_unsafe(collateral, debt, terms.liquidation_ratio, price),
                E_POSITION_SAFE,
                "the position is safe at this collateral type's time-weighted price",
            );
            let (tip, lot, to_raise) = liquidation_split(collateral, debt, &terms);
            refuse_unless(lot > 0 && to_raise > 0, E_BAD_STATE, "nothing to auction");
            validate_schedule(terms.min_multiplier, terms.max_multiplier, terms.window);
            plan.timestamp_window(bounded(now));
            plan.effect(&ctype, &Effect::CheckFeed { terms: terms.clone(), price, now });
            plan.effect(
                &position,
                &Effect::Seize {
                    ctype_id,
                    collateral,
                    debt,
                    liquidator: liquidator.account_id,
                    tip,
                    now,
                },
            );
            plan.effect(
                &auction,
                &Effect::CreateAuction(AuctionState {
                    config_id: terms.config_id,
                    collateral_seized: lot,
                    collateral_left: lot,
                    collateral_sold: 0,
                    to_raise,
                    raised: 0,
                    price,
                    opened_at: now,
                    last_seen: now,
                    settled: 0,
                    min_multiplier: terms.min_multiplier,
                    max_multiplier: terms.max_multiplier,
                    window: terms.window,
                }),
            );
            plan.event(event(
                "Liquidated",
                &Liquidated {
                    position_id,
                    ctype_id,
                    auction_id,
                    liquidator: liquidator.account_id,
                    tip,
                    lot,
                    to_raise,
                    price,
                    at: now,
                },
            ));
        }
        Instruction::InitDebtConfig { config_id, terms } => {
            let [debt_config, config, admin] = rows::<3>(input);
            expect_row(&debt_config, debt_config_account(&me, &config_id), &me, "the row is not this config's debt auction terms");
            expect_row(&config, config_account(&me, &config_id), &me, "no config at this id");
            expect_signer_handle(&admin, &me);
            validate_debt_terms(&terms);
            plan.effect(&config, &Effect::CheckAdmin { admin: admin.account_id });
            plan.effect(&debt_config, &Effect::CreateDebtConfig(DebtConfig { terms }));
        }
        Instruction::BookShortfall {
            auction_id,
            config_id,
            shortfall,
            now,
            terms,
        } => {
            let [auction, debt_config, debt_auction, caller] = rows::<4>(input);
            expect_row(&auction, auction_account(&me, &auction_id), &me, "no auction at this id");
            expect_row(&debt_config, debt_config_account(&me, &config_id), &me, "no debt auction terms for this config");
            expect_row(&debt_auction, debt_auction_account(&me, &auction_id), &me, "the row is not this auction's debt auction");
            expect_signer_handle(&caller, &me);
            refuse_unless(shortfall > 0, E_NO_SHORTFALL, "a shortfall of zero");
            validate_debt_terms(&terms);
            let end = now
                .checked_add(terms.auction_ttl_ms)
                .unwrap_or_else(|| refuse(E_ARITHMETIC, "the auction's end overflows"));
            plan.timestamp_window(bounded(now));
            plan.effect(&auction, &Effect::CheckShortfall { config_id, shortfall });
            plan.effect(&debt_config, &Effect::CheckDebtConfig(terms.clone()));
            plan.effect(
                &debt_auction,
                &Effect::CreateDebtAuction(DebtAuction {
                    source_auction: auction_id,
                    config_id,
                    protocol_token: terms.protocol_token,
                    coin: shortfall,
                    lot: terms.initial_lot,
                    beg: terms.beg,
                    bid_ttl_ms: terms.bid_ttl_ms,
                    has_bid: 0,
                    bidder: caller.account_id,
                    bid_expiry: end,
                    end,
                    last_seen: now,
                    settled: 0,
                }),
            );
            plan.event(event(
                "ShortfallBooked",
                &ShortfallBooked {
                    auction_id,
                    shortfall,
                    lot: terms.initial_lot,
                    at: now,
                },
            ));
        }
        Instruction::DebtBid { auction_id, lot, now } => {
            let [debt_auction, bidder] = rows::<2>(input);
            expect_row(&debt_auction, debt_auction_account(&me, &auction_id), &me, "no debt auction at this id");
            expect_signer_handle(&bidder, &me);
            plan.timestamp_window(bounded(now));
            plan.effect(&debt_auction, &Effect::DebtBid { bidder: bidder.account_id, lot, now });
            plan.event(event(
                "DebtBidPlaced",
                &DebtBidPlaced {
                    auction_id,
                    bidder: bidder.account_id,
                    lot,
                    at: now,
                },
            ));
        }
        Instruction::SettleDebtAuction { auction_id, now } => {
            let [debt_auction, caller] = rows::<2>(input);
            expect_row(&debt_auction, debt_auction_account(&me, &auction_id), &me, "no debt auction at this id");
            expect_signer_handle(&caller, &me);
            plan.timestamp_window(now..);
            plan.effect(&debt_auction, &Effect::DebtSettle { now });
            plan.event(event("DebtAuctionSettled", &AuctionSettled { auction_id, at: now }));
        }
    }
    plan
}

/// `[now, now + START_WINDOW_MS)`: valid from the time named, and not long after.
fn bounded(now: u64) -> TimestampValidityWindow {
    let until = now
        .checked_add(START_WINDOW_MS)
        .unwrap_or_else(|| refuse(E_TIME_WENT_BACKWARDS, "time overflows"));
    TimestampValidityWindow::try_from(now..until).expect("a positive window is not empty")
}

// -------------------------------------------------------------------- apply

/// Decode a stored config, refusing an empty shard: nothing was created there.
#[must_use]
pub fn load_config(pre: &[u8]) -> Config {
    refuse_unless(!pre.is_empty(), E_NOT_ANCHORED, "no config at this id");
    Config::try_from_slice(pre).unwrap_or_else(|_| refuse(E_BAD_STATE, "config failed to deserialize"))
}

/// Decode a stored auction, refusing an empty shard.
#[must_use]
pub fn load_auction(pre: &[u8]) -> AuctionState {
    refuse_unless(!pre.is_empty(), E_NOT_ANCHORED, "no auction at this id");
    AuctionState::try_from_slice(pre).unwrap_or_else(|_| refuse(E_BAD_STATE, "auction failed to deserialize"))
}

fn store(s: &impl BorshSerialize) -> Vec<u8> {
    borsh::to_vec(s).expect("borsh serialization is infallible")
}

/// A bid, applied to a stored auction: the v0.2.4 `buy_collateral` body.
#[must_use]
pub fn buy(mut s: AuctionState, bid: u128, now: u64) -> AuctionState {
    refuse_unless(s.settled == 0, E_ALREADY_SETTLED, "this auction is settled");
    refuse_unless(
        now >= s.last_seen,
        E_TIME_WENT_BACKWARDS,
        "a bid cannot be priced at a clock earlier than one already honoured",
    );
    refuse_unless(
        s.collateral_left != 0 && s.to_raise != s.raised,
        E_NOTHING_LEFT,
        "nothing left to sell or raise",
    );
    let fill = quote(&s, bid, now).unwrap_or_else(|_| refuse(E_ARITHMETIC, "the bid could not be priced"));
    refuse_unless(
        fill.collateral_out != 0,
        E_BID_TOO_SMALL,
        "this bid buys less than one base unit at the current discount",
    );
    s.collateral_left -= fill.collateral_out;
    s.collateral_sold += fill.collateral_out;
    s.raised += fill.paid;
    s.last_seen = now;
    // F2: terminate early the moment the target is reached or the lot is gone.
    if s.raised == s.to_raise || s.collateral_left == 0 {
        s.settled = 1;
    }
    // R6, asserted over two counters moved separately.
    refuse_unless(
        s.collateral_sold + s.collateral_left == s.collateral_seized,
        E_ARITHMETIC,
        "collateral is not conserved",
    );
    refuse_unless(s.raised <= s.to_raise, E_ARITHMETIC, "raised more than the target");
    s
}

/// A settlement, applied to a stored auction: the v0.2.4 `settle_auction` body.
#[must_use]
pub fn settle(mut s: AuctionState, now: u64) -> AuctionState {
    refuse_unless(s.settled == 0, E_ALREADY_SETTLED, "this auction is settled");
    refuse_unless(
        now >= s.last_seen,
        E_TIME_WENT_BACKWARDS,
        "settlement cannot be dated before a fill already honoured",
    );
    refuse_unless(
        s.raised == s.to_raise || s.collateral_left == 0,
        E_NOT_FINISHED,
        "the target is not met and collateral remains",
    );
    s.settled = 1;
    s.last_seen = now;
    s
}

/// The apply entry point: one effect against one shard.
#[must_use]
pub fn apply(effect: Effect, pre: &[u8]) -> Option<Vec<u8>> {
    match effect {
        Effect::CreateConfig(c) => {
            // An id is used once; refusing any existing bytes keeps a replayed
            // creation from replacing the admin.
            refuse_unless(pre.is_empty(), E_EXISTS, "a config already exists at this id");
            Some(store(&c))
        }
        Effect::CheckConfig {
            admin,
            min_multiplier,
            max_multiplier,
            window,
        } => {
            let c = load_config(pre);
            refuse_unless(c.admin == admin, E_NOT_ADMIN, "signer is not this config's admin");
            refuse_unless(
                (c.min_multiplier, c.max_multiplier, c.window) == (min_multiplier, max_multiplier, window),
                E_CONFIG_MISMATCH,
                "the schedule named is not the one this config holds",
            );
            None
        }
        Effect::CreateAuction(state) => {
            refuse_unless(pre.is_empty(), E_EXISTS, "an auction already exists at this id");
            Some(store(&state))
        }
        Effect::Buy { bid, now } => Some(store(&buy(load_auction(pre), bid, now))),
        Effect::Settle { now } => Some(store(&settle(load_auction(pre), now))),
        Effect::CreateCtype(c) => {
            refuse_unless(pre.is_empty(), E_EXISTS, "a collateral type already exists at this id");
            Some(store(&c))
        }
        Effect::PushPrice { admin, price, now } => Some(store(&push_price(load_ctype(pre), admin, price, now))),
        Effect::CheckCtype => {
            let _ = load_ctype(pre);
            None
        }
        Effect::CreatePosition(p) => {
            refuse_unless(pre.is_empty(), E_EXISTS, "a position already exists at this id");
            Some(store(&p))
        }
        Effect::CheckFeed { terms, price, now } => {
            check_feed(&load_ctype(pre), &terms, price, now);
            None
        }
        Effect::Seize {
            ctype_id,
            collateral,
            debt,
            liquidator,
            tip,
            now,
        } => Some(store(&seize(load_position(pre), ctype_id, collateral, debt, liquidator, tip, now))),
        Effect::CheckAdmin { admin } => {
            refuse_unless(load_config(pre).admin == admin, E_NOT_ADMIN, "signer is not this config's admin");
            None
        }
        Effect::CreateDebtConfig(c) => {
            refuse_unless(pre.is_empty(), E_EXISTS, "this config's debt auction terms are already set");
            Some(store(&c))
        }
        Effect::CheckShortfall { config_id, shortfall } => {
            check_shortfall(&load_auction(pre), &config_id, shortfall);
            None
        }
        Effect::CheckDebtConfig(terms) => {
            refuse_unless(load_debt_config(pre).terms == terms, E_CONFIG_MISMATCH, "the debt terms named are not this config's");
            None
        }
        Effect::CreateDebtAuction(d) => {
            // One debt auction per collateral auction: a shortfall is booked once.
            refuse_unless(pre.is_empty(), E_EXISTS, "this auction's shortfall is already booked");
            Some(store(&d))
        }
        Effect::DebtBid { bidder, lot, now } => Some(store(&debt_bid(load_debt_auction(pre), bidder, lot, now))),
        Effect::DebtSettle { now } => Some(store(&debt_settle(load_debt_auction(pre), now))),
    }
}

/// The account rows each instruction takes, in order, for clients. The plan
/// checks exactly these.
pub mod rows {
    use super::*;

    fn sel(account: AccountId, program: AccountId) -> ProgramShardSelector {
        ProgramShardSelector::new(account, program)
    }

    #[must_use]
    pub fn init_config(program: &AccountId, config_id: &[u8; 32], admin: AccountId) -> Vec<ProgramShardSelector> {
        vec![sel(config_account(program, config_id), *program), sel(admin, *program)]
    }

    #[must_use]
    pub fn start_auction(
        program: &AccountId,
        auction_id: &[u8; 32],
        config_id: &[u8; 32],
        admin: AccountId,
    ) -> Vec<ProgramShardSelector> {
        vec![
            sel(auction_account(program, auction_id), *program),
            sel(config_account(program, config_id), *program),
            sel(admin, *program),
        ]
    }

    /// `[auction, signer handle]`: buy_collateral, settle_auction.
    #[must_use]
    pub fn handle(program: &AccountId, auction_id: &[u8; 32], signer: AccountId) -> Vec<ProgramShardSelector> {
        vec![sel(auction_account(program, auction_id), *program), sel(signer, *program)]
    }

    #[must_use]
    pub fn init_ctype(program: &AccountId, ctype_id: &[u8; 32], config_id: &[u8; 32], admin: AccountId) -> Vec<ProgramShardSelector> {
        vec![
            sel(ctype_account(program, ctype_id), *program),
            sel(config_account(program, config_id), *program),
            sel(admin, *program),
        ]
    }

    #[must_use]
    pub fn update_price(program: &AccountId, ctype_id: &[u8; 32], admin: AccountId) -> Vec<ProgramShardSelector> {
        vec![sel(ctype_account(program, ctype_id), *program), sel(admin, *program)]
    }

    #[must_use]
    pub fn open_position(program: &AccountId, position_id: &[u8; 32], ctype_id: &[u8; 32], owner: AccountId) -> Vec<ProgramShardSelector> {
        vec![
            sel(position_account(program, position_id), *program),
            sel(ctype_account(program, ctype_id), *program),
            sel(owner, *program),
        ]
    }

    #[must_use]
    pub fn liquidate(program: &AccountId, position_id: &[u8; 32], ctype_id: &[u8; 32], liquidator: AccountId) -> Vec<ProgramShardSelector> {
        vec![
            sel(position_account(program, position_id), *program),
            sel(ctype_account(program, ctype_id), *program),
            sel(auction_account(program, &liquidation_auction_id(position_id)), *program),
            sel(liquidator, *program),
        ]
    }

    #[must_use]
    pub fn init_debt_config(program: &AccountId, config_id: &[u8; 32], admin: AccountId) -> Vec<ProgramShardSelector> {
        vec![
            sel(debt_config_account(program, config_id), *program),
            sel(config_account(program, config_id), *program),
            sel(admin, *program),
        ]
    }

    #[must_use]
    pub fn book_shortfall(program: &AccountId, auction_id: &[u8; 32], config_id: &[u8; 32], caller: AccountId) -> Vec<ProgramShardSelector> {
        vec![
            sel(auction_account(program, auction_id), *program),
            sel(debt_config_account(program, config_id), *program),
            sel(debt_auction_account(program, auction_id), *program),
            sel(caller, *program),
        ]
    }

    /// `[debt_auction, signer handle]`: debt_bid, settle_debt_auction.
    #[must_use]
    pub fn debt_handle(program: &AccountId, auction_id: &[u8; 32], signer: AccountId) -> Vec<ProgramShardSelector> {
        vec![sel(debt_auction_account(program, auction_id), *program), sel(signer, *program)]
    }
}

#[cfg(test)]
mod tests;
#[cfg(test)]
mod engine_tests;
