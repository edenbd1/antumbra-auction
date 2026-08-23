//! Antumbra collateral auction — the increasing-discount auction house of
//! RFP-014, as a SPEL program on the Logos Execution Zone.
//!
//! What this program is, and is not. It is the auction house: the state machine
//! and the arithmetic over collateral it has been told was seized, at a price it
//! was handed. It is not the liquidation trigger, because that needs a host CDP
//! with positions, and it does not move tokens yet, because that needs a payer
//! and a pinned transfer program. Both are the next instruction, not this one.
//! What is here is deployed, driven and readable back off the chain.
//!
//! Three things the arithmetic library this depends on establishes, and which
//! shape every line below:
//!
//! - The reference this RFP names, Reflexer's increasing-discount auction house,
//!   cannot be transcribed. `rdivide(x, y) = x · RAY / y` takes the product
//!   first, which is about 1e45 for an ordinary collateral price, against a
//!   `u128` ceiling of 3.4e38. So every quotient here goes through `mul_div`,
//!   whose product is taken in 256 bits and never stored.
//! - The discount is linear in the multiplier rather than compounded per second,
//!   because compounding is what forces `rpower`, and `rpower` squares a
//!   ray-scaled rate into 1e54 on its first iteration.
//! - `collateral_out` rounds **down**. F3 requires partial fills, so a bidder
//!   chooses how often the quotient is rounded; rounding up lets them split one
//!   payment into N and take N times the collateral.
//!
//! Time is a caller argument, and a caller who can rewind it can re-price a
//! fill at yesterday's discount. So every instruction that reads the clock
//! refuses a `now` earlier than one already honoured, and the auction stores
//! `last_seen` for that purpose alone.

#![no_main]

use spel_framework::prelude::*;

risc0_zkvm::guest::entry!(main);

/// The auction's own error base. The sibling programs in this family use
/// disjoint bases so an error code names the program it came from.
const E_NOT_ANCHORED: u32 = 8001;
const E_BAD_STATE: u32 = 8002;
const E_NOT_ADMIN: u32 = 8003;
const E_TIME_WENT_BACKWARDS: u32 = 8004;
const E_ALREADY_SETTLED: u32 = 8005;
const E_NOTHING_LEFT: u32 = 8006;
const E_BID_TOO_SMALL: u32 = 8007;
const E_ARITHMETIC: u32 = 8008;
const E_BAD_SCHEDULE: u32 = 8009;
const E_NOT_FINISHED: u32 = 8010;

/// One deployer's configuration.
///
/// On LEZ a program's id *is* the ImageID of its guest, so two deployers of
/// byte-identical code get the same program id. RFP-014's "each product deploys
/// its own configured instance" therefore cannot mean a separate deployment —
/// it means a distinct config account under one shared program, which is what
/// this is. It is also the better outcome: it is what makes the RFP's own
/// rationale, shared audits and shared liquidator tooling, actually true.
#[account_type]
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct Config {
    /// The only account allowed to open auctions under this configuration.
    pub admin: [u8; 32],
    /// WAD. The price multiplier at the moment an auction opens.
    pub min_multiplier: u128,
    /// WAD. The multiplier at and after `window` seconds. Smaller means a
    /// deeper discount, so this is below `min_multiplier`.
    pub max_multiplier: u128,
    /// Seconds from open to reaching `max_multiplier`.
    pub window: u64,
}

/// One live auction.
#[account_type]
#[derive(BorshSerialize, BorshDeserialize, Clone, Debug)]
pub struct AuctionState {
    /// The config this auction was opened under.
    pub config_id: [u8; 32],
    /// Base units of collateral seized when the auction opened. Never changes,
    /// so `seized == sold + left` is checkable at any point — R6's invariant.
    pub collateral_seized: u128,
    /// Base units still unsold.
    pub collateral_left: u128,
    /// Base units handed to bidders so far, accumulated independently of
    /// `collateral_left` rather than derived from it. Two counters that are
    /// maintained separately can disagree, which is the only way an invariant
    /// over them can fail — and therefore the only way asserting it can mean
    /// anything. It is also what lets anyone holding this account check R6
    /// without trusting our assertion: `sold + left == seized`.
    pub collateral_sold: u128,
    /// Base units of system coin the auction set out to raise.
    pub to_raise: u128,
    /// Base units raised so far.
    pub raised: u128,
    /// WAD. System coin per base unit of collateral, as handed in at open.
    pub price: u128,
    pub opened_at: u64,
    /// The largest `now` this auction has honoured. A caller-supplied clock is
    /// only monotone if the program makes it so.
    pub last_seen: u64,
    /// 0 open, 1 settled. Settled is terminal: F6 asks for no reversals.
    pub settled: u8,
}

