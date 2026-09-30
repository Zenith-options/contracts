use soroban_sdk::{panic_with_error, symbol_short, Address, Env, Symbol, Vec};

use crate::error::Error;
use crate::math::{
    DEFAULT_FEE_RATE_BPS, DEFAULT_MAX_ACTIVE_SERIES, MAX_ACTIVE_SERIES_CEILING, MAX_FEE_RATE_BPS,
    SETTLEMENT_WINDOW,
};
use crate::params_client;
use crate::ttl;
use crate::types::{
    Config, DataKey, MigrationState, OptionPosition, OptionSeries, OptionType,
    SeriesPositionCounts, SeriesState, Stats, POSITIONS_PER_PAGE,
};

/// Registry keys for the parameters options_market reads from params.
pub const FEE_RATE_PARAM: Symbol = symbol_short!("fee_bps");
pub const SETTLEMENT_WINDOW_PARAM: Symbol = symbol_short!("settle_w");
pub const MAX_ACTIVE_SERIES_PARAM: Symbol = symbol_short!("max_ser");

pub fn fee_rate_bps(env: &Env) -> i128 {
    sync_params(env);
    env.storage()
        .instance()
        .get(&DataKey::FeeRateBps)
        .unwrap_or(DEFAULT_FEE_RATE_BPS)
}

pub fn settlement_window(env: &Env) -> u64 {
    sync_params(env);
    env.storage()
        .instance()
        .get(&DataKey::SettlementWindow)
        .unwrap_or(SETTLEMENT_WINDOW)
}

/// Cap on concurrently active series per underlying: the params registry
/// value if one is set and in range, else the local override, else
/// `DEFAULT_MAX_ACTIVE_SERIES`.
pub fn max_active_series(env: &Env) -> u32 {
    sync_params(env);
    env.storage()
        .instance()
        .get(&DataKey::MaxActiveSeries)
        .unwrap_or(DEFAULT_MAX_ACTIVE_SERIES)
}

/// Refreshes the locally cached registry values, but only when the
/// registry's version has moved — one cheap cross-contract call per read
/// instead of one per parameter. A registry that's unreachable, a
/// parameter that has never been set there, or a value outside this
/// contract's own hard limits all leave the cached value untouched.
fn sync_params(env: &Env) {
    let registry: Option<Address> = env.storage().instance().get(&DataKey::ParamsRegistry);
    let Some(registry) = registry else {
        return;
    };
    let client = params_client::Client::new(env, &registry);
    let Ok(Ok(version)) = client.try_get_version() else {
        return;
    };
    let cached: Option<u64> = env.storage().instance().get(&DataKey::ParamsVersion);
    if cached == Some(version) {
        return;
    }
    if let Ok(Ok(Some(param))) = client.try_get_param(&FEE_RATE_PARAM) {
        if (0..=MAX_FEE_RATE_BPS).contains(&param.value) {
            env.storage()
                .instance()
                .set(&DataKey::FeeRateBps, &param.value);
        }
    }
    if let Ok(Ok(Some(param))) = client.try_get_param(&SETTLEMENT_WINDOW_PARAM) {
        if param.value > 0 && param.value <= u64::MAX as i128 {
            env.storage()
                .instance()
                .set(&DataKey::SettlementWindow, &(param.value as u64));
        }
    }
    if let Ok(Ok(Some(param))) = client.try_get_param(&MAX_ACTIVE_SERIES_PARAM) {
        if param.value > 0 && param.value <= MAX_ACTIVE_SERIES_CEILING as i128 {
            env.storage()
                .instance()
                .set(&DataKey::MaxActiveSeries, &(param.value as u32));
        }
    }
    env.storage()
        .instance()
        .set(&DataKey::ParamsVersion, &version);
}

pub fn require_not_paused(env: &Env) {
    let paused: bool = env
        .storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false);
    if paused {
        panic_with_error!(env, Error::ContractPaused);
    }
}

pub fn require_active_series(env: &Env, series_id: u64) -> OptionSeries {
    let series: OptionSeries = ttl::get_persistent(env, &DataKey::Series(series_id))
        .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound));
    if series.state != SeriesState::Active {
        panic_with_error!(env, Error::SeriesNotActive);
    }
    if env.ledger().timestamp() >= series.expiry {
        panic_with_error!(env, Error::SeriesNotActive);
    }
    series
}

pub fn next_position_id(env: &Env) -> u64 {
    let counter: u64 = env
        .storage()
        .instance()
        .get(&DataKey::PositionCounter)
        .unwrap_or(0);
    let next = counter.checked_add(1).unwrap();
    env.storage()
        .instance()
        .set(&DataKey::PositionCounter, &next);
    next
}

