//! Refusal codes. A v0.3 program refuses by panicking, which fails the whole
//! transaction; the code opens the panic message (`E8004: ...`), so a refusal
//! reads the same in the sequencer log, the executor tests and the CLI.
//!
//! 8001 to 8010 are the v0.2.4 codes, unchanged in meaning. 8011 onwards are
//! what v0.3's account model adds: nothing is refused for already existing
//! unless the program refuses it, the plan sees rows rather than accounts, and
//! a config read moved from a direct read to a checked claim.

pub const E_NOT_ANCHORED: u32 = 8001;
pub const E_BAD_STATE: u32 = 8002;
pub const E_NOT_ADMIN: u32 = 8003;
pub const E_TIME_WENT_BACKWARDS: u32 = 8004;
pub const E_ALREADY_SETTLED: u32 = 8005;
pub const E_NOTHING_LEFT: u32 = 8006;
pub const E_BID_TOO_SMALL: u32 = 8007;
pub const E_ARITHMETIC: u32 = 8008;
pub const E_BAD_SCHEDULE: u32 = 8009;
pub const E_NOT_FINISHED: u32 = 8010;
pub const E_EXISTS: u32 = 8011;
pub const E_ACCOUNTS: u32 = 8012;
pub const E_NOT_SIGNED: u32 = 8013;
pub const E_CONFIG_MISMATCH: u32 = 8014;
// 8015 onwards: liquidation, the price feed and the debt auction (F1, F4, R2, R4).
pub const E_FEED_STALE: u32 = 8015;
pub const E_PRICE_MISMATCH: u32 = 8016;
pub const E_POSITION_SAFE: u32 = 8017;
pub const E_ALREADY_LIQUIDATED: u32 = 8018;
pub const E_POSITION_MISMATCH: u32 = 8019;
pub const E_NO_SHORTFALL: u32 = 8020;
pub const E_NOT_AN_IMPROVEMENT: u32 = 8021;
pub const E_CLOSED: u32 = 8022;
pub const E_NO_BID: u32 = 8023;

/// Every code with its name, in the order the IDL lists them.
pub const ALL: [(u32, &str); 23] = [
    (E_NOT_ANCHORED, "NotAnchored"),
    (E_BAD_STATE, "BadState"),
    (E_NOT_ADMIN, "NotAdmin"),
    (E_TIME_WENT_BACKWARDS, "TimeWentBackwards"),
    (E_ALREADY_SETTLED, "AlreadySettled"),
    (E_NOTHING_LEFT, "NothingLeft"),
    (E_BID_TOO_SMALL, "BidTooSmall"),
    (E_ARITHMETIC, "Arithmetic"),
    (E_BAD_SCHEDULE, "BadSchedule"),
    (E_NOT_FINISHED, "NotFinished"),
    (E_EXISTS, "AlreadyExists"),
    (E_ACCOUNTS, "Accounts"),
    (E_NOT_SIGNED, "NotSigned"),
    (E_CONFIG_MISMATCH, "ConfigMismatch"),
    (E_FEED_STALE, "FeedStale"),
    (E_PRICE_MISMATCH, "PriceMismatch"),
    (E_POSITION_SAFE, "PositionSafe"),
    (E_ALREADY_LIQUIDATED, "AlreadyLiquidated"),
    (E_POSITION_MISMATCH, "PositionMismatch"),
    (E_NO_SHORTFALL, "NoShortfall"),
    (E_NOT_AN_IMPROVEMENT, "NotAnImprovement"),
    (E_CLOSED, "Closed"),
    (E_NO_BID, "NoBid"),
];

/// Refuse the transaction with `code`.
#[track_caller]
pub fn refuse(code: u32, why: &str) -> ! {
    panic!("E{code}: {why}")
}

#[track_caller]
pub fn refuse_unless(ok: bool, code: u32, why: &str) {
    if !ok {
        refuse(code, why);
    }
}