fn write(account: &mut Account, state: &impl BorshSerialize) -> Result<(), SpelError> {
    let bytes = borsh::to_vec(state)
        .map_err(|_| SpelError::custom(E_BAD_STATE, "state failed to serialize"))?;
    account.data = bytes
        .try_into()
        .map_err(|_| SpelError::custom(E_BAD_STATE, "state does not fit the buffer"))?;
    Ok(())
}

#[lez_program]
mod antumbra_auction {
    #[allow(unused_imports)]
    use super::*;
    use antumbra::{Auction, DiscountSchedule};

    /// Open a configuration. One per deployer per parameter set.
    #[instruction]
    pub fn init_config(
        ctx: ProgramContext,
        #[account(init, pda = [arg("config_id")])] mut config: AccountWithMetadata,
        #[account(signer)] admin: AccountWithMetadata,
        config_id: [u8; 32],
        min_multiplier: u128,
        max_multiplier: u128,
        window: u64,
    ) -> SpelResult {
        let _ = (ctx, config_id);
        // Validated here rather than at each quote: a schedule that cannot be
        // evaluated would otherwise fail on the first bid, after an auction had
        // already been opened against it and collateral had been committed.
        let probe = DiscountSchedule {
            min: min_multiplier,
            max: max_multiplier,
            window,
        };
        probe
            .multiplier_at(0)
            .map_err(|_| SpelError::custom(E_BAD_SCHEDULE, "discount schedule is out of range"))?;

        let state = Config {
            admin: *admin.account_id.value(),
            min_multiplier,
            max_multiplier,
            window,
        };
        write(&mut config.account, &state)?;
        Ok(SpelOutput::execute(vec![config, admin], vec![]))
    }

    /// Open an auction over collateral already seized.
    ///
    /// The seizure itself belongs to the host CDP and is not this instruction:
    /// `collateral` is what the caller asserts was taken. Wiring that assertion
    /// to a real seizure is F1 and F7, and needs a host that exists.
    #[instruction]
    pub fn start_auction(
        ctx: ProgramContext,
        #[account(init, pda = [arg("auction_id")])] mut auction: AccountWithMetadata,
        #[account(pda = [arg("config_id")])] config: AccountWithMetadata,
        #[account(signer)] admin: AccountWithMetadata,
        auction_id: [u8; 32],
        config_id: [u8; 32],
        collateral: u128,
        to_raise: u128,
        price: u128,
        now: u64,
    ) -> SpelResult {
        let _ = auction_id;
        if config.account.program_owner != ctx.self_program_id {
            return Err(SpelError::custom(E_NOT_ANCHORED, "no config at this id"));
        }
        let cfg = Config::try_from_slice(&config.account.data)
            .map_err(|_| SpelError::custom(E_BAD_STATE, "config failed to deserialize"))?;
        if &cfg.admin != admin.account_id.value() {
            return Err(SpelError::custom(E_NOT_ADMIN, "signer is not this config's admin"));
        }
        if collateral == 0 || to_raise == 0 || price == 0 {
            return Err(SpelError::custom(
                E_BAD_STATE,
                "an auction needs collateral, a target and a price",
            ));
        }

        let state = AuctionState {
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
        };
        write(&mut auction.account, &state)?;
        Ok(SpelOutput::execute(vec![auction, config, admin], vec![]))
    }

