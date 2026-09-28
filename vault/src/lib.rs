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
use zenith_common::{require_admin_or_multisig, ActionClass, Auth};

#[cfg(test)]
mod test;

mod error;
mod events;
mod types;

use error::Error;
use types::DataKey;

fn require_not_paused(env: &Env) {
    zenith_common::require_not_paused(env, &DataKey::Paused, Error::ContractPaused);
}

fn require_auth(env: &Env, auth: &Auth, class: ActionClass) {
    require_admin_or_multisig(env, &DataKey::Admin, auth, class, Error::Unauthorized);
}

fn debit(env: &Env, tag: u64, amount: i128) {
    let key = DataKey::Escrow(tag);
    let balance: i128 = env.storage().persistent().get(&key).unwrap_or(0);
    if balance < amount {
        panic_with_error!(env, Error::InsufficientEscrowBalance);
    }
    env.storage()
        .persistent()
        .set(&key, &balance.checked_sub(amount).unwrap());
}

fn pay_out(env: &Env, to: &Address, amount: i128) {
    let total: i128 = env
        .storage()
        .instance()
        .get(&DataKey::TotalEscrowed)
        .unwrap();
    env.storage()
        .instance()
        .set(&DataKey::TotalEscrowed, &total.checked_sub(amount).unwrap());

    let token_address: Address = env.storage().instance().get(&DataKey::Token).unwrap();
    token::Client::new(env, &token_address).transfer(&env.current_contract_address(), to, &amount);
}

/// Total seconds the vault counts as paused toward MaxPauseDuration:
/// time accrued by earlier pauses plus the current pause, if any.
fn pause_duration(env: &Env) -> u64 {
    let accrued: u64 = env
        .storage()
        .instance()
        .get(&DataKey::PauseAccrued)
        .unwrap_or(0);
    let current = match Vault::get_paused_at(env.clone()) {
        Some(paused_at) if zenith_common::is_paused(env, &DataKey::Paused) => {
            env.ledger().timestamp().saturating_sub(paused_at)
        }
        _ => 0,
    };
    accrued.saturating_add(current)
}

fn set_paused(env: &Env, paused: bool) {
    let now = env.ledger().timestamp();
    let was_paused = zenith_common::is_paused(env, &DataKey::Paused);
    let max: u64 = env
        .storage()
        .instance()
        .get(&DataKey::MaxPauseDuration)
        .unwrap();
    if paused && !was_paused {
        // Accrued pause time only resets after the vault has stayed
        // unpaused for a full MaxPauseDuration, so a brief unpause/re-
        // pause can't restart the escape-hatch clock.
        let unpaused_at: Option<u64> = env.storage().instance().get(&DataKey::UnpausedAt);
        if unpaused_at.is_none_or(|t| now.saturating_sub(t) >= max) {
            env.storage().instance().set(&DataKey::PauseAccrued, &0u64);
        }
        env.storage().instance().set(&DataKey::PausedAt, &now);
    } else if !paused && was_paused {
        let accrued = pause_duration(env);
        env.storage()
            .instance()
            .set(&DataKey::PauseAccrued, &accrued);
        env.storage().instance().set(&DataKey::UnpausedAt, &now);
    }
    zenith_common::set_paused(env, &DataKey::Paused, paused);
    if paused {
        events::paused(env);
    } else {
        events::unpaused(env);
    }
}

#[contract]
pub struct Vault;

impl Vault {
    fn transfer_admin_inner(env: Env, auth: Auth, new_admin: Address) {
        require_auth(&env, &auth, ActionClass::Critical);
        let admin = zenith_common::set_admin(&env, &DataKey::Admin, &new_admin);
        events::admin_transferred(&env, admin, new_admin);
    }

    fn pause_inner(env: Env, auth: Auth) {
        require_auth(&env, &auth, ActionClass::Emergency);
        set_paused(&env, true);
    }

    fn unpause_inner(env: Env, auth: Auth) {
        require_auth(&env, &auth, ActionClass::Standard);
        set_paused(&env, false);
    }

    fn withdraw_inner(env: Env, auth: Auth, tag: u64, to: Address, amount: i128) {
        require_not_paused(&env);
        require_auth(&env, &auth, ActionClass::Critical);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        debit(&env, tag, amount);
        pay_out(&env, &to, amount);
        events::withdrawn(&env, to, tag, amount);
    }

