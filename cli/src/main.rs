//! `antumbra-auction`: open configurations and auctions, bid, settle and
//! inspect antumbra_auction on LEZ v0.3.
//!
//! The commands that only read (`show`, `show-config`, `quote`, `ids`,
//! `image-id`, `now`) need no wallet: they ask the sequencer at `--rpc`
//! (`ANTUMBRA_RPC`, by default the public testnet) or compute locally. The
//! commands that sign take the keys, the sequencer URL and the fee settings
//! from the v0.3 wallet home (`LEE_WALLET_HOME_DIR`). The program's header
//! account comes from `--program` or `ANTUMBRA_PROGRAM`.
//!
//! Every transaction prints one JSON line on stdout: the operation, its hash,
//! the block it was included in, and a verdict read back from the chain.
//! `applied` means the watched shard changed as the instruction intends;
//! `reverted` means the transaction was included (and charged) but the program
//! refused it; `rejected` means the sequencer never took it; `not-included`
//! means no attempt was seen in a block while the CLI waited.

use std::{path::PathBuf, time::Duration};

use antumbra_auction_core::{self as core, auction_account, config_account, AuctionState, Config, Instruction};
use anyhow::{anyhow, bail, ensure, Context as _, Result};
use clap::{Parser, Subcommand};
use lee::{program::Program, AccountId, ProgramShardSelector};
use sequencer_service_rpc::{RpcClient as _, SequencerClient, SequencerClientBuilder};
use wallet::{program_facades::program_loader::ProgramLoader, AccountIdentity, AccountMention, WalletCore};

/// The public testnet v0.3 sequencer.
const TESTNET_RPC: &str = "https://testnet.lez.logos.co";

#[derive(Parser)]
#[command(name = "antumbra-auction", version, about = "antumbra_auction on LEZ v0.3")]
struct Cli {
    /// The program's header account (base58).
    #[arg(long, env = "ANTUMBRA_PROGRAM", global = true)]
    program: Option<AccountId>,
    /// The sequencer the read-only commands ask. Commands that sign use the
    /// wallet's own sequencer instead.
    #[arg(long, env = "ANTUMBRA_RPC", global = true, default_value = TESTNET_RPC)]
    rpc: String,
    /// The guest binary, for `deploy` and `image-id`.
    #[arg(long, env = "ANTUMBRA_ELF", global = true, default_value = "artifacts/programs/v0.3/antumbra_auction.bin")]
    elf: PathBuf,
    /// The account that pays each transaction's fee. Default: the first funded signer.
    #[arg(long, env = "ANTUMBRA_PAYER", global = true)]
    payer: Option<AccountId>,
    /// Declared execution gas for a transaction (at most 10,000,000).
    #[arg(long, global = true)]
    gas_limit: Option<u64>,
    /// Send even if the local pre-check says the program will refuse. Used to
    /// put refusals on chain on purpose.
    #[arg(long, global = true)]
    force: bool,
    /// A label for the JSON line.
    #[arg(long, global = true, default_value = "")]
    label: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Deploy the guest through program_loader; prints the header. Resumable:
    /// with `--header` and `--segment`, a step already on chain is skipped.
    Deploy {
        #[arg(long)]
        payer: AccountId,
        #[arg(long, requires = "segment")]
        header: Option<AccountId>,
        #[arg(long, requires = "header")]
        segment: Vec<AccountId>,
        /// Deploy an immutable program (no later update).
        #[arg(long)]
        immutable: bool,
    },
    /// Print the chain clock (ms). Reads `--rpc`; needs no wallet.
    Now,
    /// Print the ImageID of `--elf` as the 32 bytes a program header stores.
    ImageId,
    /// Print the config and auction PDAs of the ids. Computed locally.
    Ids {
        #[arg(long)]
        config_id: Option<String>,
        #[arg(long)]
        auction_id: Option<String>,
    },
    /// Decode an auction. Reads `--rpc`; needs no wallet.
    Show { auction_id: String },
    /// Decode a config. Reads `--rpc`; needs no wallet.
    ShowConfig { config_id: String },
    /// What a bid would buy at a time, from the stored auction. Reads `--rpc`.
    Quote {
        auction_id: String,
        #[arg(long)]
        bid: u128,
        #[arg(long, default_value = "now")]
        at: String,
    },
    InitConfig {
        #[arg(long)]
        config_id: String,
        /// WAD multiplier at open, e.g. 980000000000000000 for 2% off.
        #[arg(long)]
        min_multiplier: u128,
        /// WAD multiplier at and after the window, e.g. 800000000000000000.
        #[arg(long)]
        max_multiplier: u128,
        /// Seconds from open to the deepest discount.
        #[arg(long)]
        window: u64,
        #[arg(long)]
        admin: AccountId,
    },
    /// Open an auction. The schedule is read from the config on chain unless
    /// given; `--max-multiplier` lets a refusal be sent on purpose.
    Start {
        #[arg(long)]
        auction_id: String,
        #[arg(long)]
        config_id: String,
        #[arg(long)]
        collateral: u128,
        #[arg(long)]
        to_raise: u128,
        /// WAD: system coin per base unit of collateral.
        #[arg(long)]
        price: u128,
        #[arg(long, default_value = "now")]
        at: String,
        #[arg(long)]
        admin: AccountId,
        #[arg(long)]
        max_multiplier: Option<u128>,
    },
    Buy {
        #[arg(long)]
        auction_id: String,
        #[arg(long)]
        bid: u128,
        #[arg(long, default_value = "now")]
        at: String,
        #[arg(long)]
        bidder: AccountId,
    },
    Settle {
        #[arg(long)]
        auction_id: String,
        #[arg(long, default_value = "now")]
        at: String,
        #[arg(long)]
        caller: AccountId,
    },
}

