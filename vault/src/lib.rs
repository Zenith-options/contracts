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
mod events;
mod multisig_client;
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

    /// Hands off control to a new address. Requires the CURRENT admin's
    /// signature, not the incoming one. Since `admin` also gates every
    /// withdraw, this is how the calling contract a vault serves would
    /// change (e.g. after an options_market upgrade to a new contract
    /// address).
    pub fn transfer_admin(env: Env, new_admin: Address) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::admin_transferred(&env, admin, new_admin);
    }

    /// Permissionless alternative to transfer_admin: cross-calls a
    /// deployed Multisig and checks is_approved(action_id) instead of
    /// requiring the current admin's own signature — same pattern as
    /// options_market's and price_oracle's transfer_admin_via_multisig.
    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_admin: Address,
    ) {
        let multisig = multisig_client::Client::new(&env, &multisig_contract);
        if !multisig.is_approved(&action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        env.storage().instance().set(&DataKey::Admin, &new_admin);
        events::admin_transferred(&env, admin, new_admin);
    }

    /// Emergency stop: blocks deposit and withdraw. Both sides, unlike
    /// options_market's pause (which only blocks new exposure, not
    /// winding existing positions down) — a vault holding real funds
    /// should be freezable outright if something's gone wrong, since
    /// there's no "existing position" here that needs an exit path
    /// independent of the vault itself.
    pub fn pause(env: Env) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &true);
        events::paused(&env);
    }

    pub fn unpause(env: Env) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        env.storage().instance().set(&DataKey::Paused, &false);
        events::unpaused(&env);
    }

    /// Permissionless alternative to pause(), same rationale and pattern
    /// as options_market's and price_oracle's pause_via_multisig.
    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        let multisig = multisig_client::Client::new(&env, &multisig_contract);
        if !multisig.is_approved(&action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        env.storage().instance().set(&DataKey::Paused, &true);
        events::paused(&env);
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        let multisig = multisig_client::Client::new(&env, &multisig_contract);
        if !multisig.is_approved(&action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        env.storage().instance().set(&DataKey::Paused, &false);
        events::unpaused(&env);
    }

    pub fn is_paused(env: Env) -> bool {
        env.storage()
            .instance()
            .get(&DataKey::Paused)
            .unwrap_or(false)
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

        events::deposited(&env, from, tag, amount);
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

        events::withdrawn(&env, to, tag, amount);
    }

    /// Permissionless alternative to withdraw: cross-calls a deployed
    /// Multisig and checks is_approved(action_id) instead of requiring
    /// the admin's own signature. Useful for a manual-recovery or
    /// migration path where the intended calling contract itself can't
    /// produce the admin's signature (e.g. it's being replaced), so an
    /// M-of-N-approved payout is the only way to move funds out. Same
    /// InsufficientEscrowBalance cap applies — approval changes who can
    /// call this, not how much any tag is entitled to.
    pub fn withdraw_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        tag: u64,
        to: Address,
        amount: i128,
    ) {
        require_not_paused(&env);
        let multisig = multisig_client::Client::new(&env, &multisig_contract);
        if !multisig.is_approved(&action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
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

        events::withdrawn(&env, to, tag, amount);
    }

    /// Moves `amount` of escrow from `from_tag` to `to_tag` without any
    /// token movement at all — a pure ledger reassignment. Meant for
    /// exactly the case options_market's roll_position represents: a
    /// position closes and its replacement opens in the same breath, so
    /// the collateral doesn't need to leave the vault and come back, it
    /// just needs to be re-earmarked under the new position's tag.
    /// Admin-gated, same as withdraw.
    pub fn transfer_tag(env: Env, from_tag: u64, to_tag: u64, amount: i128) {
        require_not_paused(&env);
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let from_key = DataKey::Escrow(from_tag);
        let from_balance: i128 = env.storage().persistent().get(&from_key).unwrap_or(0);
        if from_balance < amount {
            panic_with_error!(&env, Error::InsufficientEscrowBalance);
        }
        env.storage()
            .persistent()
            .set(&from_key, &from_balance.checked_sub(amount).unwrap());

        let to_key = DataKey::Escrow(to_tag);
        let to_balance: i128 = env.storage().persistent().get(&to_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&to_key, &to_balance.checked_add(amount).unwrap());

        // TotalEscrowed is unaffected — nothing entered or left the vault,
        // only which tag it's earmarked under changed.
        events::tag_transferred(&env, from_tag, to_tag, amount);
    }

    /// Permissionless alternative to transfer_tag: cross-calls a deployed
    /// Multisig and checks is_approved(action_id) instead of requiring
    /// the admin's own signature. Same rationale as withdraw_via_multisig.
    pub fn transfer_tag_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        from_tag: u64,
        to_tag: u64,
        amount: i128,
    ) {
        require_not_paused(&env);
        let multisig = multisig_client::Client::new(&env, &multisig_contract);
        if !multisig.is_approved(&action_id) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }

        let from_key = DataKey::Escrow(from_tag);
        let from_balance: i128 = env.storage().persistent().get(&from_key).unwrap_or(0);
        if from_balance < amount {
            panic_with_error!(&env, Error::InsufficientEscrowBalance);
        }
        env.storage()
            .persistent()
            .set(&from_key, &from_balance.checked_sub(amount).unwrap());

        let to_key = DataKey::Escrow(to_tag);
        let to_balance: i128 = env.storage().persistent().get(&to_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&to_key, &to_balance.checked_add(amount).unwrap());

        events::tag_transferred(&env, from_tag, to_tag, amount);
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

    /// Recovers tokens that landed on the vault's own address OUTSIDE
    /// deposit() — e.g. a direct token transfer sent straight to this
    /// contract's address by mistake, rather than through deposit(),
    /// which is the only path that actually credits a tag's ledger. Those
    /// tokens sit in the vault's real balance but aren't accounted for
    /// under any tag, so they'd otherwise be stuck forever: no tag's
    /// withdraw could ever reach them (withdraw is capped at that tag's
    /// OWN escrowed balance), and get_total_escrowed's ledger sum would
    /// permanently under-report the vault's actual token balance.
    pub fn sweep_untagged(env: Env, to: Address) -> i128 {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let token_address: Address = env.storage().instance().get(&DataKey::Token).unwrap();
        let token_client = token::Client::new(&env, &token_address);
        let actual_balance = token_client.balance(&env.current_contract_address());
        let total_escrowed: i128 = env
            .storage()
            .instance()
            .get(&DataKey::TotalEscrowed)
            .unwrap();

        let untagged = actual_balance.checked_sub(total_escrowed).unwrap();
        if untagged <= 0 {
            panic_with_error!(&env, Error::NoUntaggedFunds);
        }

        token_client.transfer(&env.current_contract_address(), &to, &untagged);
        events::swept_untagged(&env, to, untagged);
        untagged
    }
}
