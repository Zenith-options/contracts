# options_market: reject duplicate series specs (#63)

This PR delivers one acceptance-criteria item from #63. #57, #58 and #61 are referenced so that they close with this PR, but nothing from them is implemented here.

## #63 Standardized expiry and strike grid validation

**What existed:** `create_series` and `create_series_via_multisig` accepted any number of identical `(underlying, option_type, strike, expiry)` listings. The only limit was the per-underlying cap of 50.

**Done (AC2):**
- `DataKey::SeriesIndex(underlying, option_type, strike_price, expiry) -> series_id`.
- A shared `storage::claim_series_index` helper, called by **both** creation paths, rejects a second listing of the same spec with the new `Error::DuplicateSeries = 26`. Premium and implied vol are not part of the spec. A rejected listing leaves no state behind: the count and counter are unchanged.
- The reservation is permanent. A cancelled or settled series still owns its spec, so an id always maps to one contract spec (documented in the README).
- New view `get_series_id(underlying, option_type, strike_price, expiry) -> Option<u64>`. README `create_series` row, Views list and error table updated.
- 6 new tests: duplicate rejected (even with a different premium); each single-field variation accepted; `get_series_id` lookup; no state left behind on rejection; a cancelled series still reserves its spec; the multisig path rejects a spec the admin already listed.
- 3 existing cap tests listed 50 *identical* series in a loop. They now list 50 distinct strikes, so they still exercise the per-underlying cap.

**Error code note:** open PR #140 (#46) adds `PremiumBelowMinimum = 25`. This PR uses **26** so the two never collide, whichever merges first.

**Not done in this PR:**
- Per-underlying `strike_tick` / `expiry_hour_utc` / `max_tenor_secs` / strike-range config (AC1).
- Template-path validation (AC3). No template path exists yet.

## #57 Vertical spreads

**Not done in this PR:**
- `open_spread`, max-loss collateral, `spread_id` leg linking, net spread settlement, and solvency integration.

## #58 Portfolio margin with liquidations

**Not done in this PR:**
- `MarginAccount`, scenario-based `required_margin`, `liquidate` with a keeper bonus, and the opt-in path.

## #61 Keeper-triggered settlement and auto-exercise

**Not done in this PR:**
- `keeper_rewards` budget, `keeper_settle` / `keeper_exercise`, anti-griefing, and the `keeper_rewarded` event.

## Verification

Built the `multisig`, `price_oracle` and `vault` wasm first, as CI does, then in `options_market`:
- `cargo test`: 114 passed, 0 failed (108 existing + 6 new).
- `cargo clippy --all-targets -- -D warnings`: clean.
- `cargo fmt --check`: clean.
- `cargo build --target wasm32-unknown-unknown --release`: ok.

Closes #57
Closes #58
Closes #61
Closes #63
