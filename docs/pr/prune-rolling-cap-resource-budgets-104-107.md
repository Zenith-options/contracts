# options_market: pruning, rolling active-series cap, resource and wasm budgets (#104 #105 #106 #107)

## Base restore (read first)

On `main`, `options_market/src/{lib,storage,types,error,test}.rs` are empty
files. They were emptied by #149 (`f368522`) and #150. This branch
rebuilds them before adding anything:

- `lib.rs`, `storage.rs`, `error.rs`, `test.rs` come from `709ab7a`.
- `types.rs` comes from `f5c5a4d`, plus the keys `lib.rs` already used
  (`ParamsRegistry`, `ParamsVersion`, `SettlementWindow`, `SeriesIndex`).
- `transfer_position` was truncated mid-line in every commit that has it.
  It was rewritten from its doc comment and the README. `split_position`
  was lost after `1d65c61` and comes back from there.
- `claim_series_index` was called but never committed, so it was
  written from the #63 PR notes. `add_orphaned_liability` and
  `deduct_orphaned_liability` come from `f5c5a4d`.

The batch-trading and paginated views from #149 (`buy_batch`,
`write_batch`, `get_series_page`, ...) are **not** restored: their source
was never committed intact.

Still empty on `main` and out of scope here: `vault/src/lib.rs`,
`price_oracle/src/lib.rs`, `multisig/src/lib.rs`, and all of `timelock/`.
Until they are restored, options_market's tests can't build, because
they register real multisig, vault and price_oracle contracts.

## #107 Rolling active-series cap

- `ActiveSeriesCount(underlying)` replaces the lifetime
  `MAX_SERIES_PER_UNDERLYING` check. `create_series` takes a slot. A
  series releases its slot exactly once, guarded by the
  `SeriesReleased(id)` flag (the "counted_inactive" guard):
  - **Settled**: once the exercise window closes, through the
    permissionless `release_series_slot` or `prune_series`. A settled
    series with open claims (unexercised ITM longs) stops counting as
    active once the window closes. Those claims are tracked in
    `OrphanedLiabilities` and still block pruning.
  - **Cancelled**: automatically, on the last refund. An empty series
    releases at cancellation.
  - **Pruned**: if it hadn't released already.
  - There is no Voided state in this contract, so Cancelled covers it.
- The cap is configurable: `set_max_active_series(_via_multisig)`
  (1..=500) and the params registry key `max_ser`. The default is 50.
- Migration: `migrate_counters(limit)` is permissionless and bounded to
  `MAX_PAGE_SCAN` (200) reads per call. It rebuilds per-series position
  counts first, then active counts. Cursors make concurrent trading safe:
  ids above a cursor are counted by the scan, ids at or below it by live
  code. `create_series`, pruning and slot releases return
  `MigrationPending` until it finishes. Fresh deployments start migrated.
- Settlement is now one-shot. Re-settling returns `AlreadySettled`, and
  settling a Cancelled series returns `SeriesNotActive`. Both used to be
  allowed, and both would corrupt the terminal-state rules.
- `get_series_count_for_underlying` still returns the lifetime count.

## #104 Pruning

- `prune_positions(ids)` handles up to 25 ids, all-or-nothing.
  `prune_series(id)` requires `SeriesPositions.live == 0`. Both are
  permissionless, remove index references (`UserPositions`,
  `SeriesIndex`, escrow and counters), and emit
  `position_pruned` / `series_pruned` with the full struct.
- Terminal-state and retention rules are in the README ("Retention
  policy"). Retention is 30 days after the later of settlement and the
  end of the exercise window, or after cancellation. Nothing that still
  carries a liability is prunable: unexercised ITM longs,
  unreclaimed shorts, and unrefunded positions of cancelled series.
- `get_position_status` / `get_series_status` return `Live(record)`,
  `Pruned` or `None`. Ids come from counters, so no tombstone entry is
  needed.
- Not done: keeper bounty (the keeper issue hasn't landed).

## #105 Resource budget suite

- `tools/resource-snapshot`: `measure()` records CPU and memory from the
  budget, plus read/write entries and bytes from the host's recording
  footprint (reset before each measured call). `Suite::check` compares
  against `snapshots/resources/<scenario>.json` with a 5% default
  tolerance, configurable per scenario. `UPDATE_SNAPSHOTS=1` regenerates
  the snapshots.
- `options_market/src/test_resources.rs` has 17 scenarios, including
  the worst cases `exercise_batch_25`, `reclaim_batch_25`,
  `prune_positions_25` and `migrate_counters_200`.
- With `--features resource-wasm`, the suite registers the built (in CI,
  optimized) wasm. Each snapshot records its mode, and a wasm snapshot is
  never compared against a native run.
- `scripts/resource-diff/resource_diff.py` writes the summary table to
  the job summary, with a CPU delta against the base branch.
- Not done: max feeders (price_oracle) and max signers (multisig)
  scenarios, because those crates' sources are empty on `main`.
  `tools/resource-snapshot` is ready for them.

**Snapshots are not committed yet.** This branch was written without
building or running anything. The first run needs:

```sh
cd options_market && cargo build --target wasm32-unknown-unknown --release
UPDATE_SNAPSHOTS=1 cargo test --features resource-wasm resource_snapshots
```

To show the gate failing, add a redundant `ttl::get_persistent` read to
`exercise_one`. `exercise_batch_25` then fails on `read_bytes`/`cpu_insns`
and the table marks the row ❌.

## #106 Wasm size gate and reductions

- `scripts/wasm-size/check.sh` reports raw and optimized size per
  contract. It optimizes with `stellar contract optimize`, or falls back
  to `wasm-opt -Oz`. It fails above the budgets in
  `scripts/wasm-size/budgets.txt` (size + 10%). The CI `budgets` job
  uploads the optimized wasm as an artifact, then runs the resource suite
  **against the optimized wasm**.
- options_market size techniques:
  1. Each `_via_multisig` twin now shares one `*_inner(env, Auth, ...)`
     with its admin entrypoint, through `zenith_common::require_admin_or_multisig`.
     That removes 8 duplicated bodies.
  2. The multisig `contractimport!` client is removed. `zenith-common`
     calls `is_executable` through `invoke_contract`.
  3. Shared helpers replace repeated inline code: `usdc`, `load_series`,
     `load_position`, `save_*`, `refund_inner` (claim_refund plus the
     vault variant), and `settle_inner` (both settlement paths).
  4. Entrypoint `///` docs are embedded in the wasm's contract spec, so
     they are now one-line summaries. The rationale moved to `//` comments.
  5. `lto = true` in the release profile.
- **Not measured.** The ≥15% target and the before/after table need a
  build. `main`'s options_market is empty, so the "before" number has to
  come from `709ab7a` (plus its missing pieces) or from this branch with
  items 1 to 5 reverted.
- Budgets are not recorded yet. Set them with `scripts/wasm-size/check.sh --update`.

## Other fixes

- `ci.yml`: removed a stray `crate: [...]` line under `on.pull_request`.
- `tools/spec-diff/check.sh`: builds `params` before `options_market`,
  which needs params' wasm.
- The cap tests list 50 distinct strikes, so they no longer trip
  `DuplicateSeries`. `setup_multisig` passes the `Delays` and account
  arguments that #150 added.

## Verification

Nothing was compiled or run, as requested. Only `rustfmt` was applied.
Expect some compile fixes on the first build, especially in
`tools/resource-snapshot`, which reaches into host storage internals.

Closes #104
Closes #105
Closes #106
Closes #107
