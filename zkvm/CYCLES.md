# Measured cycle counts

Every figure here is produced by `zkvm/`, not typed. Reproduce it with:

```
cd zkvm && cargo run --release
```

The harness runs the guest under the RISC0 **executor**, not the prover: these
are the counts that decide whether an operation fits the budget, and they are
deterministic, so the same guest ELF always reports the same work. Each row is a
`cycle_count()` delta with the measurement's own overhead subtracted — 90 cycles,
measured in the same run — and `black_box` keeps the optimiser from hoisting a
pure function out of the region being timed.

The fixtures are compiled into the guest rather than fed from the host, so the
table is reproducible from the binary alone.

## The table

Median over the cases named below, against the 33,554,432-cycle session budget.

| op | cases | median | min | max | share of budget |
|---|---:|---:|---:|---:|---:|
| `multiplier_at` | 6 | 8,903 | 97 | 8,903 | 0.027 % |
| `quote` | 36 | 27,596 | 18,837 | 37,851 | 0.082 % |
| `settle` | 6 | 180 | 178 | 181 | 0.001 % |
| `health` | 6 | 367 | 366 | 367 | 0.001 % |
| `liquidator_reward` | 6 | 18,960 | 18,958 | 18,960 | 0.057 % |
| `accepts` | 6 | 104 | 104 | 104 | 0.000 % |
| `mul_div_floor` | 6 | 10,243 | 10,140 | 10,340 | 0.031 % |
| `mul_div_ceil` | 6 | 10,248 | 10,145 | 10,345 | 0.031 % |

One whole bid — quote the fill, then apply it — is **27,753** cycles end to end,
which is 0.083 % of the budget. That is the number RFP-014's P2 and P3 are
about: P3 asks for a bid inside one block, and P2 asks for many concurrent
auctions inside the compute limit. The answer to P2 is not this number alone but
what it is multiplied by: nothing. Each auction is its own account, so a bid
touches one of them and the cost does not grow with how many are live.

## What the cases are

`multiplier_at` is measured at six points of the schedule: the instant it opens,
one second in, the middle where the multiplier is interpolated, the deadline,
and past it. The spread is the whole story — 97 cycles when the schedule is
past its window and the answer is a constant, 8,903 when the interpolation runs.

`quote` is those six moments against six auctions: ordinary sizes, a bid the
target clamps, a bid the lot clamps, a lot of one base unit, and an auction at
a quarter of the 128-bit ceiling. Thirty-six measurements, and the range is what
the clamps cost.

`liquidator_reward` and the two `mul_div` rows carry the same shape for the same
reason: they divide in 256 bits, and the long division is the cost.
