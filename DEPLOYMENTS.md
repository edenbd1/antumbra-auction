# On the public LEZ testnet

Everything below is on `https://testnet.lez.logos.co` and re-checkable from a
clean clone in one command:

```bash
./scripts/verify-onchain.sh
```

The script counts its own checks rather than stating a number, because this
sentence has already been wrong once. Some of them are things that must **not**
resolve, and they are the reason the rest mean anything.

## Two programs, and why there are two

The first build of this engine had a defect. It is still deployed, its accounts
are still readable, and one of them holds the defect's own output — because a
finding you can fetch is worth more than a finding you describe.

| | first build | the one to use |
|---|---|---|
| ImageID | `5dc0e0881cb7ce3cb055e2c3ab7658f6f6f2747e8e11690e080b91f0167d07cf` | `38ab5886d20784c8322d1c6a3595cc8a666568946acb63248bac8ecf1149bd57` |
| Deploy transaction | `c00b9698…5e6c5e43`, block 20265 | [`ac43ac1a…9607a75b`](https://explorer.testnet.lez.logos.co/transaction/ac43ac1a8833f87f623b3346be551ece27da0e3a0c049de3ed86ae4b9607a75b), block 20377 |
| Artifact | 433,112 bytes | `artifacts/programs/antumbra_auction.bin`, 435,440 bytes |

**Two digests, not one.** The **ImageID** is what the program is called by —
RISC0 derives it from the guest ELF, `spel program-id` prints it, and a driven
transaction carries it as its `program_id`. `SHA256(u32_le(len) ‖ bytecode)`
over the packaged binary is a different number: it is what the **deploy
transaction** hashes to. Running them together is an easy mistake and they are
kept apart here deliberately.

The deploy hash is knowable before submitting, because a deployment carries only
the bytecode — no signer, no nonce, no fee. `verify-onchain.sh` recomputes it
from the committed artifact rather than reading it from the table above: a hash
written down beside a binary proves only that someone wrote it down.

A program's id *is* its code, so these are two program ids because they are two
different programs. That is also why "each product deploys its own configured
instance" has to mean a config account rather than a deployment.

## The defect, and where to fetch it

Reading the live account of the first build's second auction is what found it.
The auction sat with `3.947368421052631579` units of collateral left and 4,000
still to raise, and the arithmetic said a bid of 4,000 would take all three of
those things — the lot, the whole bid, and the shortfall.

`quote` clamped the collateral handed out to what the lot still held, but went
on charging the bid that had been priced against a larger lot. So a bidder
overpaid, and — worse — the auction recorded `raised == to_raise` and reported
itself **fully raised** while the collateral it actually sold was worth 1,631.58
less. A shortfall that is never booked is a shortfall the debt auction in F4
never hears about.

That bid was submitted, on purpose, so the defect has a public address:

| | |
|---|---|
| Transaction | [`e46fb1ac…77555ae2`](https://explorer.testnet.lez.logos.co/transaction/e46fb1acefa644119e728980e64c82eab1a7f6ae5cf5e9ef1cfe5e5f77555ae2), block 20376 |
| Account | `9xMhmHEsJXX18f4rBSwFkRKA9kwE2tQBEnKSCXpdK2yB` |
| What it now reads | `raised` 5000·10¹⁸, `to_raise` 5000·10¹⁸ — **fully raised** |
| What was really sold | 3.947368421052631579 units, worth 2368.42 at the struck price |
| Shortfall erased | 1631.578947368421052600 |

The fix re-strikes the price when it is the *lot* that ran out rather than the
target: the bidder pays for the collateral they receive, rounded towards the
auction, and never more than the bid it replaces. Two tests fail without it.

**A second defect, in the same reading.** The first build asserted its R6
invariant as `left + (seized - left) != seized`, which is an identity — true for
every value of every field, including the ones a leak would produce. It could
never have fired. `AuctionState` now carries `collateral_sold`, moved
independently of `collateral_left`, and the assertion compares the two. That
also means **anyone holding the account can check R6 without trusting us**:
`collateral_sold + collateral_left == collateral_seized`.

## The lifecycle, driven end to end on the fixed build

A 2 % discount at open growing to 20 % over an hour; ten base units of
collateral seized, priced at 2000, raising 20000.

| step | transaction | block |
|---|---|---|
| `init_config` | [`887c7b7e…8ec95398`](https://explorer.testnet.lez.logos.co/transaction/887c7b7e43c98265ff1c671b080c077b9138dbae23df73481f433e868ec95398) | 20378 |
| `start_auction` | [`53004d4c…bde1fa8e`](https://explorer.testnet.lez.logos.co/transaction/53004d4c5b5bf1582f7df191fa15dfe68e4bd3f18cb2d4c6f8f7e915bde1fa8e) | 20379 |
| `buy_collateral` at t=0 | [`66fe3871…639fec60`](https://explorer.testnet.lez.logos.co/transaction/66fe3871ccc9a5838bdc437d0a228c5af35e9f2d0a1ef4288f35ff20639fec60) | 20380 |
| `buy_collateral` at t=1800 | [`559ef1a1…27df8a74`](https://explorer.testnet.lez.logos.co/transaction/559ef1a16ce9d1c1e99417b30300f9291aef2b6f213acb096e1d5f2c27df8a74) | 20381 |
| a bid of 16000 on a lot worth 12569.59 | [`3a5941a2…35190b9d`](https://explorer.testnet.lez.logos.co/transaction/3a5941a2dfe5b24db331005d9e50027f174992037d90096d90cc18f635190b9d) | 20382 |

Read off `F5cKK2ubAGUYqR5Hbm26GWPqv5zV19GqrJJQQDdzTmV9` afterwards:

```
collateral_seized     10000000000000000000
collateral_sold       10000000000000000000
collateral_left                          0     sold + left == seized, which is R6
raised            16569594129786746160000
to_raise          20000000000000000000000
settled                                  1
```

Three things at once, and none of them is a paragraph:

**The discount escalated.** The same 2000 bought `1020408163265306122` at t=0
and `1123595505617977528` at t=1800 — **10.11 % more collateral for the same
money**, which is F2.

**The last bid was re-struck.** 16000 was offered; 12569.594129786746160 was
taken, because that is what the remaining 7.855996331116716350 units were worth
at a 20 % discount. The library computes the same figure to the base unit.

**And the auction closed short, and said so.** `raised` is 3430.405870213253840
under `to_raise`. On the first build the identical bid would have recorded
20000 and reported nothing wrong. That difference is the whole point of F4.

## Two configurations, two auctions, at the same time

A second config with a sharper schedule — 5 % at open reaching 40 % in ten
minutes instead of an hour — and an auction under it, running while the first
was live:

| step | transaction | block |
|---|---|---|
| `init_config` (faster schedule) | [`0871332b…835d147f`](https://explorer.testnet.lez.logos.co/transaction/0871332b40f5974c9ec81935641eac4b92f6b20a7c30b2e72a8115f6835d147f) | 20383 |
| `start_auction` #2 under it | [`260b0e5a…60228d8b`](https://explorer.testnet.lez.logos.co/transaction/260b0e5af35e850bab621cfecaa5196c7ff534c1b867eabffdad032f60228d8b) | 20384 |
| `buy_collateral` on #2 at t=300 | [`a63aa570…0af777d65`](https://explorer.testnet.lez.logos.co/transaction/a63aa5702e219b834abf9e9796ae3e72659627e791261f560fa0c660af777d65) | 20385 |

Each auction is its own PDA, so concurrency here is isolation rather than a
lock. That is R3.

## What it refused, and what that cost

Each refusal is recorded by hash because on this runtime that is the only trace
one leaves. There is no receipt mechanism — `getTransactionReceipt`, `getEvents`
and `getLogs` all answer `Method not found`, which `verify-onchain.sh` asserts on
every run — so a refused instruction surfaces as *"Transaction not found in
preconfigured amount of blocks"* and nothing else.

**A refusal costs a full proof: 430 s, against 62 s for a fill.** That number is
a design constraint, not trivia. A liquidator bot cannot probe by trying — seven
minutes and a proof per rejected attempt, with no diagnostic. It has to read
account state before submitting, which is precisely what RFP-014 believed
LP-0012's events had made unnecessary.

## The explorer lags the sequencer

The transactions above are live on the sequencer now. The block explorer indexes
roughly an hour and three quarters behind, so a link may read *"Transaction not
found"* for a while after the RPC has it — the same page an impossible hash
returns. Judge by `verify-onchain.sh`, which asks the sequencer; the explorer
catches up.

## Reproducing the binary

```bash
cargo generate-lockfile --manifest-path programs/auction/Cargo.toml   # if missing
cargo risczero build --manifest-path programs/auction/Cargo.toml
cp programs/auction/target/riscv32im-risc0-zkvm-elf/docker/antumbra_auction.bin \
   artifacts/programs/
spel program-id artifacts/programs/antumbra_auction.bin
spel generate-idl programs/auction/src/bin/antumbra_auction.rs > idl/antumbra_auction.idl.json
```

Run from the repository root, not from the program directory: the Docker build
context is taken from the working directory, and the guest depends on the
arithmetic crate by path.

Needs Docker (the guest builder image is `linux/amd64` and runs under emulation
on Apple silicon), `cargo risczero` 3.0.5, and `spel`.

`cargo risczero build` emits **two** files. `antumbra_auction` is a bare ELF and
does not deploy; `antumbra_auction.bin` is R0BF-wrapped and does. Their first
four bytes tell them apart: `7f454c46` against `52304246`.

The IDL is generated from the program source, not maintained by hand — a field
added to an account and forgotten in the IDL decodes every later field at the
wrong offset, and prints confident nonsense rather than failing.
