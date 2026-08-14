#![no_std]

//! Zenith Price Oracle — on-chain price feed for the options_market contract.
//!
//! options_market currently trusts a bare `oracle: Address` with no on-chain
//! logic behind it at all — this contract is that logic: a small set of
//! admin-authorized feeders report prices per symbol, and the aggregate
//! (median across fresh reports) is what callers read.

use soroban_sdk::{contract, contractimpl, panic_with_error, Address, Env};

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
    }
}
