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
- [`vault/`](vault) — a per-(token, tag) escrow ledger for any number of
  allowlisted tokens and integrator contracts,
  motivated by a gap discovered while testing options_market: that
  contract holds every writer's collateral and every buyer's premium in
  one undifferentiated balance, with no accounting of which balance is
  actually earmarked for which position. `deposit`/`withdraw` here are
  scoped to a namespaced `tag` (e.g. `(options_market, "series", id)`), so a withdrawal
  can never draw down more than was specifically deposited under that
  tag — regardless of what the vault's raw token balance happens to be
  from other tags' deposits. `options_market::escrow_series_to_vault` /
  `claim_refund_from_vault` now wire this in for the specific gap it was
  built for — see "Known gaps" below for the scope of that integration.
- [`timelock/`](timelock) — a delayed-execution admin (proposer /
  executor / canceller roles, predecessor dependencies) meant to hold the
  admin role on the other contracts so users always get an exit window.
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
- [`params/`](params) — a timelock-controlled registry of protocol
  parameters, each with hard `[min, max]` bounds. `options_market`
  reads its fee rate and settlement window from it (cached locally,
  refreshed only when the registry version moves). See
  "`params` reference" below for the parameter catalog.
- [`grants_escrow/`](grants_escrow) — milestone-based escrow for
  community grants and bounties: funded upfront, released per milestone
  on reviewer quorum, reclaimable by the funder after a missed deadline
  plus grace period. Operations guide:
  [`docs/grants_escrow.md`](docs/grants_escrow.md).

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

cd ../params
cargo build --target wasm32-unknown-unknown --release   # options_market needs it too (no dependencies of its own)

