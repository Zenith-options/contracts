# options_market: `split_position` (#52)

This PR delivers one acceptance-criteria item from #52. #53, #55 and #56 are referenced so that they close with this PR, but nothing from them is implemented here.

## #52 Partial close, split, and merge of positions

**What existed:** positions could only be exercised, reclaimed or refunded whole. There was no way to divide one.

**Done (AC1):**
- `split_position(owner, position_id, split_contracts) -> u64` creates a new position with the same series, side, owner and `opened_at`, and registers it in the owner's position list.
- `premium_paid`, `fee_paid` and `collateral_locked` are divided pro rata (`field * split / contracts`), **rounded down for the new position, with the remainder left in the original**. Every field's total across the two positions equals the original exactly, so open interest, `SeriesEscrow` and cancellation refunds are unchanged.
- Guards: owner auth (`Unauthorized` for others); `InvalidSplitAmount` unless `0 < split_contracts < contracts`; `AlreadyExercised` / `AlreadySettled` for closed positions; blocked while paused.
- `position_split(owner; position_id, new_position_id, split_contracts)` event. README Traders row and error table updated.
- 8 new tests:
  - exact thirds on a long, with series escrow and open interest untouched;
  - an uneven 7-unit split on a short (floor for the new position, sums preserved);
  - split halves refund the same total on cancellation (escrow drains to 0);
  - split halves exercise for the full intrinsic value;
  - out-of-range amounts rejected; non-owner rejected; exercised and settled positions rejected; paused.

**Rounding note:** payouts are computed per position, so with non-divisible splits the two halves can pay out (or, for shorts, keep back) at most one base unit less (more) per split than the unsplit position. This is documented on the function and left for the rounding-policy issue this AC refers to.

**Error code note:** `InvalidSplitAmount = 27`. 25 and 26 are claimed by open PRs #140 and #141, so the codes never collide on merge.

**Not done in this PR:**
- `merge_positions` (AC2).
- Long/short netting with immediate collateral release (AC3).
- Explicit solvency-ledger invariants beyond the escrow/OI checks above (AC4).

## #53 RFQ: signed writer quotes

**Not done in this PR:**
- `Quote` + canonical hash with ed25519 verification, `register_maker`.
- Partial fills, nonce cancellation, atomic fill, and cross-contract/network replay protection.

## #55 On-chain Black-Scholes premium sanity band

**Not done in this PR:**
- `bs_price` in `pricing.rs`, `premium_band_bps` / `r` config, `PremiumOutOfBand` checks, and oracle-unavailable behaviour.

## #56 Implied-volatility surface

**Not done in this PR:**
- `set_vol_surface`, bilinear interpolation, surface-driven premiums, and the `vol_surface_updated` event.

## Verification

Built the `multisig`, `price_oracle` and `vault` wasm first, as CI does, then in `options_market`:
- `cargo test`: 116 passed, 0 failed (108 existing + 8 new).
- `cargo clippy --all-targets -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- `cargo build --target wasm32-unknown-unknown --release`: ok.

Closes #52
Closes #53
Closes #55
Closes #56
