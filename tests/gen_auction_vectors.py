#!/usr/bin/env python3
# SPDX-License-Identifier: MIT OR Apache-2.0
"""Exact vectors for the auction quote, from a second implementation.

The reference here is Python's arbitrary-precision integers, so the Rust is
compared against a DIFFERENT language's arithmetic rather than against a second
copy of itself. A differential test whose reference shares the implementation's
assumptions tests nothing.

What is modelled is the *composition*, not the primitives: `mul_div_floor` and
`mul_div_ceil` are lifted from the sibling `antumbra-lez` crate and already have
their own differential vectors there. What has none is what this file covers —
the discount multiplier, the two clamps, and the re-strike that fires when the
lot rather than the target runs out. That re-strike is where a real defect lived:
before it, a bid of 4,000 against a lot worth 2,368 was charged the whole 4,000
and the target reported as fully raised, hiding the shortfall.

    python3 tests/gen_auction_vectors.py > tests/vectors/quote.txt
"""
import random

U128 = (1 << 128) - 1
WAD = 10**18
random.seed(0xA0C7_10E5)          # deterministic: a failure is reproducible


def mul_div(a, b, d, up):
    """floor/ceil(a·b/d) in 256 bits, or None when the quotient needs >128."""
    if d == 0:
        return None
    q, r = divmod(a * b, d)
    if up and r:
        q += 1
    return q if q <= U128 else None


def multiplier_at(mn, mx, window, elapsed):
    if mn == 0 or mn > WAD or mx == 0 or mx > mn:
        return "DiscountOutOfRange"
    if window == 0 or elapsed >= window:
        return mx
    travelled = mul_div(mn - mx, elapsed, window, False)
    if travelled is None:
        return "Overflow"
    return mn - travelled


def quote(mn, mx, window, left, to_raise, price, bid, elapsed):
    """The reference quote. Returns (multiplier, paid, collateral_out) or a kind."""
    if left == 0 or to_raise == 0:
        return "NothingLeftToSell"
    if price == 0:
        return "DivideByZero"
    m = multiplier_at(mn, mx, window, elapsed)
    if isinstance(m, str):
        return m
    paid = min(bid, to_raise)
    discounted = mul_div(price, m, WAD, False)
    if discounted is None:
        return "Overflow"
    if discounted == 0:
        return "DivideByZero"
    raw = mul_div(paid, WAD, discounted, False)
    if raw is None:
        return "Overflow"
    out = min(raw, left)
    if out < raw:
        restruck = mul_div(out, discounted, WAD, True)
        if restruck is None:
            return "Overflow"
        paid = min(restruck, paid)
    return (m, paid, out)


def cases():
    """Ordinary sizes first, then the edges the clamps and the re-strike live on."""
    # Hand-picked: each one is a boundary somebody could get wrong.
    yield (98 * WAD // 100, 80 * WAD // 100, 3600, 5000 * WAD, 5000 * WAD, 2 * WAD, 4000 * WAD, 0)
    yield (98 * WAD // 100, 80 * WAD // 100, 3600, 1184 * WAD, 5000 * WAD, 2 * WAD, 4000 * WAD, 1800)
    yield (98 * WAD // 100, 80 * WAD // 100, 3600, 1, 5000 * WAD, 2 * WAD, 4000 * WAD, 3600)
    yield (WAD, WAD, 0, 10**30, 10**30, WAD, 10**30, 0)          # no discount at all
    yield (WAD, 1, 1, 10**30, 10**30, WAD, 10**30, 10**9)        # multiplier floored to 1
    yield (98 * WAD // 100, 80 * WAD // 100, 3600, 5000 * WAD, 1, 2 * WAD, 4000 * WAD, 0)
    yield (98 * WAD // 100, 80 * WAD // 100, 3600, 5000 * WAD, 5000 * WAD, 1, 4000 * WAD, 0)
    yield (0, 0, 1, 1, 1, 1, 1, 0)                               # discount out of range
    yield (WAD, WAD, 3600, 0, 5000 * WAD, 2 * WAD, 1, 0)         # nothing left
    yield (WAD, WAD, 3600, 5000 * WAD, 5000 * WAD, 0, 1, 0)      # zero price
    yield (WAD, 1, 10**9, U128, U128, U128, U128, 1)             # everything at the ceiling
    n = 0
    while n < 3000:
        mode = n % 6
        mx = random.randint(1, WAD)
        mn = random.randint(mx, WAD)
        window = random.choice([0, 1, 60, 3600, 86400, 10**9])
        elapsed = random.choice([0, 1, window // 2 if window else 0, window, window + 1, 10**9])
        if mode == 0:                       # ordinary
            left, to_raise = random.randint(1, 10**24), random.randint(1, 10**24)
            price, bid = random.randint(1, 10**20), random.randint(1, 10**24)
        elif mode == 1:                     # the lot clamps
            left = random.randint(1, 10**6)
            to_raise, price, bid = 10**24, random.randint(1, 10**20), 10**24
        elif mode == 2:                     # the target clamps
            to_raise = random.randint(1, 10**6)
            left, price, bid = 10**30, random.randint(1, 10**20), 10**24
        elif mode == 3:                     # tiny everything, where rounding decides
            left, to_raise = random.randint(1, 3), random.randint(1, 3)
            price, bid = random.randint(1, 3), random.randint(1, 3)
        elif mode == 4:                     # near the 128-bit ceiling
            left, to_raise = random.randint(U128 // 2, U128), random.randint(U128 // 2, U128)
            price, bid = random.randint(1, U128), random.randint(U128 // 2, U128)
        else:                               # a price so small the discount floors it away
            left, to_raise = 10**24, 10**24
            price, bid = random.randint(1, 10), random.randint(1, 10**24)
        yield (mn, mx, window, left, to_raise, price, bid, elapsed)
        n += 1


print("# min max window left to_raise price bid elapsed | multiplier paid out"
      "   -- a single token instead of three is the refusal's name")
for c in cases():
    r = quote(*c)
    tail = r if isinstance(r, str) else "%d %d %d" % r
    print("%s | %s" % (" ".join(str(x) for x in c), tail))