/// A 32-byte id: 64 hex digits, or text of at most 32 bytes, zero-padded.
fn id32(s: &str) -> Result<[u8; 32]> {
    if s.len() == 64 {
        if let Ok(v) = hex::decode(s) {
            return Ok(v.try_into().expect("32 bytes"));
        }
    }
    if s.len() > 32 {
        bail!("an id is 64 hex digits or at most 32 bytes of text: {s}");
    }
    Ok(core::seed_from_str(s))
}

struct Ctx {
    /// Only the commands that sign open the wallet.
    wallet: Option<WalletCore>,
    rpc: SequencerClient,
    program: Option<AccountId>,
    elf: PathBuf,
    payer: Option<AccountId>,
    force: bool,
    label: String,
}

impl Ctx {
    fn program(&self) -> Result<AccountId> {
        self.program.ok_or_else(|| anyhow!("set --program or ANTUMBRA_PROGRAM to the program's header account"))
    }

    fn wallet(&self) -> &WalletCore {
        self.wallet.as_ref().expect("a signing command opens the wallet")
    }

    async fn account(&self, id: AccountId) -> Result<lee::Account> {
        match &self.wallet {
            Some(w) => w.get_account_public(id).await,
            None => self.rpc.get_account(id).await.map_err(|e| anyhow!("reading {id} from the sequencer: {e}")),
        }
    }

    async fn shard(&self, id: AccountId, program: AccountId) -> Result<Vec<u8>> {
        Ok(self.account(id).await?.data.shard(program).to_vec())
    }

    async fn now(&self) -> Result<u64> {
        let clock = clock_core::CLOCK_01_PROGRAM_ACCOUNT_ID;
        let acc = self.account(clock).await?;
        Ok(clock_core::ClockAccountData::from_bytes(acc.data.shard(clock_core::clock_account_id())).timestamp)
    }

    /// `now`, `now+90s`, `now-5m`, or plain milliseconds.
    async fn time(&self, s: &str) -> Result<u64> {
        let Some(rest) = s.strip_prefix("now") else {
            return s.parse().with_context(|| format!("not a time: {s}"));
        };
        let now = self.now().await?;
        if rest.is_empty() {
            return Ok(now);
        }
        let (sign, num) = rest.split_at(1);
        let (digits, unit) = num.split_at(num.len() - 1);
        let n: u64 = digits.parse().with_context(|| format!("not a time: {s}"))?;
        let ms = n * match unit {
            "s" => 1_000,
            "m" => 60_000,
            "h" => 3_600_000,
            _ => bail!("unit is s, m or h: {s}"),
        };
        Ok(match sign {
            "+" => now + ms,
            "-" => now - ms,
            _ => bail!("not a time: {s}"),
        })
    }

    async fn auction(&self, aid: &[u8; 32]) -> Result<AuctionState> {
        let p = self.program()?;
        let bytes = self.shard(auction_account(&p, aid), p).await?;
        ensure!(!bytes.is_empty(), "no auction at id {}", hex::encode(aid));
        Ok(borsh::from_slice(&bytes)?)
    }

    async fn config(&self, cid: &[u8; 32]) -> Result<Config> {
        let p = self.program()?;
        let bytes = self.shard(config_account(&p, cid), p).await?;
        ensure!(!bytes.is_empty(), "no config at id {}", hex::encode(cid));
        Ok(borsh::from_slice(&bytes)?)
    }

