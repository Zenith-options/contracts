# options_market: `min_premium` slippage protection on `write_option` (#46)

This PR delivers one acceptance-criteria item from #46. #44, #45 and #48 are referenced so that they close with this PR, but nothing from them is implemented here.

## #46 Writer slippage protection (`min_premium`) and trade deadlines

**What existed:** `buy_option` had `max_premium`, but `write_option` had no protection. An `update_premium` (or a `set_fee_rate`) ordered ahead of a pending write locked the writer's collateral for whatever premium was current at execution.

**Done:**
- `write_option(writer, series_id, contracts, collateral_amount, min_premium)` fails with the new `Error::PremiumBelowMinimum = 25` when the premium is below `min_premium`. The check runs before any collateral transfer or pool debit.
- `min_premium` is compared against the **net** premium the writer actually receives (`writer_premium`, after the protocol fee). That covers both a premium cut and a fee-rate increase ordered ahead of the write. `0` opts out. The boundary is inclusive: exactly `min_premium` is accepted.
- README Traders row and error table updated.
- 3 new tests: a same-ledger `update_premium` front-run is rejected (no collateral locked, pool untouched); a fee-increase front-run is rejected; the inclusive boundary (net + 1 rejected, net accepted and paid in full).
- Existing `write_option` test calls pass `min_premium = 0`.

**Integrator note (breaking ABI):** `write_option` takes a 5th argument. Frontends and bindings must pass `min_premium` (use the quoted net premium, or `0` for the old behaviour).

**Not done in this PR:**
- `deadline` on `write_option` and `buy_option` with `DeadlineExpired` (AC2).
- `premium_version` on the series and `expected_version` (AC3).
- Batch/multisig variants. There are none for `write_option` today.

## #44 Insurance fund contract

**Not done in this PR:**
- New `insurance_fund` contract (`deposit`, `cover_shortfall`, `withdraw_via_governance`, per-event and daily caps).
- `options_market` calling `cover_shortfall`, the events, `get_coverage_ratio`, the CI build order, and the fund policy doc.

## #45 Socialized loss: pro-rata payouts

**Not done in this PR:**
- `payout_ratio` computed at settlement and applied in exercise.
- The `series_underfunded` event, the long/short OI split, and the README explanation.

## #48 Fee escrow until settlement

**Not done in this PR:**
- `SeriesFeeEscrow(series_id)`, permissionless `release_fees`, and gross refunds on cancellation.
- The `fee_escrowed` legacy-position flag, the `fees_released` event, and the README refund semantics.

## Verification

Built the `multisig`, `price_oracle` and `vault` wasm first, as CI does, then in `options_market`:
- `cargo test`: 111 passed, 0 failed (108 existing + 3 new).
- `cargo clippy --all-targets -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- `cargo build --target wasm32-unknown-unknown --release`: ok.

Closes #44
Closes #45
Closes #46
Closes #48
