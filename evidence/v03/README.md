# antumbra_auction on LEZ v0.3: a local end-to-end run

A fresh local LEZ v0.3.1 standalone sequencer (`1a003bfb`, the version the
public testnet runs), the v0.3.0 wallet, the binary then committed (kept at
`artifacts/programs/v0.3/deployed-dc3618e8/antumbra_auction.bin`) deployed through
`program_loader` at header `B31q8iMw3yXS5Ghu2R53P2N3HGv5o2Vj62uH22nFCMGi`, ImageID
`dc3618e80b2645906b70c5aee9c6e1be50d985fc00f031c3dc03e0ad9fd9384f`, and
`scripts/e2e-v03.sh` driving the whole lifecycle through the
`antumbra-auction` CLI. The same run on the public testnet (2026-10-15, header `ADLHPytqfLnzM2hfoJUx6ZXPLwFo6o4RmqXWRJGETMJ7`):
`testnet.tsv`, `testnet-transcript.txt`, `testnet-verify.txt`, 26 checks, 0 failures.

**Result: 8 transactions applied, 12 refused exactly where they must be, 11
state checks read back from the chain, 0 failures; the verifier re-checks the
manifest at 26 checks, 0 failures.**

| File | What it is |
|---|---|
| `local-transcript.txt` | the run's output, one JSON line per transaction and one line per check |
| `local-e2e.tsv` | the manifest: every transaction with its expectation, verdict, block and hash, the program header and ImageID, and the final state of both configs and both auctions |
| `local-verify.txt` | `scripts/verify-onchain.sh --manifest evidence/v03/local-e2e.tsv --rpc http://127.0.0.1:3042` |

What the run covers: two configurations side by side under different admins
(F7, R3); a config id used twice and a schedule that cannot be evaluated,
refused; an auction opened only by its config's admin and only with the
schedule its config holds; partial fills at an escalating discount, the same
bid buying more collateral 60 s later (F2, F3); a bid priced at a clock earlier
than one already honoured, refused; a settle on an unfinished auction, refused;
the lot running out, the last bid re-struck and the shortfall booked, with
`raised` equal to the v0.2.4 testnet figure to the base unit
(`16569594129786746160000`) and `sold + left == seized` read from the account
(R6); settling twice and bidding after settlement, refused (F6); a second
auction whose target is met exactly by a clamped bid; and a bid on an auction
that does not exist. The window is 120 s instead of the testnet run's hour, so
the run takes minutes.

On v0.3 a refused public transaction is included in a block and charged, so
"reverted" in the manifest means included with no effect, which the final-state
lines pin down. The fee payer spent 1,226,706,046 atomic units on this chain
over the run, the deploy included.
