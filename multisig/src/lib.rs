#![no_std]

//! Zenith Multisig — M-of-N approval tracking for opaque, caller-defined
//! actions.
//!
//! Every contract in this repo (options_market, price_oracle, vault) has
//! a single `admin: Address` as its sole point of control — one
//! compromised or lost key controls pause/transfer_admin/fee-rate/upgrade
//! for the whole thing. This contract doesn't fix that by itself (it
//! isn't wired into any of the other three yet — see the README), but it
//! provides the primitive a real fix would need: a fixed set of signers,
//! a threshold, and `is_approved(action_id)` that flips true once enough
//! of them have signed off. `action_id` is caller-defined and never
//! interpreted here — this contract only counts signatures, not what
//! they're for.
//!
//! Signers and threshold are fixed at `initialize` and immutable —
//! changing the signer set means deploying a new Multisig. A generic
//! "vote to change your own signer set" mechanism was left out
//! deliberately to keep this contract's own trust model simple: there's
//! no in-protocol path for a compromised signer to add another
//! compromised signer.

use soroban_sdk::{contract, contractimpl, panic_with_error, Address, Env, Vec};

#[cfg(test)]
mod test;

mod error;
mod types;

use error::Error;
use types::DataKey;

#[contract]
pub struct Multisig;

#[contractimpl]
impl Multisig {
    pub fn initialize(env: Env, signers: Vec<Address>, threshold: u32) {
        if env.storage().instance().has(&DataKey::Signers) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        if threshold == 0 || threshold > signers.len() {
            panic_with_error!(&env, Error::InvalidThreshold);
        }
        for i in 0..signers.len() {
            for j in (i + 1)..signers.len() {
                if signers.get(i).unwrap() == signers.get(j).unwrap() {
                    panic_with_error!(&env, Error::DuplicateSigner);
                }
            }
        }

        env.storage().instance().set(&DataKey::Signers, &signers);
        env.storage()
            .instance()
            .set(&DataKey::Threshold, &threshold);
    }

    pub fn is_signer(env: Env, address: Address) -> bool {
        let signers: Vec<Address> = env.storage().instance().get(&DataKey::Signers).unwrap();
        signers.contains(&address)
    }

    pub fn get_signer_count(env: Env) -> u32 {
        let signers: Vec<Address> = env.storage().instance().get(&DataKey::Signers).unwrap();
        signers.len()
    }

    pub fn get_threshold(env: Env) -> u32 {
        env.storage().instance().get(&DataKey::Threshold).unwrap()
    }

    /// Records `signer`'s approval of `action_id`. Requires the signer's
    /// own signature and current membership in the fixed signer set.
    pub fn approve(env: Env, signer: Address, action_id: u64) {
        signer.require_auth();

        let signers: Vec<Address> = env.storage().instance().get(&DataKey::Signers).unwrap();
        if !signers.contains(&signer) {
            panic_with_error!(&env, Error::NotASigner);
        }

        let approval_key = DataKey::Approval(action_id, signer);
        if env
            .storage()
            .persistent()
            .get(&approval_key)
            .unwrap_or(false)
        {
            panic_with_error!(&env, Error::AlreadyApproved);
        }
        env.storage().persistent().set(&approval_key, &true);

        let count_key = DataKey::ApprovalCount(action_id);
        let count: u32 = env.storage().persistent().get(&count_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&count_key, &count.checked_add(1).unwrap());
    }

    /// Withdraws `signer`'s own approval of `action_id` — e.g. they
    /// approved before new information came in and want to reconsider.
    pub fn revoke(env: Env, signer: Address, action_id: u64) {
        signer.require_auth();

        let approval_key = DataKey::Approval(action_id, signer);
        if !env
            .storage()
            .persistent()
            .get(&approval_key)
            .unwrap_or(false)
        {
            panic_with_error!(&env, Error::NotYetApproved);
        }
        env.storage().persistent().set(&approval_key, &false);

        let count_key = DataKey::ApprovalCount(action_id);
        let count: u32 = env.storage().persistent().get(&count_key).unwrap_or(0);
        env.storage()
            .persistent()
            .set(&count_key, &count.saturating_sub(1));
    }

    pub fn has_approved(env: Env, action_id: u64, signer: Address) -> bool {
        env.storage()
            .persistent()
            .get(&DataKey::Approval(action_id, signer))
            .unwrap_or(false)
    }

    pub fn get_approval_count(env: Env, action_id: u64) -> u32 {
        env.storage()
            .persistent()
            .get(&DataKey::ApprovalCount(action_id))
            .unwrap_or(0)
    }

    pub fn is_approved(env: Env, action_id: u64) -> bool {
        let count = Self::get_approval_count(env.clone(), action_id);
        let threshold: u32 = env.storage().instance().get(&DataKey::Threshold).unwrap();
        count >= threshold
    }
}
