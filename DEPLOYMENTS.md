# On the public LEZ testnet

Everything below is on `https://testnet.lez.logos.co` and re-checkable from a
clean clone in one command:

```bash
./scripts/verify-onchain.sh
```

Eighteen checks. Five of them are things that must **not** resolve, and they
are the reason the other thirteen mean anything. The script counts them rather
than stating a number, because this sentence has already been wrong once.

## The program

| | |
|---|---|
| ImageID | `5dc0e0881cb7ce3cb055e2c3ab7658f6f6f2747e8e11690e080b91f0167d07cf` |
| Deploy transaction | `c00b9698ae21fcfcff3eb05ec3f8367d1378d6df63627d8aa70082875e6c5e43` |
| Block | 20265 |
| Artifact | `artifacts/programs/antumbra_auction.bin`, 433,112 bytes |

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

## The lifecycle, driven end to end

A 2 % discount at open growing to 20 % over an hour; ten base units of
collateral seized, priced at 2000, raising 20000.

| step | transaction | block | proof |
|---|---|---|---|
| `init_config` | [`37581bb0…deb17321`](https://explorer.testnet.lez.logos.co/transaction/37581bb093e7e775710e985847f84a5baf5ca0b0709401c8378ffb93deb17321) | 20269 | 31 s |
| `start_auction` | [`1379ed3c…bab9986b`](https://explorer.testnet.lez.logos.co/transaction/1379ed3c9df06752c0dfd4749175b50de64d79317274ea2856fbce0abab9986b) | 20282 | 31 s |
| `buy_collateral` at t=0 | [`e95fe0d0…b37117029`](https://explorer.testnet.lez.logos.co/transaction/e95fe0d05c3dd009b6df578754f6d0a0332cda482aa42a9c0803915b37117029) | 20283 | 62 s |
| `buy_collateral` at t=1800 | [`fd108f3f…e1234ae3e`](https://explorer.testnet.lez.logos.co/transaction/fd108f3f7e681971d81c3ea7cebb277506b7fa294f205737ca6b5b7e1234ae3e) | 20284 | 62 s |
| the bid that exhausts the lot | [`5ad0ab52…4116d0ca`](https://explorer.testnet.lez.logos.co/transaction/5ad0ab52648edf1de9c0bfce49e9e0d294eb0619bf9e13e2377438524116d0ca) | 20306 | 31 s |

Read off the auction account afterwards, and checked against the library:

```
                                on chain              the library computes
first fill                      1020408163265306122   1020408163265306122
second fill                     1123595505617977528   1123595505617977528
collateral left                 7855996331116716350   7855996331116716350
```

**The same bid bought 10.11 % more collateral half an hour later.** That is F2 —
the discount escalating — on the public chain rather than in a paragraph. And
`collateral_seized` still equals what left plus what remains, to the base unit,
which is R6.

## Two configurations, two auctions, at the same time

A program's id is its code, so "each product deploys its own configured
instance" is a config account rather than a deployment. A second one, with a
sharper schedule — 5% at open reaching 40% in ten minutes instead of an hour:

| step | transaction | block |
|---|---|---|
| `init_config` (faster schedule) | [`1fdf1a30…2aeb0de3`](https://explorer.testnet.lez.logos.co/transaction/1fdf1a3052d904f9549fc85b8d9cbd708f60f54c36345b1fc33c3f532aeb0de3) | 20314 |
| `start_auction` #2 under it | [`4a69eaf6…c5c5f14d`](https://explorer.testnet.lez.logos.co/transaction/4a69eaf6d5e963a814c2cab756104a79e3423f9ce861b91024b948b4c5c5f14d) | 20315 |
| `buy_collateral` on #2 | [`961f4371…2dd6d22e`](https://explorer.testnet.lez.logos.co/transaction/961f437162ce144fed0e462a783c8908862e3786a782353f9fb428132dd6d22e) | 20316 |

The fill on the second auction priced at its own schedule —
`1052631578947368421` base units, which is what the library computes for a 5%
discount — and the first auction's account did not move. That is R3: each
auction is its own PDA, so concurrency is isolation rather than a lock.

## What it refused, and what that cost

| | transaction | |
|---|---|---|
| a bid dated before a fill already honoured | `25c6014d…09ad92d51e` | refused |
| settling an auction neither sold out nor funded | `d7de1bc3…cbbbc112` | refused |
| a bid on an auction that does not exist | `a61aed9e…2f21cbfd` | refused |
| settling an auction already settled | `1a16f15f…6541b28f` | refused |
| a hash that was never deployed | `dede…dede` | never existed |

Each refusal is recorded by hash because on this runtime that is the only trace
one leaves. There is no receipt mechanism — `getTransactionReceipt`, `getEvents`
and `getLogs` all answer `Method not found`, which `verify-onchain.sh` asserts on
every run — so a refused instruction surfaces as *"Transaction not found in
preconfigured amount of blocks"* and nothing else. After each refusal the auction
account was byte-identical to before it.

**A refusal costs a full proof: 430 s, against 62 s for a fill.** That number is
a design constraint, not trivia. A liquidator bot cannot probe by trying —
seven minutes and a proof per rejected attempt, with no diagnostic. It has to
read account state before submitting, which is precisely what RFP-014 believed
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
```

Run from the repository root, not from the program directory: the Docker build
context is taken from the working directory, and the guest depends on the
arithmetic crate by path.

Needs Docker (the guest builder image is `linux/amd64` and runs under emulation
on Apple silicon), `cargo risczero` 3.0.5, and `spel`.

`cargo risczero build` emits **two** files. `antumbra_auction` is a bare ELF and
does not deploy; `antumbra_auction.bin` is R0BF-wrapped and does. Their first
four bytes tell them apart: `7f454c46` against `52304246`.
