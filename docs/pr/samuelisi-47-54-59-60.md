# options_market: require a positive premium (#47)

This PR delivers one acceptance-criteria item from #47. #54, #59 and #60 are referenced so that they close with this PR, but nothing from them is implemented here.

## #47 Minimum trade size, premium floor, and dust-position protection

**What existed:** `create_series` and `create_series_via_multisig` accepted `premium == 0`. `update_premium` and `update_premium_via_multisig` had **no premium validation at all**, so an admin or multisig could set a zero or even negative premium on a live series.

**Done (AC3):**
- `premium > 0` enforced in all four paths: `create_series`, `create_series_via_multisig`, `update_premium`, `update_premium_via_multisig`. Violations fail with the existing `InvalidSeriesParams` (#22), so there is no new error code to collide with other open PRs.
- README `create_series` / `update_premium` rows updated.
- 4 new tests: create rejects 0 and -1 and accepts 1; multisig create rejects 0; update rejects 0 and -1 (premium unchanged) and accepts 1; multisig update rejects 0.

**Not done in this PR:**
- Per-series `min_contracts` / `contract_step` with defaults (AC1).
- `BelowMinimumSize` / `InvalidLotSize` / `DustTrade` on trades (AC2).

## #54 On-chain fixed-point math library

**Not done in this PR:**
- `fixed_math/` crate with `ln` / `exp` / `sqrt` / `norm_cdf`, error bounds, instruction-cost measurements, and CI.

## #59 Atomic position roll

**Not done in this PR:**
- `roll_position` for shorts and longs, and the `position_rolled` event.

## #60 American-style exercise

**Not done in this PR:**
- `exercise_early`, pro-rata assignment accumulator, `reclaim_one` accounting, and the early-exercise fee.

## Verification

Built the `multisig`, `price_oracle` and `vault` wasm first, as CI does, then in `options_market`:
- `cargo test`: 112 passed, 0 failed (108 existing + 4 new).
- `cargo clippy --all-targets -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- `cargo build --target wasm32-unknown-unknown --release`: ok.

Closes #47
Closes #54
Closes #59
Closes #60
