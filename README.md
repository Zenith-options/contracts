# Zenith Contracts

Soroban (Stellar smart contract) crates for the Zenith options protocol.

## Crates

- [`options_market/`](options_market) — European-style put/call options on
  XLM, BTC, ETH, and SOL. Premium is set by the admin (computed off-chain via
  Black-Scholes); writers lock collateral, buyers pay premium, settlement
  happens at expiry against an oracle-reported price.
- [`price_oracle/`](price_oracle) — the on-chain logic behind that
  oracle-reported price. options_market previously only ever trusted a
  bare `oracle: Address` with nothing backing it; this contract is a
  small set of admin-authorized feeders reporting per-symbol prices,
  aggregated as a median across whatever's still fresh, gated by an
  admin-settable minimum quorum of fresh reports (`min_reports`) so a
  single feeder can't single-handedly determine the aggregate just
  because every other feeder happened to go stale or get removed.
  `options_market::set_settlement_price_from_oracle` cross-calls a live
  price_oracle deployment to settle a series permissionlessly, as an
  alternative to the original `set_settlement_price`'s trusted-oracle-
  address flow.
- [`vault/`](vault) — a per-tag escrow ledger for a single token,
  motivated by a gap discovered while testing options_market: that
  contract holds every writer's collateral and every buyer's premium in
  one undifferentiated balance, with no accounting of which balance is
  actually earmarked for which position. `deposit`/`withdraw` here are
  scoped to a caller-defined `tag` (e.g. a position_id), so a withdrawal
  can never draw down more than was specifically deposited under that
  tag — regardless of what the vault's raw token balance happens to be
  from other tags' deposits. `options_market::escrow_series_to_vault` /
  `claim_refund_from_vault` now wire this in for the specific gap it was
  built for — see "Known gaps" below for the scope of that integration.
- [`multisig/`](multisig) — M-of-N approval tracking for opaque,
  caller-defined actions, motivated by every other contract here having
  a single `admin: Address` as its sole point of control. A fixed
  signer set and threshold, `is_approved(action_id)` that flips true
  once enough signers have approved — `action_id` is never interpreted
  by this contract, only counted. Approvals carry an immutable
  `approval_ttl` (set at `initialize`, zero = never expires) so a vote
  cast for a long-abandoned action can't silently still be sitting at
  threshold if that `action_id` is ever reused later. `options_market`,
  `price_oracle`, and `vault` each cross-call a live Multisig deployment
  as a permissionless alternative to their own admin-gated
  `pause`/`unpause`/`transfer_admin`; see "Known gaps" below for what's
  still admin-only.

## Building and testing

Each crate is standalone (no workspace `Cargo.toml`), but **build order
matters for options_market, price_oracle, and vault**: all three
cross-call other contracts via soroban-sdk's `contractimport!` against
their *compiled wasm* (not a normal source dependency — that would
link the other contract's own functions into the caller's wasm and
collide with functions of the same name, like `pause`/`transfer_admin`).
options_market depends on price_oracle's, multisig's, AND (since the
`escrow_series_to_vault`/`claim_refund_from_vault` integration) vault's
wasm; price_oracle and vault each depend on multisig's wasm. That means
the dependency wasm has to exist before the dependent crate can be
compiled at all, even natively:

```sh
cd multisig
cargo build --target wasm32-unknown-unknown --release   # do this FIRST — price_oracle, vault, and options_market all need it

cd ../price_oracle
cargo build --target wasm32-unknown-unknown --release   # do this SECOND — options_market needs it, and price_oracle itself needs multisig's wasm to already exist

cd ../vault
cargo build --target wasm32-unknown-unknown --release   # do this THIRD — options_market needs it too, and vault itself needs multisig's wasm to already exist

cd ../options_market   # now this crate can build/test/etc.
cargo build                                   # native build, fast iteration
cargo test                                    # unit tests (soroban-sdk testutils)
cargo clippy --all-targets -- -D warnings     # matches CI
cargo fmt --check                             # matches CI
cargo build --target wasm32-unknown-unknown --release   # the real deploy artifact
```

`vault` only needs multisig's wasm built first, no other dependency of
its own (options_market depending on vault's wasm doesn't run the other
way). `multisig` itself has no dependency on anything else and can be
built/tested independently, in any order relative to the others.