    fn mentions(&self, rows: Vec<ProgramShardSelector>, signers: &[AccountId]) -> Vec<AccountMention> {
        rows.into_iter()
            .map(|r| {
                let who = if signers.contains(&r.account_id) {
                    AccountIdentity::Public(r.account_id)
                } else {
                    AccountIdentity::PublicNoSign(r.account_id)
                };
                who.select_program_shard(r.program_account_id)
            })
            .collect()
    }

    /// Send `ix` and report whether `watch` (this program's shard of that
    /// account) changed. The public testnet drops transactions: one not
    /// included within five minutes is sent again, up to three attempts. Each
    /// attempt is signed with the payer's nonce as the chain has it, so while
    /// an earlier one is only late they share a nonce and at most one lands;
    /// every attempt is looked for before the next is sent, and all of them
    /// are reported.
    async fn send(&self, op: &str, ix: &Instruction, rows: Vec<ProgramShardSelector>, signers: &[AccountId], watch: AccountId) -> Result<()> {
        let program = self.program()?;
        let before = self.shard(watch, program).await?;
        let data = Program::serialize_instruction(ix.clone())?;
        let mut hashes = vec![];
        let mut found = None;
        for attempt in 1..=3 {
            match self.wallet().send_pub_tx_paid_by(self.mentions(rows.clone(), signers), data.clone(), program, self.payer).await {
                Ok(h) => hashes.push(h),
                Err(e) if hashes.is_empty() => {
                    println!("{}", serde_json::json!({"op": op, "label": self.label, "verdict": "rejected", "error": format!("{e:?}")}));
                    return Ok(());
                }
                Err(e) => eprintln!("{op}: attempt {attempt} not sent: {e:?}"),
            }
            for (i, h) in hashes.iter().enumerate() {
                let wait = if i + 1 == hashes.len() { 300 } else { 5 };
                if let Ok(Ok((_, b))) = tokio::time::timeout(Duration::from_secs(wait), self.wallet().poll_transaction(*h)).await {
                    found = Some((*h, b));
                    break;
                }
            }
            if found.is_some() {
                break;
            }
            eprintln!("{op}: attempt {attempt} not included yet");
        }
        let (hash, block) = match found {
            Some((h, b)) => (h, Some(b)),
            None => (*hashes.last().expect("one attempt was sent"), None),
        };
        let after = self.shard(watch, program).await?;
        let verdict = match block {
            None => "not-included",
            Some(_) if after != before => "applied",
            Some(_) => "reverted",
        };
        let mut line = serde_json::json!({"op": op, "label": self.label, "tx": hash.to_string(), "block": block, "verdict": verdict});
        if hashes.len() > 1 {
            line["attempts"] = hashes.iter().map(|h| h.to_string()).collect();
        }
        println!("{line}");
        Ok(())
    }
}

/// The ImageID as a program header stores it: eight u32 words, little-endian.
fn image_hex(program: &Program) -> String {
    program.id().iter().flat_map(|w| w.to_le_bytes()).map(|b| format!("{b:02x}")).collect()
}

fn auction_json(s: &AuctionState) -> serde_json::Value {
    serde_json::json!({
        "config_id": hex::encode(s.config_id),
        "collateral_seized": s.collateral_seized.to_string(),
        "collateral_left": s.collateral_left.to_string(),
        "collateral_sold": s.collateral_sold.to_string(),
        "to_raise": s.to_raise.to_string(),
        "raised": s.raised.to_string(),
        "price": s.price.to_string(),
        "opened_at": s.opened_at,
        "last_seen": s.last_seen,
        "settled": s.settled,
        "min_multiplier": s.min_multiplier.to_string(),
        "max_multiplier": s.max_multiplier.to_string(),
        "window": s.window,
        "r6_sold_plus_left_is_seized": s.collateral_sold + s.collateral_left == s.collateral_seized,
        "shortfall": s.to_raise.saturating_sub(s.raised).to_string(),
    })
}