    fn transfer_tag_inner(env: Env, auth: Auth, from_tag: u64, to_tag: u64, amount: i128) {
        require_not_paused(&env);
        require_auth(&env, &auth, ActionClass::Critical);
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        debit(&env, from_tag, amount);

        let to_key = DataKey::Escrow(to_tag);
        let to_balance: i128 = env.storage().persistent().get(&to_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&to_key, &to_balance.checked_add(amount).unwrap());

        // TotalEscrowed is unaffected — nothing entered or left the vault,
        // only which tag it's earmarked under changed.
        events::tag_transferred(&env, from_tag, to_tag, amount);
    }

    fn sweep_untagged_inner(env: Env, auth: Auth, to: Address) -> i128 {
        require_auth(&env, &auth, ActionClass::Critical);

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

#[contractimpl]
impl Vault {
    /// `max_pause_duration` (seconds) is fixed here and immutable: once
    /// the vault has been paused for longer than this, tag beneficiaries
    /// can `emergency_withdraw` their own balances.
    pub fn initialize(env: Env, admin: Address, token: Address, max_pause_duration: u64) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        if max_pause_duration == 0 {
            panic_with_error!(&env, Error::InvalidPauseDuration);
        }
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Token, &token);
        env.storage()
            .instance()
            .set(&DataKey::TotalEscrowed, &0i128);
        env.storage()
            .instance()
            .set(&DataKey::MaxPauseDuration, &max_pause_duration);
    }

    /// Hands off control to a new address. Requires the CURRENT admin's
    /// signature, not the incoming one. Since `admin` also gates every
    /// withdraw, this is how the calling contract a vault serves would
    /// change (e.g. after an options_market upgrade to a new contract
    /// address).
    pub fn transfer_admin(env: Env, new_admin: Address) {
        Self::transfer_admin_inner(env, Auth::Admin, new_admin);
    }