CI (`.github/workflows/ci.yml`) builds the required dependency wasm(s)
first whenever a job is about to touch options_market, price_oracle,
or vault, then runs the same four checks against every push and PR,
for all four crates.

Every event any of these four contracts publishes has a test that
decodes its actual payload via `TryFromVal` (topics and data), not
just a test that confirms an event fired — the intent being that
anything an off-chain indexer would need to parse out of an event is
pinned down by a test, so a change to a tuple's field order or type
shows up as a test failure rather than as a silently broken indexer.

## Deploying

```sh
soroban contract deploy \
  --wasm target/wasm32-unknown-unknown/release/zenith_options_market.wasm \
  --source <your-identity> \
  --network testnet
```

Then initialize it once, from the admin's identity:

```sh
soroban contract invoke --id <contract-id> --source <admin> --network testnet -- \
  initialize \
  --admin <admin-address> \
  --oracle <oracle-address> \
  --collateral_token <usdc-sac-address> \
  --fee_recipient <fee-recipient-address>
```

`price_oracle` deploys and initializes the same way, with just an
`--admin` argument:

```sh
soroban contract deploy \
  --wasm target/wasm32-unknown-unknown/release/zenith_price_oracle.wasm \
  --source <your-identity> \
  --network testnet

soroban contract invoke --id <contract-id> --source <admin> --network testnet -- \
  initialize --admin <admin-address>
```

## `options_market` reference

All amounts are fixed-point at `PRICE_PRECISION` (1e7) unless noted —
e.g. `10_000_000` means "1.0 contracts" or "1.0 units of the underlying."
Rates (implied vol, collateral ratio, fee bps) use their own precision as
documented per field.

### Admin

| Function | Description |
|---|---|
| `initialize(admin, oracle, collateral_token, fee_recipient)` | One-time setup. Panics with `AlreadyInitialized` if called twice. |
| `transfer_admin(new_admin)` | Hands off control. Requires the **current** admin's signature. |
| `transfer_admin_via_multisig(multisig_contract, action_id, new_admin)` | Permissionless alternative to `transfer_admin`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the current admin's own signature. |
| `set_fee_rate(new_bps)` | Sets the protocol fee (basis points). Capped at `MAX_FEE_RATE_BPS` (1000 = 10%). |
| `set_fee_rate_via_multisig(multisig_contract, action_id, new_bps)` | Permissionless alternative to `set_fee_rate`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. Still enforces `MAX_FEE_RATE_BPS` — approval changes who can call it, not what rate is valid. |
| `pause()` / `unpause()` | Emergency stop. Blocks `create_series`, `update_premium`, `buy_option`, `write_option`. Does **not** block `exercise`, `set_settlement_price`, or `reclaim_collateral` — a pause winds existing positions down, it doesn't trap funds. |
| `pause_via_multisig(multisig_contract, action_id)` / `unpause_via_multisig(...)` | Permissionless alternative to `pause`/`unpause`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. No `require_auth()` — the M-of-N approval itself is what authorizes the call. |
| `upgrade(new_wasm_hash)` | Swaps the contract's executable via Soroban's deployer, keeping the same address, ID, and storage. |
| `upgrade_via_multisig(multisig_contract, action_id, new_wasm_hash)` | Permissionless alternative to `upgrade`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. Arguably the highest-value place for this pattern in the whole codebase — a contract's executable is the single most consequential thing about it. |
| `create_series(underlying, option_type, strike_price, expiry, premium, implied_vol)` | Lists a new series. `expiry` must be > 1 hour out. Capped at `MAX_SERIES_PER_UNDERLYING` (50) series ever listed per underlying symbol. |
| `create_series_via_multisig(multisig_contract, action_id, underlying, option_type, strike_price, expiry, premium, implied_vol)` | Permissionless alternative to `create_series`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. Same validation and per-underlying cap apply. |
| `update_premium(series_id, new_premium, new_implied_vol)` | Re-prices an Active series. |
| `update_premium_via_multisig(multisig_contract, action_id, series_id, new_premium, new_implied_vol)` | Permissionless alternative to `update_premium`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. |
| `cancel_series(series_id)` | Cancels an Active series. Position holders then call `claim_refund` individually — the admin doesn't push funds to everyone in one call, since that would scale badly against Soroban's per-call resource limits. |
| `cancel_series_via_multisig(multisig_contract, action_id, series_id)` | Permissionless alternative to `cancel_series`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. Cancelling disrupts every open position in a series, so gating it behind M-of-N is at least as warranted as pause. |

