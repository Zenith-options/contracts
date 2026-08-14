#![no_std]

//! Zenith Price Oracle — on-chain price feed for the options_market contract.
//!
//! options_market currently trusts a bare `oracle: Address` with no on-chain
//! logic behind it at all — this contract is that logic: a small set of
//! admin-authorized feeders report prices per symbol, and the aggregate
//! (median across fresh reports) is what callers read.

use soroban_sdk::{contract, contractimpl, panic_with_error, Address, Env, Vec};

#[cfg(test)]
mod test;

mod error;
mod types;

use error::Error;
use types::DataKey;

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
        env.storage().instance().set(&DataKey::Feeders, &Vec::<Address>::new(&env));
    }

    /// Admin authorizes a new price feeder. Feeders are the only addresses
    /// allowed to call report_price.
    pub fn add_feeder(env: Env, feeder: Address) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let mut feeders: Vec<Address> = env.storage().instance().get(&DataKey::Feeders).unwrap();
        if feeders.contains(&feeder) {
            panic_with_error!(&env, Error::FeederAlreadyAdded);
        }
        feeders.push_back(feeder);
        env.storage().instance().set(&DataKey::Feeders, &feeders);
    }

    /// Admin revokes a feeder's authorization. Their most recent price
    /// report is left in storage (for audit purposes) but is no longer
    /// counted toward the aggregate once removed.
    pub fn remove_feeder(env: Env, feeder: Address) {
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
}