    /// Permissionless alternative to transfer_admin: cross-calls a
    /// deployed Multisig and checks is_executable(action_id, Critical)
    /// instead of requiring the current admin's own signature — same
    /// pattern as options_market's and price_oracle's
    /// transfer_admin_via_multisig.
    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_admin: Address,
    ) {
        Self::transfer_admin_inner(env, Auth::Multisig(multisig_contract, action_id), new_admin);
    }

    /// Emergency stop: blocks deposit and withdraw. Both sides, unlike
    /// options_market's pause (which only blocks new exposure, not
    /// winding existing positions down). Not a custody seizure: once
    /// paused for longer than MaxPauseDuration (cumulatively — see
    /// `set_paused`), registered beneficiaries can `emergency_withdraw`.
    pub fn pause(env: Env) {
        Self::pause_inner(env, Auth::Admin);
    }

    pub fn unpause(env: Env) {
        Self::unpause_inner(env, Auth::Admin);
    }

    /// Permissionless alternative to pause(), same rationale and pattern
    /// as options_market's and price_oracle's pause_via_multisig.
    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        Self::pause_inner(env, Auth::Multisig(multisig_contract, action_id));
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        Self::unpause_inner(env, Auth::Multisig(multisig_contract, action_id));
    }

    pub fn is_paused(env: Env) -> bool {
        zenith_common::is_paused(&env, &DataKey::Paused)
    }

    /// Pulls `amount` of the vault's token from `from` into the vault's
    /// own balance, crediting `tag`'s escrow ledger. `from` must authorize
    /// the transfer — this contract never moves funds without the source
    /// account's own signature. The first depositor to a tag becomes its
    /// owner, the only address that can `set_beneficiary` for it.
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

        let owner_key = DataKey::TagOwner(tag);
        if !env.storage().persistent().has(&owner_key) {
            env.storage().persistent().set(&owner_key, &from);
        }

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
        Self::withdraw_inner(env, Auth::Admin, tag, to, amount);
    }

    /// Permissionless alternative to withdraw: cross-calls a deployed
    /// Multisig and checks is_executable(action_id, Critical) instead of
    /// requiring the admin's own signature. Useful for a manual-recovery
    /// or migration path where the intended calling contract itself
    /// can't produce the admin's signature. Same
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
        Self::withdraw_inner(
            env,
            Auth::Multisig(multisig_contract, action_id),
            tag,
            to,
            amount,
        );
    }

    /// Moves `amount` of escrow from `from_tag` to `to_tag` without any
    /// token movement at all — a pure ledger reassignment. Meant for
    /// exactly the case options_market's roll_position represents: a
    /// position closes and its replacement opens in the same breath, so
    /// the collateral doesn't need to leave the vault and come back, it
    /// just needs to be re-earmarked under the new position's tag.
    /// Admin-gated, same as withdraw.
    pub fn transfer_tag(env: Env, from_tag: u64, to_tag: u64, amount: i128) {
        Self::transfer_tag_inner(env, Auth::Admin, from_tag, to_tag, amount);
    }

    /// Permissionless alternative to transfer_tag, same rationale as
    /// withdraw_via_multisig.
    pub fn transfer_tag_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        from_tag: u64,
        to_tag: u64,
        amount: i128,
    ) {
        Self::transfer_tag_inner(
            env,
            Auth::Multisig(multisig_contract, action_id),
            from_tag,
            to_tag,
            amount,
        );
    }

    /// Registers who may `emergency_withdraw` `tag`'s balance after a
    /// prolonged pause. Only the tag's owner (its first depositor) can
    /// set it, and not while the vault is paused, so a beneficiary can't
    /// be redirected once the escape-hatch clock is running.
    pub fn set_beneficiary(env: Env, tag: u64, beneficiary: Address) {
        require_not_paused(&env);
        let owner: Address = env
            .storage()
            .persistent()
            .get(&DataKey::TagOwner(tag))
            .unwrap_or_else(|| panic_with_error!(&env, Error::TagNotOwned));
        owner.require_auth();
        env.storage()
            .persistent()
            .set(&DataKey::Beneficiary(tag), &beneficiary);
        events::beneficiary_set(&env, tag, beneficiary);
    }

    /// Escape hatch: once the vault has been paused for longer than
    /// MaxPauseDuration, `tag`'s registered beneficiary can pull the
    /// tag's entire balance to themselves without the admin.
    pub fn emergency_withdraw(env: Env, tag: u64, beneficiary: Address) -> i128 {
        beneficiary.require_auth();
        if !zenith_common::is_paused(&env, &DataKey::Paused) {
            panic_with_error!(&env, Error::EmergencyNotAvailable);
        }
        let max: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MaxPauseDuration)
            .unwrap();
        if pause_duration(&env) <= max {
            panic_with_error!(&env, Error::EmergencyNotAvailable);
        }
        let registered: Option<Address> =
            env.storage().persistent().get(&DataKey::Beneficiary(tag));
        if registered != Some(beneficiary.clone()) {
            panic_with_error!(&env, Error::NotBeneficiary);
        }

        let amount = Self::balance_of(env.clone(), tag);
        if amount <= 0 {
            panic_with_error!(&env, Error::InsufficientEscrowBalance);
        }
        debit(&env, tag, amount);
        pay_out(&env, &beneficiary, amount);
        events::emergency_withdrawn(&env, tag, beneficiary, amount);
        amount
    }

    pub fn get_beneficiary(env: Env, tag: u64) -> Option<Address> {
        env.storage().persistent().get(&DataKey::Beneficiary(tag))
    }

    pub fn get_tag_owner(env: Env, tag: u64) -> Option<Address> {
        env.storage().persistent().get(&DataKey::TagOwner(tag))
    }

    pub fn get_max_pause_duration(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MaxPauseDuration)
            .unwrap()
    }

    /// When the current (or most recent) pause started.
    pub fn get_paused_at(env: Env) -> Option<u64> {
        env.storage().instance().get(&DataKey::PausedAt)
    }

    /// Seconds counted toward MaxPauseDuration right now.
    pub fn get_pause_duration(env: Env) -> u64 {
        pause_duration(&env)
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
        zenith_common::get_admin(&env, &DataKey::Admin)
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
        Self::sweep_untagged_inner(env, Auth::Admin, to)
    }

    /// Permissionless alternative to sweep_untagged, same rationale as
    /// withdraw_via_multisig.
    pub fn sweep_untagged_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        to: Address,
    ) -> i128 {
        Self::sweep_untagged_inner(env, Auth::Multisig(multisig_contract, action_id), to)
    }
}