### Oracle

| Function | Description |
|---|---|
| `set_settlement_price(series_id, price)` | The trusted-oracle-address flow: whatever address was set as `oracle` at `initialize` asserts a price directly. Records it and flips the series to `Settled`. |
| `set_settlement_price_from_oracle(series_id, oracle_contract)` | The permissionless alternative: anyone can settle an expired series by pointing at a live `price_oracle` deployment and letting it supply the price via a cross-contract call. No signature required — the price is already backed by that contract's own feeder-authenticated aggregate. |

### Traders

| Function | Description |
|---|---|
| `buy_option(buyer, series_id, contracts, max_premium)` | Opens a Long position. `max_premium` is slippage protection. Premium (net of the protocol fee) funds the pool `write_option` pays writers from. |
| `write_option(writer, series_id, contracts, collateral_amount)` | Opens a Short position. Collateral: notional value for calls, 110% of strike for puts. Premium is paid out of the pool buyers have funded — `InsufficientPremiumPool` if no buyer has paid in enough yet (a write can't be paid a "premium" out of its own just-deposited collateral). |
| `exercise(owner, position_id)` | Long-side payout after expiry, within the 24h settlement window, if in the money. |
| `exercise_batch(owner, position_ids)` | Same as `exercise`, for every id in `position_ids` in one call — for an owner with several long positions who'd otherwise need one transaction per position. All-or-nothing (any single id failing `exercise`'s own checks aborts the whole batch) and capped at `MAX_BATCH_SIZE` (25). Returns the summed payout. |
| `reclaim_collateral(writer, position_id)` | Short-side payout after settlement: locked collateral minus the max loss paid out to longs. |
| `reclaim_batch(writer, position_ids)` | Batched `reclaim_collateral`, same all-or-nothing/`MAX_BATCH_SIZE` contract as `exercise_batch`. Returns the summed reclaim. |
| `claim_refund(owner, position_id)` | On a Cancelled series: buyers get their premium back (net of the fee already sent to `fee_recipient`), writers get their full collateral back. Paid directly from options_market's own balance. |
| `claim_refund_from_vault(vault_contract, owner, position_id)` | Same eligibility checks and refund formula as `claim_refund`, but pays out of a deployed `vault`'s `series_id`-tagged escrow instead — see "Vault integration" below. Requires `escrow_series_to_vault` to have moved this series' liability into `vault` first. |

### Vault integration