async fn wait_for<F, Fut>(done: F, limit: Duration) -> Result<()>
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<bool>>,
{
    let start = std::time::Instant::now();
    while start.elapsed() < limit {
        if done().await.unwrap_or(false) {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(10)).await;
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let rpc = SequencerClientBuilder::default()
        .build(&cli.rpc)
        .with_context(|| format!("not a sequencer URL: {}", cli.rpc))?;
    let signs = matches!(
        cli.cmd,
        Cmd::Deploy { .. } | Cmd::InitConfig { .. } | Cmd::Start { .. } | Cmd::Buy { .. } | Cmd::Settle { .. }
    );
    let wallet = if signs {
        let mut wallet = WalletCore::from_env()
            .await
            .context("this command signs, so it needs a v0.3 wallet home (LEE_WALLET_HOME_DIR); the read-only commands do not")?;
        if let Some(g) = cli.gas_limit {
            let mut cfg = wallet.config().clone();
            cfg.gas_limit = g;
            wallet.set_config(cfg);
        }
        Some(wallet)
    } else {
        None
    };
    let ctx = Ctx {
        wallet,
        rpc,
        program: cli.program,
        elf: cli.elf.clone(),
        payer: cli.payer,
        force: cli.force,
        label: cli.label.clone(),
    };
    run(ctx, cli.cmd).await
}

async fn run(mut ctx: Ctx, cmd: Cmd) -> Result<()> {
    match cmd {
        Cmd::Deploy { payer, header, segment, immutable } => {
            let bytecode = std::fs::read(&ctx.elf).with_context(|| format!("reading {}", ctx.elf.display()))?;
            let program = Program::new(bytecode.clone().into())?;
            // The loader chunks the user ELF inside the program binary, not the binary itself.
            let user_elf = risc0_binfmt::ProgramBinary::decode(&bytecode).map_err(|e| anyhow!("{e}"))?.user_elf.to_vec();
            let segments = user_elf.len().div_ceil(program_loader_core::MAX_SEGMENT_DATA_LEN);
            let wallet = ctx.wallet.as_mut().expect("deploy opens the wallet");
            let (header, segs) = match header {
                Some(h) => {
                    ensure!(segment.len() == segments, "this binary needs {segments} segment accounts, got {}", segment.len());
                    (h, segment)
                }
                None => {
                    let h = wallet.create_new_account_public(None).0;
                    let s: Vec<AccountId> = std::iter::repeat_with(|| wallet.create_new_account_public(None).0).take(segments).collect();
                    wallet.store_persistent_data()?;
                    (h, s)
                }
            };
            // Printed before anything is sent: with these ids, `deploy --header --segment`
            // resumes a deploy that stopped half way.
            eprintln!("{}", serde_json::json!({"op": "deploy-plan", "header": header.to_string(), "segments": segs.iter().map(|s| s.to_string()).collect::<Vec<_>>()}));
            let loader = ProgramLoader(ctx.wallet());
            let loader_id = lee_core::program::PROGRAM_LOADER_ACCOUNT_ID;
            let chunks: Vec<&[u8]> = user_elf.chunks(program_loader_core::MAX_SEGMENT_DATA_LEN).collect();
            // Segments last to first (each names the next), then the header. A step already on
            // chain is skipped, and a transaction the sequencer drops is sent again.
            for i in (0..segs.len()).rev() {
                let next = segs.get(i + 1).copied();
                let mut tries = 0;
                while ctx.shard(segs[i], loader_id).await?.is_empty() {
                    tries += 1;
                    ensure!(tries <= 12, "segment {i} ({}) is not on chain after 12 sends", segs[i]);
                    match loader.write_segment(segs[i], chunks[i].to_vec(), next, Some(payer)).await {
                        Ok(h) => eprintln!("segment {i}: sent {h} (try {tries})"),
                        Err(e) => eprintln!("segment {i}: send failed (try {tries}): {e:?}"),
                    }
                    wait_for(|| async { Ok(!ctx.shard(segs[i], loader_id).await?.is_empty()) }, Duration::from_secs(600)).await?;
                }
                eprintln!("segment {i}: on chain");
            }
            let mut tries = 0;
            while ctx.shard(header, loader_id).await?.is_empty() {
                tries += 1;
                ensure!(tries <= 12, "the header {header} is not on chain after 12 sends");
                match loader.create_header(header, segs[0], &segs, immutable, Some(payer)).await {
                    Ok(h) => eprintln!("header: sent {h} (try {tries})"),
                    Err(e) => eprintln!("header: send failed (try {tries}): {e:?}"),
                }
                wait_for(|| async { Ok(!ctx.shard(header, loader_id).await?.is_empty()) }, Duration::from_secs(600)).await?;
            }
            println!("{}", serde_json::json!({"op": "deploy", "header": header.to_string(), "image_id": image_hex(&program), "segments": segs.iter().map(|s| s.to_string()).collect::<Vec<_>>()}));
        }
        Cmd::Now => println!("{}", ctx.now().await?),
        Cmd::ImageId => {
            let bytecode = std::fs::read(&ctx.elf).with_context(|| format!("reading {}", ctx.elf.display()))?;
            println!("{}", image_hex(&Program::new(bytecode.into())?));
        }
        Cmd::Ids { config_id, auction_id } => {
            let p = ctx.program()?;
            let mut j = serde_json::json!({});
            if let Some(c) = config_id {
                j["config"] = config_account(&p, &id32(&c)?).to_string().into();
            }
            if let Some(a) = auction_id {
                j["auction"] = auction_account(&p, &id32(&a)?).to_string().into();
            }
            println!("{j}");
        }
        Cmd::Show { auction_id } => {
            let s = ctx.auction(&id32(&auction_id)?).await?;
            let mut j = auction_json(&s);
            j["clock"] = ctx.now().await.unwrap_or(0).into();
            println!("{j}");
        }
        Cmd::ShowConfig { config_id } => {
            let c = ctx.config(&id32(&config_id)?).await?;
            println!(
                "{}",
                serde_json::json!({
                    "admin": c.admin.to_string(), "min_multiplier": c.min_multiplier.to_string(),
                    "max_multiplier": c.max_multiplier.to_string(), "window": c.window,
                })
            );
        }
        Cmd::Quote { auction_id, bid, at } => {
            let s = ctx.auction(&id32(&auction_id)?).await?;
            let at = ctx.time(&at).await?;
            let f = core::quote(&s, bid, at).map_err(|e| anyhow!("{e:?}"))?;
            println!(
                "{}",
                serde_json::json!({
                    "at": at, "elapsed_s": core::elapsed_secs(&s, at), "multiplier": f.multiplier.to_string(),
                    "paid": f.paid.to_string(), "collateral_out": f.collateral_out.to_string(),
                })
            );
        }
        Cmd::InitConfig { config_id, min_multiplier, max_multiplier, window, admin } => {
            let p = ctx.program()?;
            let cid = id32(&config_id)?;
            let ix = Instruction::InitConfig { config_id: cid, min_multiplier, max_multiplier, window };
            ctx.send("init-config", &ix, core::rows::init_config(&p, &cid, admin), &[admin], config_account(&p, &cid)).await?;
        }
        Cmd::Start { auction_id, config_id, collateral, to_raise, price, at, admin, max_multiplier } => {
            let p = ctx.program()?;
            let (aid, cid) = (id32(&auction_id)?, id32(&config_id)?);
            let c = match ctx.config(&cid).await {
                Ok(c) => c,
                Err(e) if ctx.force => {
                    eprintln!("{e}; sending anyway");
                    Config { admin, min_multiplier: 1, max_multiplier: 1, window: 0 }
                }
                Err(e) => return Err(e),
            };
            if !ctx.force && c.admin != admin {
                bail!("{admin} is not this config's admin ({}); use --force to send anyway", c.admin);
            }
            let at = ctx.time(&at).await?;
            let ix = Instruction::StartAuction {
                auction_id: aid,
                config_id: cid,
                collateral,
                to_raise,
                price,
                now: at,
                min_multiplier: c.min_multiplier,
                max_multiplier: max_multiplier.unwrap_or(c.max_multiplier),
                window: c.window,
            };
            ctx.send("start", &ix, core::rows::start_auction(&p, &aid, &cid, admin), &[admin], auction_account(&p, &aid)).await?;
        }
        Cmd::Buy { auction_id, bid, at, bidder } => {
            let p = ctx.program()?;
            let aid = id32(&auction_id)?;
            let at = ctx.time(&at).await?;
            if !ctx.force {
                let s = ctx.auction(&aid).await?;
                ensure!(s.settled == 0, "the auction is settled; use --force to send anyway");
                ensure!(at >= s.last_seen, "{at} is earlier than {} already honoured; use --force to send anyway", s.last_seen);
                let f = core::quote(&s, bid, at).map_err(|e| anyhow!("{e:?}"))?;
                ensure!(f.collateral_out > 0, "this bid buys nothing at {at}; use --force to send anyway");
            }
            let ix = Instruction::BuyCollateral { auction_id: aid, bid, now: at };
            ctx.send("buy", &ix, core::rows::handle(&p, &aid, bidder), &[bidder], auction_account(&p, &aid)).await?;
        }
        Cmd::Settle { auction_id, at, caller } => {
            let p = ctx.program()?;
            let aid = id32(&auction_id)?;
            let at = ctx.time(&at).await?;
            if !ctx.force {
                let s = ctx.auction(&aid).await?;
                ensure!(s.settled == 0, "the auction is already settled; use --force to send anyway");
                ensure!(
                    s.raised == s.to_raise || s.collateral_left == 0,
                    "the target is not met and collateral remains; use --force to send anyway"
                );
            }
            let ix = Instruction::SettleAuction { auction_id: aid, now: at };
            ctx.send("settle", &ix, core::rows::handle(&p, &aid, caller), &[caller], auction_account(&p, &aid)).await?;
        }
    }
    Ok(())
}