cd ../options_market   # now this crate can build/test/etc.
cargo build                                   # native build, fast iteration
cargo test                                    # unit tests (soroban-sdk testutils)
cargo clippy --all-targets -- -D warnings     # matches CI
cargo fmt --check                             # matches CI
cargo build --target wasm32-unknown-unknown --release   # the real deploy artifact
```

`params` and `grants_escrow` have no wasm dependencies of their own.
`vault` only needs multisig's wasm built first, no other dependency of
its own (options_market depending on vault's wasm doesn't run the other
way). `multisig` itself has no dependency on anything else and can be
built/tested independently, in any order relative to the others.

CI (`.github/workflows/ci.yml`) builds the required dependency wasm(s)
first whenever a job is about to touch options_market, price_oracle,
or vault, then runs the same four checks against every push and PR,
for every crate.

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

## Storage TTL policy

Every contract extends its **instance** storage at the top of every
entrypoint, and extends a **persistent** entry whenever it reads or
writes it (`ttl.rs` in each crate). `extend_ttl` only charges rent once
an entry's remaining TTL drops below the threshold, so the steady-state
cost of a read is one TTL check. No contract uses temporary storage.
Ledgers are ~5s, so one day ≈ 17,280 ledgers.

| Class | Keys | Threshold | Extend to | Who pays |
|---|---|---|---|---|
| Instance | options_market: `Admin`, `Oracle`, `CollateralToken`, `FeeRecipient`, counters, `TotalPremiumsCollected`, `TotalOpenInterest`, `Paused`, `FeeRateBps`, `SeriesCountForUnderlying`, `PremiumPool`, `ParamsRegistry`, `ParamsVersion`, `SettlementWindow` · price_oracle: `Admin`, `Paused`, `MaxStaleness`, `MinReports`, `Feeders` · vault: `Admin`, `Token`, `Paused`, `TotalEscrowed` · multisig: `Signers`, `Threshold`, `ApprovalTtl` · params: `Timelock`, `Version` · grants_escrow: `GrantCounter` | 23 days | 30 days | Whoever invokes any entrypoint |
| Persistent | options_market: `Series`, `Position`, `UserPositions`, `UnderlyingPrice`, `SeriesEscrow` · price_oracle: `PriceReport`, `AggregatedPrice` · vault: `Escrow` · multisig: `Approval` · params: `Param`, `PendingBounds` · grants_escrow: `Grant`, `Approval`, `GranteeGrants` | 60 days | 90 days | Whoever reads/writes the entry; keepers via `bump` |

Every contract also exposes a permissionless `bump(keys: Vec<DataKey>)`
that extends the instance plus each named persistent entry that exists,
so a keeper can keep long-lived but idle entries (a long-dated series,
a dormant position, an idle vault tag) from archiving. Entries that do
archive anyway can be restored — see the
[archival runbook](docs/runbooks/archival.md) and `scripts/restore/`.

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
| `create_series(underlying, option_type, strike_price, expiry, premium, implied_vol)` | Lists a new series. `expiry` must be > 1 hour out; `premium` must be > 0 (`InvalidSeriesParams`). Capped at `MAX_SERIES_PER_UNDERLYING` (50) series ever listed per underlying symbol. Each `(underlying, option_type, strike_price, expiry)` spec can be listed once, ever (a cancelled or settled series still owns it): a second listing fails with `DuplicateSeries`. |
| `create_series_via_multisig(multisig_contract, action_id, underlying, option_type, strike_price, expiry, premium, implied_vol)` | Permissionless alternative to `create_series`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. Same validation and per-underlying cap apply. |
| `update_premium(series_id, new_premium, new_implied_vol)` | Re-prices an Active series. `new_premium` must be > 0 (`InvalidSeriesParams`). |
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
| `write_option(writer, series_id, contracts, collateral_amount, min_premium)` | Opens a Short position. Collateral: notional value for calls, 110% of strike for puts. Premium is paid out of the pool buyers have funded — `InsufficientPremiumPool` if no buyer has paid in enough yet (a write can't be paid a "premium" out of its own just-deposited collateral). `min_premium` is slippage protection: `PremiumBelowMinimum` if the net premium the writer would receive (after the protocol fee) is below it, so an `update_premium` cut or fee increase ordered ahead of the write can't underpay the writer. Pass `0` to opt out. |
| `exercise(owner, position_id)` | Long-side payout after expiry, within the 24h settlement window, if in the money. The 24h settlement window is a soft deadline (longs remain claimable until the 90-day forfeiture window). |
| `exercise_batch(owner, position_ids)` | Same as `exercise`, for every id in `position_ids` in one call — for an owner with several long positions who'd otherwise need one transaction per position. All-or-nothing (any single id failing `exercise`'s own checks aborts the whole batch) and capped at `MAX_BATCH_SIZE` (25). Returns the summed payout. |
| `settle_long(position_id)` | Permissionless auto-exercise / keeper settlement for in-the-money longs after settlement. Anyone can call; payout is unconditionally transferred to the position owner. |
| `sweep_forfeited(position_id)` | Admin entrypoint to sweep unexercised ITM long payouts after the 90-day forfeiture deadline to the protocol treasury/fee recipient with a `payout_forfeited` event. |
| `reclaim_collateral(writer, position_id)` | Short-side payout after settlement: locked collateral minus the max loss paid out to longs. |
| `reclaim_batch(writer, position_ids)` | Batched `reclaim_collateral`, same all-or-nothing/`MAX_BATCH_SIZE` contract as `exercise_batch`. Returns the summed reclaim. |
| `transfer_position(from, to, position_id)` | Moves an open position to `to` (`from` must own it and sign); both parties' position lists are updated and `to` inherits exercise / reclaim / refund rights. Shorts are transferable because their collateral is already locked here, so `to` only gains the leftover-collateral reclaim. Rejected for exercised/settled positions and while paused; `to == from` is a no-op. Emits `position_transferred(from, to; position_id)`. |
| `split_position(owner, position_id, split_contracts)` | Splits `split_contracts` (`0 < split < contracts`, else `InvalidSplitAmount`) off an open position into a new position with the same series, side and owner. `premium_paid`, `fee_paid` and `collateral_locked` are divided pro rata, rounded down for the new position with the remainder kept in the original, so every total (and `SeriesEscrow`, open interest) is unchanged. Blocked while paused and for exercised/settled positions. Returns the new position id. |
| `claim_refund(owner, position_id)` | On a Cancelled series: buyers get their premium back (net of the fee already sent to `fee_recipient`), writers get their full collateral back. Paid directly from options_market's own balance. |
| `claim_refund_from_vault(vault_contract, owner, position_id)` | Same eligibility checks and refund formula as `claim_refund`, but pays out of a deployed `vault`'s `series_id`-tagged escrow instead — see "Vault integration" below. Requires `escrow_series_to_vault` to have moved this series' liability into `vault` first. |

### Vault integration

| Function | Description |
|---|---|
| `escrow_series_to_vault(vault_contract, series_id)` | For a Cancelled series: moves its entire remaining refund liability — tracked incrementally in `SeriesEscrow` since the series' first `buy_option`/`write_option`, debited by whichever claim path (`claim_refund` or `claim_refund_from_vault`) any given position actually uses — out of options_market's own shared balance and into `vault`, tagged by `series_id`. Permissionless (only relocates the contract's own funds into a vault it already trusts, authorizes nothing new); callable at most once per series, since it zeroes `SeriesEscrow` on success. See "Known gaps" for exactly what this does and doesn't fix. |

### Views

`get_admin`, `is_paused`, `get_fee_rate`, `get_premium_pool`,
`get_series_count_for_underlying`, `get_series_id`, `get_series`, `get_position`,
`get_user_positions`, `get_underlying_price`, `get_series_escrow`
(remaining not-yet-claimed refund liability for a series), `get_stats`
(total premiums collected, total open interest, series count),
`get_orphaned_liabilities`, `sync_orphaned_liabilities`.

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
| | | | 25 | `NotEligibleForForfeiture` |
| | | | 26 | `DuplicateSeries` |
| | | | 27 | `InvalidSplitAmount` |

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

A per-(token, tag) escrow ledger. One deployment holds any number of
allowlisted tokens and serves any number of integrator contracts
(`options_market`, an LP pool, an insurance fund, ...). The token passed
to `initialize` is the **default token** used by the `_legacy` wrappers.

### Tag scheme

A tag is a `#[contracttype] struct Tag { owner: Address, kind: Symbol, id: u64 }`,
and the storage key is the full triple, so two integrators — or two id
spaces inside one integrator — can never collide by construction.
`options_market` uses `(self, "series", series_id)` for series escrow and
reserves `(self, "position", position_id)` for per-position custody.

