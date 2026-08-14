#![no_std]

//! Zenith Vault — a per-tag escrow ledger for a single token.
//!
//! Built to close a gap discovered while testing options_market: that
//! contract holds every writer's collateral and every buyer's premium in
//! one undifferentiated token balance, with no accounting of which
//! balance is actually earmarked for which position. This contract is
//! that missing accounting layer — deposit/withdraw against a caller-
//! defined `tag` (e.g. a position_id), so "how much is earmarked for tag
//! 7" is always answerable instead of inferred from the token contract's
//! raw balance.

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env};

#[cfg(test)]
mod test;

mod error;
mod types;

use error::Error;
use types::DataKey;

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
pub struct Vault;

#[contractimpl]
impl Vault {
    pub fn initialize(env: Env, admin: Address, token: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage()
            .instance()
            .set(&DataKey::TotalEscrowed, &0i128);
    }

    /// Pulls `amount` of the vault's token from `from` into the vault's
    /// own balance, crediting `tag`'s escrow ledger. `from` must authorize
    /// the transfer — this contract never moves funds without the source
    /// account's own signature.
    pub fn deposit(env: Env, from: Address, tag: u64, amount: i128) {
        require_not_paused(&env);
        from.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let token_address: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        token::Client::new(&env, &token_address).transfer(
            &from,
            &env.current_contract_address(),
            &amount,
        );

        let key = DataKey::Escrow(tag);
        let balance: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&key, &balance.checked_add(amount).unwrap());

        let total: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalEscrowed)
            .unwrap();
        env.storage()
            .instance()
            .set(&DataKey::TotalEscrowed, &total.checked_add(amount).unwrap());
    }

    /// Pays `amount` of `tag`'s escrowed balance out to `to`, debiting the
    /// ledger. Gated on the admin's signature — set at initialize time to
    /// whichever address is trusted to trigger payouts. In the intended
    /// integration, that's the calling contract's own address (e.g.
    /// options_market's), so a contract-to-contract call satisfies
    /// require_auth() through the call itself, the same way any Soroban
    /// contract authorizes its own outgoing calls, without needing a
    /// human signature on every single settlement.
    ///
    /// Panics with InsufficientEscrowBalance if `tag` doesn't have that
    /// much earmarked — this is the check that actually closes the gap:
    /// a withdrawal can never draw down more than was specifically
    /// deposited under this tag, regardless of what the vault's raw token
    /// balance happens to be from OTHER tags' deposits.
    pub fn withdraw(env: Env, tag: u64, to: Address, amount: i128) {
        require_not_paused(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let key = DataKey::Escrow(tag);
        let balance: i128 = env.storage().persistent().get(&key).unwrap_or(0);
        if balance < amount {
            panic_with_error!(&env, Error::InsufficientEscrowBalance);
        }
        env.storage()
            .persistent()
            .set(&key, &balance.checked_sub(amount).unwrap());

        let total: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalEscrowed)
            .unwrap();
        env.storage()
            .instance()
            .set(&DataKey::TotalEscrowed, &total.checked_sub(amount).unwrap());

        let token_address: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        token::Client::new(&env, &token_address).transfer(
            &env.current_contract_address(),
            &to,
            &amount,
        );
    }

    pub fn balance_of(env: Env, tag: u64) -> i128 {
        env.storage()
            .persistent()
            .get(&DataKey::Escrow(tag))
            .unwrap_or(0)
    }

    pub fn get_total_escrowed(env: Env) -> i128 {
        env.storage()
            .instance()
            .get(&DataKey::TotalEscrowed)
            .unwrap()
    }

    pub fn get_admin(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Admin).unwrap()
    }

    pub fn get_token(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Token).unwrap()
    }
}
