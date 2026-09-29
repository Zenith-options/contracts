use soroban_sdk::{panic_with_error, symbol_short, Address, Env, Symbol, Vec};

use crate::error::Error;
use crate::math::{DEFAULT_FEE_RATE_BPS, MAX_FEE_RATE_BPS, SETTLEMENT_WINDOW};
use crate::params_client;
use crate::ttl;
use crate::types::{DataKey, OptionSeries, SeriesState};

/// Registry keys for the parameters options_market reads from params.
pub const FEE_RATE_PARAM: Symbol = symbol_short!("fee_bps");
pub const SETTLEMENT_WINDOW_PARAM: Symbol = symbol_short!("settle_w");

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

pub fn add_user_position(env: &Env, user: &Address, position_id: u64) {
    let key = DataKey::UserPositions(user.clone());
    let mut positions: Vec<u64> = ttl::get_persistent(env, &key).unwrap_or_else(|| Vec::new(env));
    positions.push_back(position_id);
    ttl::set_persistent(env, &key, &positions);
}
