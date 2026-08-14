use soroban_sdk::{panic_with_error, Address, Env, Vec};

use crate::error::Error;
use crate::types::{DataKey, OptionSeries, SeriesState};

pub fn fee_rate_bps(env: &Env) -> i128 {
    env.storage()
        .instance()
        .get(&DataKey::FeeRateBps)
        .unwrap_or(crate::math::DEFAULT_FEE_RATE_BPS)
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
    let series: OptionSeries = env
        .storage()
        .persistent()
        .get(&DataKey::Series(series_id))
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
    let mut positions: Vec<u64> = env
        .storage()
        .persistent()
        .get(&key)
        .unwrap_or_else(|| Vec::new(env));
    positions.push_back(position_id);
    env.storage().persistent().set(&key, &positions);
}