pub fn add_orphaned_liability(env: &Env, amount: i128) {
    if amount <= 0 {
        return;
    }
    let current: i128 = env
        .storage()
        .instance()
        .get(&DataKey::OrphanedLiabilities)
        .unwrap_or(0);
    env.storage().instance().set(
        &DataKey::OrphanedLiabilities,
        &current.checked_add(amount).unwrap(),
    );
}

pub fn deduct_orphaned_liability(env: &Env, amount: i128) {
    if amount <= 0 {
        return;
    }
    let current: i128 = env
        .storage()
        .instance()
        .get(&DataKey::OrphanedLiabilities)
        .unwrap_or(0);
    env.storage().instance().set(
        &DataKey::OrphanedLiabilities,
        &current.saturating_sub(amount),
    );
}

/// Reserves `(underlying, option_type, strike_price, expiry)` for
/// `series_id`, rejecting a spec that is already listed with
/// `DuplicateSeries`. The reservation lasts until `prune_series`.
pub fn claim_series_index(
    env: &Env,
    underlying: &Symbol,
    option_type: &OptionType,
    strike_price: i128,
    expiry: u64,
    series_id: u64,
) {
    let key = DataKey::SeriesIndex(
        underlying.clone(),
        option_type.clone(),
        strike_price,
        expiry,
    );
    if env.storage().persistent().has(&key) {
        panic_with_error!(env, Error::DuplicateSeries);
    }
    ttl::set_persistent(env, &key, &series_id);
}

pub fn remove_user_position(env: &Env, user: &Address, position_id: u64) {
    let key = DataKey::UserPositions(user.clone());
    let Some(mut positions) = ttl::get_persistent::<_, Vec<u64>>(env, &key) else {
        return;
    };
    if let Some(i) = positions.first_index_of(position_id) {
        positions.remove(i);
    }
    if positions.is_empty() {
        env.storage().persistent().remove(&key);
    } else {
        ttl::set_persistent(env, &key, &positions);
    }
}

// ── Active-series cap and position counters ──────────────────────────────────

pub fn migration_state(env: &Env) -> MigrationState {
    env.storage()
        .instance()
        .get(&DataKey::Migration)
        .unwrap_or_default()
}

pub fn set_migration_state(env: &Env, state: &MigrationState) {
    env.storage().instance().set(&DataKey::Migration, state);
}

pub fn migration_done(env: &Env) -> bool {
    migration_state(env).series_cursor == u64::MAX
}

pub fn require_migrated(env: &Env) {
    if !migration_done(env) {
        panic_with_error!(env, Error::MigrationPending);
    }
}

/// Whether `position_id` is already reflected in `SeriesPositions`, so
/// live code must keep its counts up to date. Positions above the
/// migration cursor are counted by `migrate_counters` instead.
fn position_counted(env: &Env, position_id: u64) -> bool {
    position_id <= migration_state(env).position_cursor
}

pub fn series_counts(env: &Env, series_id: u64) -> SeriesPositionCounts {
    ttl::get_persistent(env, &DataKey::SeriesPositions(series_id)).unwrap_or_default()
}

pub fn set_series_counts(env: &Env, series_id: u64, counts: &SeriesPositionCounts) {
    ttl::set_persistent(env, &DataKey::SeriesPositions(series_id), counts);
}

/// A new position (buy, write, split) in `series_id`.
pub fn count_position_opened(env: &Env, series_id: u64, position_id: u64) {
    if !position_counted(env, position_id) {
        return;
    }
    let mut counts = series_counts(env, series_id);
    counts.live = counts.live.checked_add(1).unwrap();
    counts.open = counts.open.checked_add(1).unwrap();
    set_series_counts(env, series_id, &counts);
}

/// A position closed by a refund, exercise, reclaim or forfeiture. Callers
/// only call this on the transition, so it runs once per position.
pub fn count_position_closed(env: &Env, series_id: u64, position_id: u64) {
    if !position_counted(env, position_id) {
        return;
    }
    let mut counts = series_counts(env, series_id);
    counts.open = counts.open.saturating_sub(1);
    set_series_counts(env, series_id, &counts);
}

/// A position removed by `prune_positions`. `was_open` is true for a
/// position pruned without ever being closed (an OTM long).
pub fn count_position_pruned(env: &Env, series_id: u64, was_open: bool) {
    let mut counts = series_counts(env, series_id);
    counts.live = counts.live.saturating_sub(1);
    if was_open {
        counts.open = counts.open.saturating_sub(1);
    }
    set_series_counts(env, series_id, &counts);
}

pub fn active_series_count(env: &Env, underlying: &Symbol) -> u32 {
    ttl::get_persistent(env, &DataKey::ActiveSeriesCount(underlying.clone())).unwrap_or(0)
}

pub fn set_active_series_count(env: &Env, underlying: &Symbol, count: u32) {
    ttl::set_persistent(env, &DataKey::ActiveSeriesCount(underlying.clone()), &count);
}

