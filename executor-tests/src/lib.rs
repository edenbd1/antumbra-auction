//! A one-node LEZ v0.3 chain in memory, for the committed auction binary.
//!
//! Transactions are signed `PublicTransaction`s and go through
//! `ValidatedStateDiff::from_public_transaction_with_cycle_budget`, the call
//! the sequencer's settlement makes, then `V03State::apply_state_diff`. Nothing
//! is mocked between the message and the state: the plan and every apply run in
//! the RISC0 executor, and the block timestamp is checked against the windows
//! the plan sets.

use antumbra_auction_core::{AuctionState, Config, Instruction};
use lee::{
    public_transaction::{Message, WitnessSet},
    Account, AccountId, PrivateKey, ProgramShardSelector, PublicKey, PublicTransaction, V03State,
    ValidatedStateDiff,
};
use lee_core::program::TransactionEvent;

/// `fee_core::market::MAX_GAS_EXEC`: the most execution gas one transaction
/// may declare, and gas is cycles one for one.
pub const GAS_CAP: u64 = 10_000_000;

/// The header account the tests deploy the program at. On a real chain this is
/// whatever account the deployer claims; the program derives every PDA from it.
pub const AUCTION: AccountId = AccountId::new(*b"antumbra/auction/test-header/v03");

pub const BIN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/../artifacts/programs/v0.3/antumbra_auction.bin");

pub struct Key {
    pub sk: PrivateKey,
    pub id: AccountId,
}

impl Key {
    #[must_use]
    pub fn new(seed: u8) -> Self {
        let sk = PrivateKey::try_new([seed; 32]).expect("a valid secp256k1 scalar");
        let id = AccountId::from(&PublicKey::new_from_private_key(&sk));
        Self { sk, id }
    }
}

pub struct Chain {
    pub state: V03State,
    pub block: u64,
    /// The block timestamp, milliseconds.
    pub now: u64,
    /// Cycles the last transaction used, landed or not.
    pub last_cycles: u64,
    pub events: Vec<TransactionEvent>,
}

impl Chain {
    #[must_use]
    pub fn new(funded: &[AccountId]) -> Self {
        let bin = std::fs::read(BIN).expect("build the guest first: artifacts/programs/v0.3/antumbra_auction.bin");
        let program = lee::program::Program::new(bin.into()).expect("a RISC0 program binary");
        let accounts: Vec<(AccountId, Account)> = funded.iter().map(|id| (*id, Account::funded(1 << 60))).collect();
        let state = V03State::new()
            .with_public_accounts(accounts)
            .with_named_programs([(AUCTION, program)]);
        Self {
            state,
            block: 10,
            now: 1_700_000_000_000,
            last_cycles: 0,
            events: vec![],
        }
    }

    /// Send `ix` over `rows`, signed by `signers`, in a block at `self.now`.
    /// `Err` carries the refusal as the state machine reports it.
    pub fn send(&mut self, ix: &Instruction, rows: Vec<ProgramShardSelector>, signers: &[&Key]) -> Result<(), String> {
        self.send_with_budget(ix, rows, signers, GAS_CAP)
    }

    pub fn send_with_budget(
        &mut self,
        ix: &Instruction,
        rows: Vec<ProgramShardSelector>,
        signers: &[&Key],
        budget: u64,
    ) -> Result<(), String> {
        let nonces = signers.iter().map(|k| self.state.get_account_by_id(k.id).nonce).collect();
        let message = Message::try_new(AUCTION, rows, nonces, ix).map_err(|e| e.to_string())?;
        let keys: Vec<&PrivateKey> = signers.iter().map(|k| &k.sk).collect();
        let tx = PublicTransaction::new(message.clone(), WitnessSet::for_message(&message, &keys));
        let r = ValidatedStateDiff::from_public_transaction_with_cycle_budget(&tx, &self.state, self.block, self.now, budget);
        self.block += 1;
        match r {
            Ok((diff, charge)) => {
                self.last_cycles = charge.cycles;
                self.events = self.state.apply_state_diff(diff);
                Ok(())
            }
            Err(e) => Err(format!("{e}")),
        }
    }

    #[must_use]
    pub fn auction(&self, auction_id: &[u8; 32]) -> AuctionState {
        let id = antumbra_auction_core::auction_account(&AUCTION, auction_id);
        antumbra_auction_core::load_auction(self.state.get_account_by_id(id).data.shard(AUCTION))
    }

    #[must_use]
    pub fn config(&self, config_id: &[u8; 32]) -> Config {
        let id = antumbra_auction_core::config_account(&AUCTION, config_id);
        antumbra_auction_core::load_config(self.state.get_account_by_id(id).data.shard(AUCTION))
    }

    /// This program's shard of any account, empty when nothing was written.
    #[must_use]
    pub fn shard(&self, id: AccountId) -> Vec<u8> {
        self.state.get_account_by_id(id).data.shard(AUCTION).to_vec()
    }

    /// The raw bytes of an auction shard, empty when nothing was created.
    #[must_use]
    pub fn auction_bytes(&self, auction_id: &[u8; 32]) -> Vec<u8> {
        let id = antumbra_auction_core::auction_account(&AUCTION, auction_id);
        self.state.get_account_by_id(id).data.shard(AUCTION).to_vec()
    }
}

/// The refusal code a failed send carries, e.g. `Some(8004)`.
#[must_use]
pub fn code(err: &str) -> Option<u32> {
    let at = err.find(": E")? + 3;
    err[at..at + 4].parse().ok().filter(|_| err.contains("Guest panicked"))
}
