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

/// Every code with its name, in the order the IDL lists them.
pub const ALL: [(u32, &str); 14] = [
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