| Function | Description |
|---|---|
| `escrow_series_to_vault(vault_contract, series_id)` | For a Cancelled series: moves its entire remaining refund liability — tracked incrementally in `SeriesEscrow` since the series' first `buy_option`/`write_option`, debited by whichever claim path (`claim_refund` or `claim_refund_from_vault`) any given position actually uses — out of options_market's own shared balance and into `vault`, tagged by `series_id`. Permissionless (only relocates the contract's own funds into a vault it already trusts, authorizes nothing new); callable at most once per series, since it zeroes `SeriesEscrow` on success. See "Known gaps" for exactly what this does and doesn't fix. |

### Views

`get_admin`, `is_paused`, `get_fee_rate`, `get_premium_pool`,
`get_series_count_for_underlying`, `get_series`, `get_position`,
`get_user_positions`, `get_underlying_price`, `get_series_escrow`
(remaining not-yet-claimed refund liability for a series), `get_stats`
(total premiums collected, total open interest, series count).

### Errors

| # | Error | | # | Error |
|---|---|---|---|---|
| 1 | `AlreadyInitialized` | | 13 | `PriceNotSet` |
| 2 | `Unauthorized` | | 14 | `NotInTheMoney` |
| 3 | `SeriesNotFound` | | 15 | `WrongSide` |
| 4 | `SeriesNotActive` | | 16 | `ExpiryTooSoon` |
| 5 | `SeriesNotExpired` | | 17 | `ContractPaused` |
| 6 | `PositionNotFound` | | 18 | `SeriesNotCancelled` |
| 7 | `InsufficientPremium` | | 19 | `InvalidFeeRate` |
| 8 | `InsufficientCollateral` | | 20 | `TooManySeriesForUnderlying` |
| 9 | `AlreadyExercised` | | 21 | `InsufficientPremiumPool` |
| 10 | `AlreadySettled` | | 22 | `InvalidSeriesParams` |
| 11 | `ExerciseWindowClosed` | | 23 | `InvalidBatchSize` |
| 12 | `ZeroContracts` | | 24 | `NothingToEscrow` |

## `price_oracle` reference

### Admin

| Function | Description |
|---|---|
| `initialize(admin)` | One-time setup. Defaults `max_staleness` to 1 hour and `min_reports` to 1. |
| `transfer_admin(new_admin)` | Hands off control. Requires the **current** admin's signature. |
| `transfer_admin_via_multisig(multisig_contract, action_id, new_admin)` | Permissionless alternative to `transfer_admin`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the current admin's own signature. |
| `add_feeder(feeder)` / `remove_feeder(feeder)` | Authorize/revoke a price reporter. Capped at `MAX_FEEDERS` (16). A removed feeder's past reports stay readable via `get_latest_report` (audit trail) but no longer count toward the aggregate. |
| `add_feeder_via_multisig(multisig_contract, action_id, feeder)` / `remove_feeder_via_multisig(...)` | Permissionless alternatives: cross-call a deployed `multisig` and check `is_approved(action_id)` instead of requiring the admin's own signature. Still enforce `FeederAlreadyAdded`/`TooManyFeeders`/`FeederNotFound` — approval changes who can call these, not the underlying invariants. |
| `set_max_staleness(seconds)` | How old a report can be and still count toward `get_price`. Rejects zero. |
| `set_max_staleness_via_multisig(multisig_contract, action_id, seconds)` | Permissionless alternative: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. Still rejects zero — approval changes who can call it, not what a valid staleness bound is. |
| `set_min_reports(count)` | How many CURRENTLY-fresh feeder reports `get_price` requires before it returns an aggregate at all. Without this, a single fresh report is enough the moment every other feeder's report goes stale or gets removed — that one feeder then fully determines the price with no averaging effect. Rejects zero. |
| `set_min_reports_via_multisig(multisig_contract, action_id, count)` | Permissionless alternative: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. Still rejects zero. |
| `pause()` / `unpause()` | Emergency stop. Blocks `add_feeder`, `remove_feeder`, `report_price`. Does **not** block `get_price` — a pause freezes changes to the feed, it doesn't hide the last-known price. |
| `pause_via_multisig(multisig_contract, action_id)` / `unpause_via_multisig(...)` | Permissionless alternative to `pause`/`unpause`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. No `require_auth()` — the M-of-N approval itself is what authorizes the call. |

### Feeders

| Function | Description |
|---|---|
| `report_price(feeder, symbol, price)` | Records this feeder's own observation. Requires the feeder's signature and current authorization; rejects non-positive prices. |

### Views

| Function | Description |
|---|---|
| `get_price(symbol)` | Median across every currently-authorized feeder's report that's within `max_staleness` — but only if at least `min_reports` of them are fresh; otherwise `None` even if some reports exist. `None` if nothing is fresh — callers must not treat that as "price is zero." |
| `get_latest_report(symbol, feeder)` | One feeder's raw report, regardless of freshness or current authorization. |
| `is_feeder`, `get_feeder_count`, `get_max_staleness`, `get_min_reports`, `get_admin`, `is_paused` | |

### Events

`admin_transferred`, `paused`, `unpaused`, `max_staleness_updated`,
`min_reports_updated`, `feeder_added`, `feeder_removed`,
`price_reported` — one per state change, so an off-chain indexer doesn't
have to poll every view function to track what changed.

### Errors

| # | Error |
|---|---|
| 1 | `AlreadyInitialized` |
| 2 | `FeederAlreadyAdded` |
| 3 | `FeederNotFound` |
| 4 | `NotAFeeder` |
| 5 | `InvalidPrice` |
| 6 | `ContractPaused` |
| 7 | `InvalidStaleness` |
| 8 | `TooManyFeeders` |
| 9 | `Unauthorized` |
| 10 | `InvalidMinReports` |

## `vault` reference

A per-tag escrow ledger for a single token, set at `initialize`.

### Admin

| Function | Description |
|---|---|
| `initialize(admin, token)` | One-time setup. |
| `transfer_admin(new_admin)` | Hands off control. Requires the **current** admin's signature. |
| `transfer_admin_via_multisig(multisig_contract, action_id, new_admin)` | Permissionless alternative to `transfer_admin`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the current admin's own signature. |
| `pause()` / `unpause()` | Emergency stop. Blocks **both** `deposit` and `withdraw` — unlike options_market's pause (which leaves settlement paths open), there's no "existing position needs an exit" concern independent of the vault itself. |
| `pause_via_multisig(multisig_contract, action_id)` / `unpause_via_multisig(...)` | Permissionless alternative to `pause`/`unpause`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. No `require_auth()` — the M-of-N approval itself is what authorizes the call. |
| `withdraw(tag, to, amount)` | Pays `amount` of `tag`'s escrowed balance to `to`. Panics with `InsufficientEscrowBalance` if `tag` doesn't have that much earmarked, regardless of the vault's total token balance. Admin-gated — in the intended integration, `admin` is set to a calling contract's own address, so a contract-to-contract call satisfies the auth check through the call itself. |
| `withdraw_via_multisig(multisig_contract, action_id, tag, to, amount)` | Permissionless alternative to `withdraw`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. Meant for manual recovery/migration when the calling contract itself can't produce that signature. Still enforces `InsufficientEscrowBalance`. |
| `sweep_untagged(to)` | Recovers tokens that landed on the vault directly, bypassing `deposit` (e.g. a stray transfer). Computes the actual token balance minus `get_total_escrowed`'s ledger sum and transfers exactly that difference; panics with `NoUntaggedFunds` if there's nothing to recover. |
| `sweep_untagged_via_multisig(multisig_contract, action_id, to)` | Permissionless alternative to `sweep_untagged`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. |
| `transfer_tag(from_tag, to_tag, amount)` | Reassigns escrow between tags with no token movement at all — meant for the roll_position case (close + reopen in one breath, collateral doesn't need to leave and come back). `TotalEscrowed` is unaffected. |
| `transfer_tag_via_multisig(multisig_contract, action_id, from_tag, to_tag, amount)` | Permissionless alternative to `transfer_tag`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. |

### Depositors

| Function | Description |
|---|---|
| `deposit(from, tag, amount)` | Pulls `amount` from `from` and credits `tag`'s ledger. Requires `from`'s own signature. |

### Views

`balance_of(tag)`, `get_total_escrowed`, `get_admin`, `get_token`, `is_paused`.

### Events

`admin_transferred`, `paused`, `unpaused`, `deposited`, `withdrawn`,
`swept_untagged`, `tag_transferred`.

### Errors

| # | Error |
|---|---|
| 1 | `AlreadyInitialized` |
| 2 | `InvalidAmount` |
| 3 | `InsufficientEscrowBalance` |
| 4 | `ContractPaused` |
| 5 | `NoUntaggedFunds` |
| 6 | `Unauthorized` |

## `multisig` reference

Signers, threshold, and `approval_ttl` are all fixed at `initialize` and
immutable — there's deliberately no in-protocol way to change the signer
set, so a compromised signer can never add another compromised signer.

| Function | Description |
|---|---|
| `initialize(signers, threshold, approval_ttl)` | One-time setup. Rejects a zero threshold, a threshold above the signer count, or a duplicate signer. `approval_ttl` is in seconds; zero means approvals never expire (the original behavior). |
| `approve(signer, action_id)` | Records `signer`'s approval, stamped with the current ledger timestamp. Requires the signer's own signature and current signer-set membership. Rejects a signer voting twice on the same `action_id` while their existing approval is still fresh — an EXPIRED approval is treated as no approval at all, so re-approving after expiry just refreshes the timestamp instead of erroring. |
| `revoke(signer, action_id)` | Withdraws `signer`'s own still-fresh vote. Rejects revoking an approval that's already expired — there's nothing left to withdraw. |
| `reset(action_id)` | Clears every signer's approval of `action_id`, so a repeat action reusing the same id starts from a clean slate. Only callable once `action_id` is already approved. |

### Views

`is_signer`, `get_signer_count`, `get_threshold`, `get_approval_ttl`, `has_approved(action_id, signer)` (false if expired), `get_approval_count(action_id)` (counts only currently-unexpired approvals), `is_approved(action_id)`.

### Events

`approved`, `revoked`, `reset`.

### Errors

| # | Error |
|---|---|
| 1 | `AlreadyInitialized` |
| 2 | `InvalidThreshold` |
| 3 | `DuplicateSigner` |
| 4 | `NotASigner` |
| 5 | `AlreadyApproved` |
| 6 | `NotYetApproved` |

## Known gaps

- **Cross-position accounting on cancellation is now opt-in via `vault`,
  not the default.** `claim_refund` still pays every position directly
  out of options_market's own undifferentiated balance, unchanged — a
  writer's collateral refund and a buyer's premium refund on the same
  cancelled series still nominally draw from that same shared pot.
  `escrow_series_to_vault` / `claim_refund_from_vault` are the fix, but
  they're additive alternatives a caller has to choose to use per
  series, not a replacement for `claim_refund`: the original 100+
  existing tests around `claim_refund` needed zero changes, and nothing
  requires a cancelled series to ever call `escrow_series_to_vault` at
  all. The integration tags escrow by `series_id` (not per-position) and
  tracks each series' outstanding liability incrementally in
  `SeriesEscrow`, debited by whichever claim path (`claim_refund` or
  `claim_refund_from_vault`) a given position actually uses — so calling
  `escrow_series_to_vault` after some positions already claimed the
  original way still only moves what's genuinely left, never double.
  It does NOT fix the deeper, separate issue below it (pooled premium):
  if `write_option` has already paid a buyer's premium contribution out
  to some writer as that writer's own premium before the series gets
  cancelled, `SeriesEscrow` still promises that buyer a full refund
  (matching `claim_refund`'s own existing behavior) even though part of
  what they contributed has already left the contract for good —
  `escrow_series_to_vault` would then try to move more into `vault` than
  the series actually has left, and fail on the token transfer rather
  than silently under- or over-paying anyone. Full routing of active
  trading (`buy_option`/`write_option`/`exercise`/`reclaim_collateral`)
  through `vault` — the only way to close that deeper gap — remains the
  real, invasive rewrite described below; this integration deliberately
  stops short of it.
- **No order matching.** This is a pooled market, not an order book —
  writers and buyers don't get matched 1:1, and the premium pool
  (`PremiumPool`) is shared across EVERY series in the contract, not
  scoped to one — so `write_option`'s `InsufficientPremiumPool` check is
  a contract-wide pool-level constraint, not a guarantee that any
  specific buyer's trade funded any specific writer's premium, or even
  that the writer and buyer are in the same series at all. That's a
  design choice, not a bug, but it's also the root cause of the
  cancellation edge case called out above: premium a buyer contributed
  can already be gone (paid to some writer, possibly in a different
  series) by the time their own series gets cancelled.
- **`set_settlement_price_from_oracle` is additive, not a replacement.**
  The original trusted-oracle-address flow (`set_settlement_price`)
  still exists unchanged — a series can be settled either way, and
  nothing stops mixing both across different series. That's intentional
  (zero risk to the original flow's existing test coverage) but means
  there's no enforcement that a series *must* use the cross-contract
  path just because a `price_oracle` deployment exists.
- **Every admin-gated function on options_market, price_oracle, and
  vault now has a `_via_multisig` alternative — except `initialize`.**
  options_market: `pause`, `unpause`, `transfer_admin`, `set_fee_rate`,
  `upgrade`, `cancel_series`, `create_series`, `update_premium`.
  price_oracle: `pause`, `unpause`, `transfer_admin`,
  `set_max_staleness`, `set_min_reports`, `add_feeder`, `remove_feeder`.
  vault: `pause`, `unpause`, `transfer_admin`, `withdraw`, `transfer_tag`,
  `sweep_untagged`. `initialize` can't have one by construction — there
  is no admin, and therefore no Multisig deployment trusted by this
  contract, until it runs. Every `_via_multisig` function is additive
  (the original admin-gated version is unchanged) and checks
  `is_approved(action_id)` on a deployed Multisig instead of a single
  signature, with the same validation the original enforces — approval
  only changes who can call a function, never what a valid call to it
  looks like. Callers still choose their own stable `action_id` scheme
  per function, since Multisig never interprets what an id means.
  `exercise_batch`/`reclaim_batch` (owner/writer-gated, same as the
  functions they batch) and `escrow_series_to_vault` (permissionless)
  fall outside this pattern entirely — none of them are admin-gated to
  begin with, so there's no single-signature check for a `_via_multisig`
  variant to replace.