/// Whether `series` no longer needs a listing slot:
/// - Settled, once the exercise window (`expiry + settlement_window`) has
///   closed. Unexercised ITM longs don't keep it active: they're tracked
///   by `OrphanedLiabilities` and still block pruning until swept.
/// - Cancelled, once every position has taken its refund (`open == 0`).
/// - Active (including expired but not yet settled): never.
pub fn is_slot_releasable(env: &Env, series: &OptionSeries, counts: &SeriesPositionCounts) -> bool {
    match series.state {
        SeriesState::Settled => {
            env.ledger().timestamp() > series.expiry.saturating_add(settlement_window(env))
        }
        SeriesState::Cancelled => counts.open == 0,
        SeriesState::Active | SeriesState::Expired => false,
    }
}

/// Decrements `ActiveSeriesCount(underlying)` for `series`, at most once
/// ever (guarded by `SeriesReleased`). Returns whether it decremented.
/// A no-op until `migrate_counters` has finished, since the migration
/// decides the slot of every pre-upgrade series itself.
pub fn mark_slot_released(env: &Env, series: &OptionSeries) -> bool {
    if !migration_done(env) {
        return false;
    }
    let key = DataKey::SeriesReleased(series.series_id);
    if env.storage().persistent().has(&key) {
        return false;
    }
    ttl::set_persistent(env, &key, &true);
    let count = active_series_count(env, &series.underlying);
    set_active_series_count(env, &series.underlying, count.saturating_sub(1));
    true
}

pub fn set_max_active_series(env: &Env, cap: u32) {
    if cap == 0 || cap > MAX_ACTIVE_SERIES_CEILING {
        panic_with_error!(env, Error::InvalidActiveSeriesCap);
    }
    env.storage()
        .instance()
        .set(&DataKey::MaxActiveSeries, &cap);
}

// ─── Config / Stats (issue #102) ───────────────────────────────────────────

pub fn load_config(env: &Env) -> Config {
    env.storage().instance().get(&DataKey::Config).unwrap()
}

pub fn save_config(env: &Env, config: &Config) {
    env.storage().instance().set(&DataKey::Config, config);
}

pub fn load_stats(env: &Env) -> Stats {
    env.storage()
        .instance()
        .get(&DataKey::Stats)
        .unwrap_or(Stats {
            premiums_collected: 0,
            open_interest: 0,
            premium_pool: 0,
            series_count: 0,
            position_counter: 0,
        })
}

pub fn save_stats(env: &Env, stats: &Stats) {
    env.storage().instance().set(&DataKey::Stats, stats);
}

// ─── Positions (issue #103) ────────────────────────────────────────────────

pub fn load_position(env: &Env, position_id: u64) -> Option<OptionPosition> {
    env.storage()
        .persistent()
        .get(&DataKey::Position(position_id))
}

pub fn save_position(env: &Env, position_id: u64, position: &OptionPosition) {
    env.storage()
        .persistent()
        .set(&DataKey::Position(position_id), position);
}

// ─── Paginated user position index (issue #100) ────────────────────────────
//
// Ids are appended to fixed-size pages (`POSITIONS_PER_PAGE` per page)
// instead of one ever-growing `Vec<u64>`. Each page is its own persistent
// entry, so a single trade only reads and rewrites the current page rather
// than the user's whole history — bounded write bytes no matter how many
// positions the user has opened.

pub fn user_position_count(env: &Env, user: &Address) -> u32 {
    env.storage()
        .persistent()
        .get(&DataKey::UserPositionCount(user.clone()))
        .unwrap_or(0)
}

fn load_page(env: &Env, user: &Address, page: u32) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::UserPositionPage(user.clone(), page))
        .unwrap_or(Vec::new(env))
}

/// O(1) append: only the current (partially-filled) page is read and
/// rewritten; earlier pages are untouched.
pub fn add_user_position(env: &Env, user: &Address, position_id: u64) {
    let count = user_position_count(env, user);
    let page_index = count / POSITIONS_PER_PAGE;
    let mut page = load_page(env, user, page_index);
    page.push_back(position_id);
    env.storage()
        .persistent()
        .set(&DataKey::UserPositionPage(user.clone(), page_index), &page);
    env.storage()
        .persistent()
        .set(&DataKey::UserPositionCount(user.clone()), &(count + 1));
}

/// Returns up to `limit` position ids starting at `cursor` (an index into
/// the user's full history, oldest first).
pub fn get_user_positions_page(env: &Env, user: &Address, cursor: u32, limit: u32) -> Vec<u64> {
    let total = user_position_count(env, user);
    let mut out = Vec::new(env);
    let mut i = cursor;
    let end = total.min(cursor.saturating_add(limit));
    while i < end {
        let page = load_page(env, user, i / POSITIONS_PER_PAGE);
        if let Some(id) = page.get(i % POSITIONS_PER_PAGE) {
            out.push_back(id);
        }
        i += 1;
    }
    out
}
}