A plain struct was chosen over a `BytesN<32>` hash of it: the key is only
an address + a short symbol + a u64 larger than a hash, and in exchange
storage keys and events stay self-describing (an indexer doesn't need a
preimage registry to tell what a tag refers to).

Legacy `u64` tags map to `(legacy owner, "legacy", id)`, where the legacy
owner is the admin at `initialize` time (fixed, so a later
`transfer_admin` can't orphan legacy balances).

### Trust model

- **Tag owners** (integrators) are the only ones who can move escrow.
  A tag's owner is recorded as `TagOwner(tag) = tag.owner` on its first
  deposit, and `withdraw` / `transfer_tag` require that owner's
  `require_auth()`. In the intended integration the owner is a contract,
  so its own cross-contract call satisfies the check without a human
  signature. A bug in one integrator can therefore only ever reach its
  own tags.
- **Creating a tag** requires `tag.owner` to be in the integrator
  registry (or be the legacy owner) and to authorize. End users can
  deposit into a contract-owned tag only with that contract's auth, and
  the contract still owns it. `transfer_tag` between different owners
  requires **both** owners to authorize.
- **The admin** configures the vault (token allowlist, integrator
  registry, pause, sweeping untagged funds) but owns no integrator's
  tags. Moving escrow without the owner is an **emergency-only** power,
  available solely through multisig approval (`withdraw_via_multisig`,
  `transfer_tag_via_multisig`), and still capped at each tag's balance.

### Admin

| Function | Description |
|---|---|
| `initialize(admin, token)` | One-time setup. `token` becomes the default token and is allowlisted. |
| `transfer_admin(new_admin)` | Hands off control. Requires the **current** admin's signature. Does not hand over any tag. |
| `transfer_admin_via_multisig(multisig_contract, action_id, new_admin)` | Permissionless alternative to `transfer_admin`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the current admin's own signature. |
| `pause()` / `unpause()` | Emergency stop. Blocks `deposit`, `withdraw` and `transfer_tag` — unlike options_market's pause (which leaves settlement paths open), there's no "existing position needs an exit" concern independent of the vault itself. |
| `pause_via_multisig(multisig_contract, action_id)` / `unpause_via_multisig(...)` | Permissionless alternative to `pause`/`unpause`: cross-calls a deployed `multisig` and checks `is_approved(action_id)` instead of requiring the admin's own signature. No `require_auth()` — the M-of-N approval itself is what authorizes the call. |
| `set_token_allowed(token, allowed)` / `set_token_allowed_via_multisig(multisig_contract, action_id, token, allowed)` | Manages the token allowlist. Removing a token only blocks **new** deposits; existing balances stay withdrawable, transferable and sweepable. |
| `set_integrator(integrator, allowed)` / `set_integrator_via_multisig(multisig_contract, action_id, integrator, allowed)` | Manages the integrator registry (who may create tags). Deregistering blocks new tags only. |
| `withdraw(token, tag, to, amount)` | Pays `amount` of `(token, tag)`'s escrow to `to`. Requires the tag owner. Panics with `InsufficientEscrowBalance` if the tag doesn't have that much earmarked, and `InsufficientVaultBalance` (instead of a token error) if the vault's actual balance can't cover it. Emits `shortfall_detected` if the ledger is under-backed. |
| `withdraw_via_multisig(multisig_contract, action_id, token, tag, to, amount)` | Emergency payout without the tag owner (e.g. the owning contract is being replaced). Still enforces `InsufficientEscrowBalance`. |
| `sweep_untagged(token, to)` / `sweep_untagged_via_multisig(multisig_contract, action_id, token, to)` | Recovers `token` that landed on the vault directly, bypassing `deposit`: the actual balance minus `get_total_escrowed(token)`. Panics with `NoUntaggedFunds` if there's nothing to recover — including when there is a shortfall instead. |
| `transfer_tag(token, from_tag, to_tag, amount)` | Reassigns escrow between tags with no token movement — meant for the roll_position case. Requires `from_tag`'s owner, and `to_tag`'s owner too if different. `TotalEscrowed` is unaffected. |
| `transfer_tag_via_multisig(multisig_contract, action_id, token, from_tag, to_tag, amount)` | Emergency reassignment without the owners. A new `to_tag` is recorded as owned by `to_tag.owner`. |
| `withdraw_batch(ops: Vec<(token, tag, to, amount)>)` | Batched `withdraw`: one admin auth check, up to `MAX_BATCH` (50) entries, every entry validated against in-flight balances (the same tag may repeat) before anything is written, one token transfer per distinct recipient, all-or-nothing. One `withdrawn` event per entry. ~4x cheaper than sequential calls (10 entries: 0.92M vs 3.80M CPU instructions, 175KB vs 844KB memory). |
| `transfer_tag_batch(ops: Vec<(token, from_tag, to_tag, amount)>)` | Batched `transfer_tag`, same validation and all-or-nothing semantics. One `tag_transferred` event per entry. |
| `migrate_legacy(ids)` | Moves pre-upgrade `Escrow(u64)` entries to `Escrow(default token, legacy_tag(id))`, owned by the current admin; the old `TotalEscrowed` moves to the default token's total on the first call. Idempotent. |

### Depositors

| Function | Description |
|---|---|
| `deposit(from, token, tag, amount) -> i128` | Pulls `amount` of an allowlisted `token` from `from` and credits `(token, tag)`. Requires `from`'s signature (and, on a tag's first deposit, its owner's). Credits — and returns — the **measured** balance delta, not `amount`: a fee-on-transfer token credits only what arrived. Costs two extra `balance` reads per deposit. |

### Legacy wrappers (default token, `u64` tags)

`deposit_legacy(from, id, amount)`, `withdraw_legacy(id, to, amount)`,
`transfer_tag_legacy(from_id, to_id, amount)`, `balance_of_legacy(id)`,
`legacy_tag(id)`.

### Views

`balance_of(token, tag)`, `get_total_escrowed(token)`,
`get_shortfall(token)` (`max(0, TotalEscrowed(token) − balance)`),
`get_tag_owner(tag)`, `is_token_allowed(token)`, `is_integrator(addr)`,
`get_admin`, `get_token`, `is_paused`,
`get_tag_count`, `get_tags(cursor, limit)`, `verify_ledger(cursor, limit) -> (partial_sum, next_cursor)`.

`get_tags`/`verify_ledger` page through the ActiveTags index (every tag
with a nonzero balance; `limit` capped at 100). The index uses
swap-remove, so read all pages against one ledger snapshot.

### Ledger reconciliation

The vault's invariant is `sum(Escrow(tag)) == TotalEscrowed <= token.balance(vault)`.
Monitors can check it over RPC:

```sh
count=$(stellar contract invoke --id $VAULT -- get_tag_count)
cursor=0; sum=0
while [ "$cursor" -lt "$count" ]; do
  read partial cursor < <(stellar contract invoke --id $VAULT -- \
    verify_ledger --cursor $cursor --limit 100 | tr -d '[]",' )
  sum=$((sum + partial))
done
total=$(stellar contract invoke --id $VAULT -- get_total_escrowed | tr -d '"')
[ "$sum" = "$total" ] && echo "ledger OK ($sum)" || echo "MISMATCH: $sum != $total"
```

Building with `--features invariants` compiles a full post-condition check
into every mutating entrypoint (tests/fuzzing only, never production wasm);
CI runs the suite that way, including a 10,000-op random fuzz test.

### Clawback risk

A SAC issuer with clawback enabled can reduce the vault's balance
directly, leaving it below `TotalEscrowed`. The vault cannot prevent
this; it makes it visible instead: `get_shortfall(token)` reports the gap,
`withdraw` emits `shortfall_detected` whenever it sees one (at the cost
of one extra `balance` read), payouts the vault can no longer cover fail
with `InsufficientVaultBalance`, and `sweep_untagged` returns
`NoUntaggedFunds` rather than trapping. Shortfall socialization across
tags is out of scope — the last withdrawers of an under-backed token
absorb it.

### Events

`admin_transferred`, `paused`, `unpaused`, `token_allowed`,
`integrator_set`, `legacy_migrated`, and the token-scoped ones:
`deposited` / `withdrawn` (topics `token`, full `tag`; data `(from|to, amount)`),
`tag_transferred` (topic `token`; data `(from_tag, to_tag, amount)`),
`swept_untagged` (topics `token`, `to`), `shortfall_detected` (topic `token`).

### Errors

| # | Error |
|---|---|
| 1 | `AlreadyInitialized` |
| 2 | `InvalidAmount` |
| 3 | `InsufficientEscrowBalance` |
| 4 | `ContractPaused` |
| 5 | `NoUntaggedFunds` |
| 6 | `Unauthorized` |
| 7 | `TokenNotAllowed` |
| 8 | `UnregisteredIntegrator` |
| 9 | `InsufficientVaultBalance` |

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

## `timelock` reference

Meant to hold the admin role on every other Zenith contract, so any
parameter or code change is visible on-chain for at least `min_delay`
seconds before it can run. Modelled on OpenZeppelin's TimelockController.

| Function | Description |
|---|---|
| `initialize(min_delay, proposers, executors, cancellers)` | One-time setup. Empty `executors` means anyone may execute a ready operation. There is no admin afterwards. |
| `schedule(proposer, calls, predecessor, salt, delay) -> id` | Proposer-only. `calls` is a `Vec<Call { target, function, args }>`; `delay >= min_delay`. The id is `sha256(xdr((calls, predecessor, salt)))` (see `hash_operation`). Rejects an id that's already pending or done. |
| `execute(executor, calls, predecessor, salt)` | Executor-only (unless open). Requires the operation ready and `predecessor` (if any) done. Runs every call in order via `invoke_contract`; any revert reverts the whole batch. |
| `cancel(canceller, id)` | Canceller-only. Removes a pending operation. |

**Self-administration.** Soroban forbids re-entry, so a `Call` whose
`target` is the timelock itself is dispatched internally by `execute`:
`update_delay(u64)`, `grant_role(Role, Address)`, `revoke_role(Role, Address)`,
`set_open_executor(bool)`. These changes are therefore only reachable
through a delayed operation.

**Views:** `get_timestamp(id)` (0 unknown, 1 done, else ready-at),
`is_operation`, `is_operation_pending`, `is_operation_ready`,
`is_operation_done`, `get_min_delay`, `has_role(role, account)`,
`is_open_executor`, `hash_operation`.

**Events:** `scheduled`, `executed`, `cancelled`, `min_delay_changed`, `role_changed`.

**Errors:** 1 `AlreadyInitialized`, 2 `Unauthorized`, 3 `InsufficientDelay`,
4 `AlreadyScheduled`, 5 `NotReady`, 6 `PredecessorNotDone`, 7 `NotPending`,
8 `UnknownSelfCall`, 9 `EmptyOperation`.

### Operations guide

1. Deploy the timelock, then `transfer_admin(timelock)` on each contract.
2. A proposer schedules the change, e.g. `calls = [Call { target: options_market, function: "set_fee_rate", args: [25u32] }]`, with a unique `salt`, and announces the id.
3. During the delay users can exit; a canceller can `cancel(id)`.
4. Once `is_operation_ready(id)`, an executor calls `execute` with the exact same `calls`/`predecessor`/`salt`.
5. Use `predecessor` to force ordering (e.g. a migration after an `upgrade`).

## `params` reference

All writes require the `timelock` address set at `initialize`. Values
move freely inside their bounds; bounds themselves only move through a
second, slower path (`BOUNDS_CHANGE_DELAY` = 7 days on top of the
timelock's own delay), so widening the sanity envelope always takes
longer than using it. Every change bumps `get_version()`.

| Function | Description |
|---|---|
| `initialize(timelock)` | One-time setup. |
| `define_param(key, value, min, max)` | Creates a parameter that has never been set. |
| `set_param(key, value)` | Moves the value within `[min, max]`. Emits `param_updated(key, old, new)`. |
| `propose_bounds(key, min, max)` | Queues new bounds (must still contain the current value), executable after `BOUNDS_CHANGE_DELAY`. |
| `execute_bounds(key)` / `cancel_bounds(key)` | Applies or drops the queued bounds change. |

Views: `get_param(key) -> Option<{value, min, max, updated_at}>`,
`get_value(key)`, `get_pending_bounds(key)`, `get_version()`,
`get_timelock()`. Events: `param_defined`, `param_updated`,
`bounds_proposed`, `bounds_updated`, `bounds_cancelled`.

### Parameter catalog

| Key | Consumer | Unit | Suggested bounds | Default when unset |
|---|---|---|---|---|
| `fee_bps` | options_market fee rate | basis points | `[0, 1000]` | local `FeeRateBps` (50) |
| `settle_w` | options_market exercise window after expiry | seconds | `[3600, 604800]` | 86,400 |

options_market points at a registry via admin-only
`set_params_registry(registry)`. It caches both values in instance
storage with the registry version and only re-reads them when the
version moves. An unreachable registry, an unset key, or a value
outside options_market's own hard limits (`MAX_FEE_RATE_BPS`, window > 0)
leaves the cached value in place. Other parameters (oracle staleness,
`min_reports`, OI caps, ...) are follow-ups.

## `grants_escrow` reference

| Function | Description |
|---|---|
| `create_grant(funder, grantee, token, milestones: Vec<(amount, deadline)>, reviewers, quorum) -> u64` | Funds the grant upfront. Rejects a reviewer who is also the grantee, duplicate reviewers, a quorum outside `1..=reviewers`, and non-positive amounts or past deadlines. |
| `submit_milestone(grant_id, idx, evidence_hash)` | Grantee only, before the milestone's deadline. |
| `approve_milestone(grant_id, idx, reviewer)` | One approval per reviewer; the approval that reaches quorum releases the milestone in full (no partial approval). |
| `reclaim(grant_id) -> i128` | Funder only. Returns every unreleased milestone whose deadline plus `GRACE_PERIOD` (7 days) has passed. |

Views: `get_grant`, `get_grant_count`, `has_approved`,
`get_grantee_grant_count(grantee)`, `get_grants_by_grantee(grantee,
start, limit)` (paginated, `limit` capped at 50). Events:
`grant_created`, `milestone_submitted`, `milestone_approved`,
`milestone_released`, `funds_reclaimed`.

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
  vault: `pause`, `unpause`, `transfer_admin`, `set_token_allowed`,
  `set_integrator`, `sweep_untagged`, plus the emergency-only `withdraw`
  and `transfer_tag` (whose normal path is tag-owner-gated, not admin). `initialize` can't have one by construction — there
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
