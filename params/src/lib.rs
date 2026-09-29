#![no_std]

//! Zenith Params — a single, timelock-controlled registry for protocol
//! parameters (fee bps, settlement window, staleness, min_reports, ...).
//!
//! Every parameter carries hard `[min, max]` bounds. The timelock can
//! move a value anywhere inside its bounds with `set_param`, but
//! changing the bounds themselves goes through a slower two-step path
//! (`propose_bounds` → wait `BOUNDS_CHANGE_DELAY` → `execute_bounds`),
//! so widening the sanity envelope always takes longer than using it.
//!
//! Consumers cache values locally alongside `get_version()` and only
//! re-read parameters when the version moves (see options_market).

use soroban_sdk::{contract, contractimpl, panic_with_error, Address, Env, Symbol, Vec};

#[cfg(test)]
mod test;

mod error;
mod events;
mod ttl;
mod types;

use error::Error;
use types::DataKey;
pub use types::{BoundsProposal, Param};

/// Extra delay on top of the timelock's own delay before a bounds
/// change can take effect.
pub const BOUNDS_CHANGE_DELAY: u64 = 7 * 86_400;

#[contract]
pub struct Params;

#[contractimpl]
impl Params {
    /// Permissionless keeper entrypoint: extends the contract instance and
    /// every named persistent entry that exists, per the TTL policy in
    /// ttl.rs. Anyone may pay the rent to keep long-lived entries alive.
    pub fn bump(env: Env, keys: Vec<DataKey>) {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
    }

    pub fn initialize(env: Env, timelock: Address) {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Timelock) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        env.storage().instance().set(&DataKey::Timelock, &timelock);
        env.storage().instance().set(&DataKey::Version, &0u64);
    }

    /// Creates a parameter that has never been set. Timelock only.
    pub fn define_param(env: Env, key: Symbol, value: i128, min: i128, max: i128) {
        ttl::extend_instance(&env);
        Self::require_timelock(&env);
        let param_key = DataKey::Param(key.clone());
        if env.storage().persistent().has(&param_key) {
            panic_with_error!(&env, Error::ParamAlreadyDefined);
        }
        if min > max {
            panic_with_error!(&env, Error::InvalidBounds);
        }
        if value < min || value > max {
            panic_with_error!(&env, Error::OutOfBounds);
        }
        let param = Param {
            value,
            min,
            max,
            updated_at: env.ledger().timestamp(),
        };
        ttl::set_persistent(&env, &param_key, &param);
        Self::bump_version(&env);
        events::param_defined(&env, key, value, min, max);
    }

    /// Moves a parameter's value within its existing bounds. Timelock only.
    pub fn set_param(env: Env, key: Symbol, value: i128) {
        ttl::extend_instance(&env);
        Self::require_timelock(&env);
        let param_key = DataKey::Param(key.clone());
        let mut param = Self::load(&env, &key);
        if value < param.min || value > param.max {
            panic_with_error!(&env, Error::OutOfBounds);
        }
        let old = param.value;
        param.value = value;
        param.updated_at = env.ledger().timestamp();
        ttl::set_persistent(&env, &param_key, &param);
        Self::bump_version(&env);
        events::param_updated(&env, key, old, value);
    }

    /// Queues new bounds for `key`, executable after `BOUNDS_CHANGE_DELAY`.
    /// The current value must still fit inside the new bounds. Replaces
    /// any earlier pending proposal for the same key. Timelock only.
    pub fn propose_bounds(env: Env, key: Symbol, min: i128, max: i128) {
        ttl::extend_instance(&env);
        Self::require_timelock(&env);
        let param = Self::load(&env, &key);
        if min > max {
            panic_with_error!(&env, Error::InvalidBounds);
        }
        if param.value < min || param.value > max {
            panic_with_error!(&env, Error::OutOfBounds);
        }
        let eta = env.ledger().timestamp() + BOUNDS_CHANGE_DELAY;
        ttl::set_persistent(
            &env,
            &DataKey::PendingBounds(key.clone()),
            &BoundsProposal { min, max, eta },
        );
        events::bounds_proposed(&env, key, min, max, eta);
    }

    /// Applies a queued bounds change once its delay has elapsed. Timelock only.
    pub fn execute_bounds(env: Env, key: Symbol) {
        ttl::extend_instance(&env);
        Self::require_timelock(&env);
        let pending_key = DataKey::PendingBounds(key.clone());
        let proposal: BoundsProposal = ttl::get_persistent(&env, &pending_key)
            .unwrap_or_else(|| panic_with_error!(&env, Error::NoPendingBounds));
        if env.ledger().timestamp() < proposal.eta {
            panic_with_error!(&env, Error::BoundsDelayNotElapsed);
        }
        let mut param = Self::load(&env, &key);
        // set_param can't have moved the value outside the new bounds
        // unless they're narrower than the old ones — re-check.
        if param.value < proposal.min || param.value > proposal.max {
            panic_with_error!(&env, Error::OutOfBounds);
        }
        param.min = proposal.min;
        param.max = proposal.max;
        param.updated_at = env.ledger().timestamp();
        ttl::set_persistent(&env, &DataKey::Param(key.clone()), &param);
        env.storage().persistent().remove(&pending_key);
        Self::bump_version(&env);
        events::bounds_updated(&env, key, proposal.min, proposal.max);
    }

    pub fn cancel_bounds(env: Env, key: Symbol) {
        ttl::extend_instance(&env);
        Self::require_timelock(&env);
        let pending_key = DataKey::PendingBounds(key.clone());
        if !env.storage().persistent().has(&pending_key) {
            panic_with_error!(&env, Error::NoPendingBounds);
        }
        env.storage().persistent().remove(&pending_key);
        events::bounds_cancelled(&env, key);
    }

    pub fn get_param(env: Env, key: Symbol) -> Option<Param> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::Param(key))
    }

    pub fn get_value(env: Env, key: Symbol) -> i128 {
        ttl::extend_instance(&env);
        Self::load(&env, &key).value
    }

    pub fn get_pending_bounds(env: Env, key: Symbol) -> Option<BoundsProposal> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::PendingBounds(key))
    }

    pub fn get_version(env: Env) -> u64 {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::Version).unwrap_or(0)
    }

    pub fn get_timelock(env: Env) -> Address {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::Timelock).unwrap()
    }
}

impl Params {
    fn require_timelock(env: &Env) {
        let timelock: Address = env.storage().instance().get(&DataKey::Timelock).unwrap();
        timelock.require_auth();
    }

    fn load(env: &Env, key: &Symbol) -> Param {
        ttl::get_persistent(env, &DataKey::Param(key.clone()))
            .unwrap_or_else(|| panic_with_error!(env, Error::ParamNotFound))
    }

    fn bump_version(env: &Env) {
        let version: u64 = env.storage().instance().get(&DataKey::Version).unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::Version, &(version + 1));
    }
}
