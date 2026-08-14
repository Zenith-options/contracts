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
  aggregated as a median across whatever's still fresh.
  `options_market::set_settlement_price_from_oracle` cross-calls a live
  price_oracle deployment to settle a series permissionlessly, as an
  alternative to the original `set_settlement_price`'s trusted-oracle-
  address flow.

## Building and testing

Each crate is standalone (no workspace `Cargo.toml`), but **build order
matters**: options_market cross-calls price_oracle via soroban-sdk's
`contractimport!` against price_oracle's *compiled wasm* (not a normal
source dependency — that would link price_oracle's own contract
functions into options_market's wasm and collide with options_market's
functions of the same name, like `pause`/`transfer_admin`). That means
price_oracle's wasm has to exist before options_market can be compiled
at all, even natively:

```sh
cd price_oracle
cargo build --target wasm32-unknown-unknown --release   # do this FIRST

cd ../options_market   # now this crate can build/test/etc.
cargo build                                   # native build, fast iteration
cargo test                                    # unit tests (soroban-sdk testutils)
cargo clippy --all-targets -- -D warnings     # matches CI
cargo fmt --check                             # matches CI
cargo build --target wasm32-unknown-unknown --release   # the real deploy artifact
```

price_oracle itself has no such dependency and can be built/tested with
the same five commands independently, in any order.

CI (`.github/workflows/ci.yml`) builds price_oracle's wasm first
whenever a job is about to touch options_market, then runs the same
four checks against every push and PR, for both crates.

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
| `set_fee_rate(new_bps)` | Sets the protocol fee (basis points). Capped at `MAX_FEE_RATE_BPS` (1000 = 10%). |
| `pause()` / `unpause()` | Emergency stop. Blocks `create_series`, `update_premium`, `buy_option`, `write_option`. Does **not** block `exercise`, `set_settlement_price`, or `reclaim_collateral` — a pause winds existing positions down, it doesn't trap funds. |
| `upgrade(new_wasm_hash)` | Swaps the contract's executable via Soroban's deployer, keeping the same address, ID, and storage. |
| `create_series(underlying, option_type, strike_price, expiry, premium, implied_vol)` | Lists a new series. `expiry` must be > 1 hour out. Capped at `MAX_SERIES_PER_UNDERLYING` (50) series ever listed per underlying symbol. |
| `update_premium(series_id, new_premium, new_implied_vol)` | Re-prices an Active series. |
| `cancel_series(series_id)` | Cancels an Active series. Position holders then call `claim_refund` individually — the admin doesn't push funds to everyone in one call, since that would scale badly against Soroban's per-call resource limits. |

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
| `reclaim_collateral(writer, position_id)` | Short-side payout after settlement: locked collateral minus the max loss paid out to longs. |
| `claim_refund(owner, position_id)` | On a Cancelled series: buyers get their premium back (net of the fee already sent to `fee_recipient`), writers get their full collateral back. |

### Views

`get_admin`, `is_paused`, `get_fee_rate`, `get_premium_pool`,
`get_series_count_for_underlying`, `get_series`, `get_position`,
`get_user_positions`, `get_underlying_price`, `get_stats` (total premiums
collected, total open interest, series count).

### Errors

| # | Error | | # | Error |
|---|---|---|---|---|
| 1 | `AlreadyInitialized` | | 12 | `ZeroContracts` |
| 2 | `Unauthorized` | | 13 | `PriceNotSet` |
| 3 | `SeriesNotFound` | | 14 | `NotInTheMoney` |
| 4 | `SeriesNotActive` | | 15 | `WrongSide` |
| 5 | `SeriesNotExpired` | | 16 | `ExpiryTooSoon` |
| 6 | `PositionNotFound` | | 17 | `ContractPaused` |
| 7 | `InsufficientPremium` | | 18 | `SeriesNotCancelled` |
| 8 | `InsufficientCollateral` | | 19 | `InvalidFeeRate` |
| 9 | `AlreadyExercised` | | 20 | `TooManySeriesForUnderlying` |
| 10 | `AlreadySettled` | | 21 | `InsufficientPremiumPool` |
| 11 | `ExerciseWindowClosed` | | 22 | `InvalidSeriesParams` |

## `price_oracle` reference

### Admin

| Function | Description |
|---|---|
| `initialize(admin)` | One-time setup. Defaults `max_staleness` to 1 hour. |
| `transfer_admin(new_admin)` | Hands off control. Requires the **current** admin's signature. |
| `add_feeder(feeder)` / `remove_feeder(feeder)` | Authorize/revoke a price reporter. Capped at `MAX_FEEDERS` (16). A removed feeder's past reports stay readable via `get_latest_report` (audit trail) but no longer count toward the aggregate. |
| `set_max_staleness(seconds)` | How old a report can be and still count toward `get_price`. Rejects zero. |
| `pause()` / `unpause()` | Emergency stop. Blocks `add_feeder`, `remove_feeder`, `report_price`. Does **not** block `get_price` — a pause freezes changes to the feed, it doesn't hide the last-known price. |

### Feeders

| Function | Description |
|---|---|
| `report_price(feeder, symbol, price)` | Records this feeder's own observation. Requires the feeder's signature and current authorization; rejects non-positive prices. |

### Views

| Function | Description |
|---|---|
| `get_price(symbol)` | Median across every currently-authorized feeder's report that's within `max_staleness`. `None` if nothing is fresh — callers must not treat that as "price is zero." |
| `get_latest_report(symbol, feeder)` | One feeder's raw report, regardless of freshness or current authorization. |
| `is_feeder`, `get_feeder_count`, `get_max_staleness`, `get_admin`, `is_paused` | |

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

## Known gaps

- **Cross-position accounting on cancellation.** `claim_refund` on a
  Cancelled series is correct for any *single* claim, but a writer's
  collateral refund and a buyer's premium refund draw from the same
  undifferentiated vault balance — claiming both after the same
  cancellation can still collectively overdraw in edge cases, since there's
  no per-position ledger of which balance is earmarked for what. The
  `PremiumPool` fix resolved the analogous gap for `write_option` itself;
  a full per-position vault ledger would be the equivalent fix here.
- **No order matching.** This is a pooled market, not an order book —
  writers and buyers don't get matched 1:1, they share a common vault and
  premium pool per series. That's a design choice, not a bug, but it means
  `write_option`'s `InsufficientPremiumPool` check is a pool-level
  constraint, not a guarantee that any specific buyer's trade funded any
  specific writer's premium.
- **`set_settlement_price_from_oracle` is additive, not a replacement.**
  The original trusted-oracle-address flow (`set_settlement_price`)
  still exists unchanged — a series can be settled either way, and
  nothing stops mixing both across different series. That's intentional
  (zero risk to the original flow's existing test coverage) but means
  there's no enforcement that a series *must* use the cross-contract
  path just because a `price_oracle` deployment exists.
