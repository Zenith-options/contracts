#![no_std]

//! Zenith Price Oracle — on-chain price feed for the options_market contract.
//!
//! options_market currently trusts a bare `oracle: Address` with no on-chain
//! logic behind it at all — this contract is that logic: a small set of
//! admin-authorized feeders report prices per symbol, and the aggregate
//! (median across fresh reports) is what callers read.

use soroban_sdk::{contract, contractimpl, panic_with_error, Address, Env, Symbol, Vec};

#[cfg(test)]
mod test;

mod error;
mod math;
mod types;

use error::Error;
use math::{median, MAX_FEEDERS};
use types::DataKey;

const DEFAULT_MAX_STALENESS: u64 = 3600; // 1 hour

fn require_not_paused(env: &Env) {
    let paused: bool = env
        .storage()
        .instance()
        .get(&DataKey::Paused)
        .unwrap_or(false);
    if paused {
        panic_with_error!(env, Error::ContractPaused);
    }
}

#[contract]
pub struct PriceOracle;

#[contractimpl]
impl PriceOracle {
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::Feeders, &Vec::<Address>::new(&env));
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &DEFAULT_MAX_STALENESS);
    }

    /// Admin adjusts how old a feeder's report can be and still count
    /// toward get_price's aggregate.
    pub fn set_max_staleness(env: Env, seconds: u64) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        if seconds == 0 {
            panic_with_error!(&env, Error::InvalidStaleness);
        }
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &seconds);
    }

    pub fn get_max_staleness(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MaxStaleness)
            .unwrap()
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    /// Admin hands off control to a new address. Requires the CURRENT
    /// admin's signature, not the incoming one.
    pub fn transfer_admin(env: Env, new_admin: Address) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
    }

    /// Emergency stop: blocks add_feeder, remove_feeder, and report_price.
    /// Does NOT block get_price or any other view — a pause should freeze
    /// further changes to the feed, not hide the last-known price from
    /// callers still reading it.
    pub fn pause(env: Env) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
    }

    pub fn unpause(env: Env) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
    }

    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
    }

    /// Admin authorizes a new price feeder. Feeders are the only addresses
    /// allowed to call report_price.
    pub fn add_feeder(env: Env, feeder: Address) {
        require_not_paused(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let mut feeders: Vec<Address> = env.storage().instance().get(&DataKey::Feeders).unwrap();
        if feeders.contains(&feeder) {
            panic_with_error!(&env, Error::FeederAlreadyAdded);
        }
        if feeders.len() >= MAX_FEEDERS {
            panic_with_error!(&env, Error::TooManyFeeders);
        }
        feeders.push_back(feeder);
        env.storage().instance().set(&DataKey::Feeders, &feeders);
    }

    /// Admin revokes a feeder's authorization. Their most recent price
    /// report is left in storage (for audit purposes) but is no longer
    /// counted toward the aggregate once removed.
    pub fn remove_feeder(env: Env, feeder: Address) {
        require_not_paused(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let mut feeders: Vec<Address> = env.storage().instance().get(&DataKey::Feeders).unwrap();
        match feeders.first_index_of(&feeder) {
            Some(i) => {
                feeders.remove(i).unwrap();
                env.storage().instance().set(&DataKey::Feeders, &feeders);
            }
            None => panic_with_error!(&env, Error::FeederNotFound),
        }
    }

    pub fn is_feeder(env: Env, address: Address) -> bool {
        let feeders: Vec<Address> = env.storage().instance().get(&DataKey::Feeders).unwrap();
        feeders.contains(&address)
    }

    pub fn get_feeder_count(env: Env) -> u32 {
        let feeders: Vec<Address> = env.storage().instance().get(&DataKey::Feeders).unwrap();
        feeders.len()
    }

    /// A feeder reports the current price for `symbol`. Only records this
    /// feeder's own report — see the aggregation step (still to come) for
    /// how per-feeder reports become the single price callers read.
    pub fn report_price(env: Env, feeder: Address, symbol: Symbol, price: i128) {
        require_not_paused(&env);
        feeder.require_auth();

        let feeders: Vec<Address> = env.storage().instance().get(&DataKey::Feeders).unwrap();
        if !feeders.contains(&feeder) {
            panic_with_error!(&env, Error::NotAFeeder);
        }
        if price <= 0 {
            panic_with_error!(&env, Error::InvalidPrice);
        }

        let now = env.ledger().timestamp();
        env.storage()
            .persistent()
            .set(&DataKey::PriceReport(symbol, feeder), &(price, now));
    }

    pub fn get_latest_report(env: Env, symbol: Symbol, feeder: Address) -> Option<(i128, u64)> {
        env.storage()
            .persistent()
            .get(&DataKey::PriceReport(symbol, feeder))
    }

    /// The aggregate price for `symbol`: the median across every
    /// CURRENTLY authorized feeder's report that isn't older than
    /// max_staleness. Returns None if no feeder has a fresh report —
    /// callers must not treat that the same as "price is zero."
    pub fn get_price(env: Env, symbol: Symbol) -> Option<i128> {
        let feeders: Vec<Address> = env.storage().instance().get(&DataKey::Feeders).unwrap();
        let max_staleness: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MaxStaleness)
            .unwrap();
        let now = env.ledger().timestamp();

        let mut buffer = [0i128; MAX_FEEDERS as usize];
        let mut count = 0usize;
        for feeder in feeders.iter() {
            let report: Option<(i128, u64)> = env
                .storage()
                .persistent()
                .get(&DataKey::PriceReport(symbol.clone(), feeder));
            if let Some((price, reported_at)) = report {
                if now.checked_sub(reported_at).unwrap_or(u64::MAX) <= max_staleness {
                    buffer[count] = price;
                    count += 1;
                }
            }
        }

        if count == 0 {
            None
        } else {
            Some(median(&mut buffer, count))
        }
    }
}