    /// Buy part of the lot at the discount the schedule has reached.
    ///
    /// F3's partial fill. The bid is clamped to what is left to raise and the
    /// collateral to what is left to sell — the first so a bidder cannot overpay
    /// into a finished auction, the second so the auction cannot hand out
    /// collateral it does not hold.
    #[instruction]
    pub fn buy_collateral(
        ctx: ProgramContext,
        #[account(mut, pda = [arg("auction_id")])] mut auction: AccountWithMetadata,
        #[account(pda = [arg("config_id")])] config: AccountWithMetadata,
        #[account(signer)] bidder: AccountWithMetadata,
        auction_id: [u8; 32],
        config_id: [u8; 32],
        bid: u128,
        now: u64,
    ) -> SpelResult {
        let _ = (auction_id, config_id);
        if auction.account.program_owner != ctx.self_program_id
            || config.account.program_owner != ctx.self_program_id
        {
            return Err(SpelError::custom(E_NOT_ANCHORED, "no auction at this id"));
        }
        let mut state = AuctionState::try_from_slice(&auction.account.data)
            .map_err(|_| SpelError::custom(E_BAD_STATE, "auction failed to deserialize"))?;
        let cfg = Config::try_from_slice(&config.account.data)
            .map_err(|_| SpelError::custom(E_BAD_STATE, "config failed to deserialize"))?;

        if state.settled != 0 {
            return Err(SpelError::custom(E_ALREADY_SETTLED, "this auction is settled"));
        }
        if now < state.last_seen {
            return Err(SpelError::custom(
                E_TIME_WENT_BACKWARDS,
                "a bid cannot be priced at a clock earlier than one already honoured",
            ));
        }
        if state.collateral_left == 0 || state.to_raise == state.raised {
            return Err(SpelError::custom(E_NOTHING_LEFT, "nothing left to sell or raise"));
        }

        let live = Auction {
            collateral_left: state.collateral_left,
            to_raise: state.to_raise - state.raised,
            schedule: DiscountSchedule {
                min: cfg.min_multiplier,
                max: cfg.max_multiplier,
                window: cfg.window,
            },
        };
        let fill = live
            .quote(state.price, bid, now.saturating_sub(state.opened_at))
            .map_err(|_| SpelError::custom(E_ARITHMETIC, "the bid could not be priced"))?;
        if fill.collateral_out == 0 {
            return Err(SpelError::custom(
                E_BID_TOO_SMALL,
                "this bid buys less than one base unit at the current discount",
            ));
        }

        state.collateral_left -= fill.collateral_out;
        state.collateral_sold += fill.collateral_out;
        state.raised += fill.paid;
        state.last_seen = now;
        // F2: terminate early the moment the target is reached, rather than
        // waiting out a deadline while collateral sits seized.
        if state.raised == state.to_raise || state.collateral_left == 0 {
            state.settled = 1;
        }
        // R6, asserted rather than assumed. If this ever fails the arithmetic
        // above leaked, and failing the proof is the right outcome.
        //
        // This check used to read `left + (seized - left) != seized`, which is
        // an identity: it holds for every value of every field, including the
        // ones a leak would produce, and it was never once going to fire. The
        // two sides must come from counters that are moved separately, or there
        // is nothing to compare.
        if state.collateral_sold + state.collateral_left != state.collateral_seized {
            return Err(SpelError::custom(E_ARITHMETIC, "collateral is not conserved"));
        }
        // And the target is a ceiling, not a suggestion: `raised` is only ever
        // increased by a `paid` the library clamped to what was left to raise.
        if state.raised > state.to_raise {
            return Err(SpelError::custom(E_ARITHMETIC, "raised more than the target"));
        }
        write(&mut auction.account, &state)?;
        Ok(SpelOutput::execute(vec![auction, config, bidder], vec![]))
    }

    /// Close an auction whose target is met or whose lot is exhausted.
    ///
    /// Separate from the last fill so that closing is observable on its own, and
    /// refused otherwise: F6 asks for strict transitions with no reversals, so a
    /// settle that is not yet due is an error rather than a no-op.
    #[instruction]
    pub fn settle_auction(
        ctx: ProgramContext,
        #[account(mut, pda = [arg("auction_id")])] mut auction: AccountWithMetadata,
        #[account(signer)] caller: AccountWithMetadata,
        auction_id: [u8; 32],
        now: u64,
    ) -> SpelResult {
        let _ = auction_id;
        if auction.account.program_owner != ctx.self_program_id {
            return Err(SpelError::custom(E_NOT_ANCHORED, "no auction at this id"));
        }
        let mut state = AuctionState::try_from_slice(&auction.account.data)
            .map_err(|_| SpelError::custom(E_BAD_STATE, "auction failed to deserialize"))?;
        if state.settled != 0 {
            return Err(SpelError::custom(E_ALREADY_SETTLED, "this auction is settled"));
        }
        if now < state.last_seen {
            return Err(SpelError::custom(
                E_TIME_WENT_BACKWARDS,
                "settlement cannot be dated before a fill already honoured",
            ));
        }
        if state.raised != state.to_raise && state.collateral_left != 0 {
            return Err(SpelError::custom(
                E_NOT_FINISHED,
                "the target is not met and collateral remains",
            ));
        }
        state.settled = 1;
        state.last_seen = now;
        write(&mut auction.account, &state)?;
        Ok(SpelOutput::execute(vec![auction, caller], vec![]))
    }
}
