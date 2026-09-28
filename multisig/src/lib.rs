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
//!
//! Two ways to use it (see the README for when to use which):
//! * Approval mode: signers `approve(action_id)` on-chain and a consumer's
//!   `_via_multisig` entrypoint checks `is_executable(action_id, class)`,
//!   which also enforces the per-class timelock.
//! * Custom-account mode: set this contract's address as a plain `admin`
//!   anywhere; `__check_auth` accepts M-of-N ed25519 signatures from the
//!   transaction's auth payload. No timelock applies in this mode.

use soroban_sdk::{
    auth::{Context, CustomAccountInterface},
    contract, contractimpl,
    crypto::Hash,
    panic_with_error, Address, Env, Vec,
};

#[cfg(test)]
mod test;

mod error;
mod events;
mod types;

pub use error::Error;
use types::DataKey;
pub use types::{AccountConfig, AccountSignature, ActionClass, Delays};

#[contract]
pub struct Multisig;

#[contractimpl]
impl Multisig {
    /// Deliberately does NOT call require_auth() on any signer, unlike
    /// every other contract's initialize() here requiring its incoming
    /// admin's signature. Naming an address as a signer costs an
    /// attacker nothing without that address's cooperation: approve()
    /// still requires the REAL signer's own signature, so a Multisig
    /// initialized with signers who never consented can simply never
    /// reach is_approved() unless enough of them independently choose
    /// to approve — at which point they could have deployed an honest
    /// instance themselves anyway. The signer list here is declarative
    /// metadata, not a claim of consent; the actual security property
    /// (only real signers can approve) lives entirely in approve()'s
    /// own require_auth().
    /// `approval_ttl` is fixed here and immutable afterward, same as
    /// `signers` and `threshold` — zero means approvals never expire
    /// (the original, still-default behavior); a nonzero value means an
    /// approval older than that many seconds stops counting toward
    /// `is_approved`, so a vote cast for a long-abandoned action can't
    /// silently still be sitting at threshold if that `action_id` is
    /// ever reused.
    /// `delays` (per-class timelocks) and `account` (optional custom-
    /// account mode) are likewise fixed here and immutable afterward.
    pub fn initialize(
        env: Env,
        signers: Vec<Address>,
        threshold: u32,
        approval_ttl: u64,
        delays: Delays,
        account: Option<AccountConfig>,
    ) {
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

        if delays.standard > delays.critical {
            panic_with_error!(&env, Error::InvalidDelays);
        }
        if let Some(account) = &account {
            if account.threshold == 0 || account.threshold > account.signers.len() {
                panic_with_error!(&env, Error::InvalidThreshold);
            }
            for i in 0..account.signers.len() {
                for j in (i + 1)..account.signers.len() {
                    if account.signers.get(i).unwrap() == account.signers.get(j).unwrap() {
                        panic_with_error!(&env, Error::DuplicateSigner);
                    }
                }
            }
            env.storage().instance().set(&DataKey::Account, account);
        }

        env.storage().instance().set(&DataKey::Delays, &delays);
        env.storage().instance().set(&DataKey::Signers, &signers);
        env.storage()
            .instance()
            .set(&DataKey::Threshold, &threshold);
        env.storage()
            .instance()
            .set(&DataKey::ApprovalTtl, &approval_ttl);
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

    pub fn get_approval_ttl(env: Env) -> u64 {
        env.storage().instance().get(&DataKey::ApprovalTtl).unwrap()
    }

    /// Records `signer`'s approval of `action_id`, stamped with the
    /// current ledger timestamp. Requires the signer's own signature and
    /// current membership in the fixed signer set. A signer whose
    /// previous approval of this `action_id` has already expired is
    /// treated the same as one who never approved — they can approve
    /// again, which simply refreshes the timestamp.
    pub fn approve(env: Env, signer: Address, action_id: u64) {
        signer.require_auth();

        let signers: Vec<Address> = env.storage().instance().get(&DataKey::Signers).unwrap();
        if !signers.contains(&signer) {
            panic_with_error!(&env, Error::NotASigner);
        }

        if Self::has_approved(env.clone(), action_id, signer.clone()) {
            panic_with_error!(&env, Error::AlreadyApproved);
        }
        let was_approved = Self::is_approved(env.clone(), action_id);
        env.storage().persistent().set(
            &DataKey::Approval(action_id, signer.clone()),
            &env.ledger().timestamp(),
        );
        let reached_key = DataKey::ReachedAt(action_id);
        if Self::is_approved(env.clone(), action_id)
            && (!was_approved || !env.storage().persistent().has(&reached_key))
        {
            env.storage()
                .persistent()
                .set(&reached_key, &env.ledger().timestamp());
        }
        events::approved(&env, signer, action_id);
    }

    /// Withdraws `signer`'s own approval of `action_id` — e.g. they
    /// approved before new information came in and want to reconsider.
    /// An already-expired approval has nothing left to withdraw, so this
    /// rejects it the same as a signer who never approved at all.
    pub fn revoke(env: Env, signer: Address, action_id: u64) {
        signer.require_auth();

        if !Self::has_approved(env.clone(), action_id, signer.clone()) {
            panic_with_error!(&env, Error::NotYetApproved);
        }
        env.storage()
            .persistent()
            .remove(&DataKey::Approval(action_id, signer.clone()));
        if !Self::is_approved(env.clone(), action_id) {
            env.storage()
                .persistent()
                .remove(&DataKey::ReachedAt(action_id));
        }
        events::revoked(&env, signer, action_id);
    }

    /// True only if `signer` approved `action_id` AND that approval
    /// hasn't expired under `approval_ttl` (zero ttl = never expires).
    pub fn has_approved(env: Env, action_id: u64, signer: Address) -> bool {
        let approved_at: Option<u64> = env
            .storage()
            .persistent()
            .get(&DataKey::Approval(action_id, signer));
        let Some(approved_at) = approved_at else {
            return false;
        };
        let ttl: u64 = env.storage().instance().get(&DataKey::ApprovalTtl).unwrap();
        if ttl == 0 {
            return true;
        }
        env.ledger()
            .timestamp()
            .checked_sub(approved_at)
            .unwrap_or(u64::MAX)
            <= ttl
    }

    /// Counts only currently-unexpired approvals — recomputed by
    /// checking every signer's own freshness rather than an incremental
    /// counter, since an approval can go stale purely from time passing,
    /// with no revoke() transaction to update a counter at the moment it
    /// happens.
    pub fn get_approval_count(env: Env, action_id: u64) -> u32 {
        let signers: Vec<Address> = env.storage().instance().get(&DataKey::Signers).unwrap();
        let mut count = 0u32;
        for signer in signers.iter() {
            if Self::has_approved(env.clone(), action_id, signer) {
                count += 1;
            }
        }
        count
    }

    pub fn is_approved(env: Env, action_id: u64) -> bool {
        let count = Self::get_approval_count(env.clone(), action_id);
        let threshold: u32 = env.storage().instance().get(&DataKey::Threshold).unwrap();
        count >= threshold
    }

    pub fn get_delays(env: Env) -> Delays {
        env.storage().instance().get(&DataKey::Delays).unwrap()
    }

    pub fn get_delay(env: Env, class: ActionClass) -> u64 {
        let delays = Self::get_delays(env);
        match class {
            ActionClass::Emergency => 0,
            ActionClass::Standard => delays.standard,
            ActionClass::Critical => delays.critical,
        }
    }

    /// When `action_id` last crossed the threshold, if it's currently
    /// tracked as having done so.
    pub fn get_threshold_reached_at(env: Env, action_id: u64) -> Option<u64> {
        env.storage()
            .persistent()
            .get(&DataKey::ReachedAt(action_id))
    }

    /// True once `action_id` is approved AND has stayed at threshold for
    /// at least `class`'s delay. Emergency-class actions only need to be
    /// approved. This is what consumers' `_via_multisig` entrypoints
    /// check, so signer majorities can't land a hostile change before
    /// users have time to exit.
    pub fn is_executable(env: Env, action_id: u64, class: ActionClass) -> bool {
        if !Self::is_approved(env.clone(), action_id) {
            return false;
        }
        let delay = Self::get_delay(env.clone(), class);
        if delay == 0 {
            return true;
        }
        match Self::get_threshold_reached_at(env.clone(), action_id) {
            Some(reached_at) => env.ledger().timestamp() >= reached_at.saturating_add(delay),
            None => false,
        }
    }

    pub fn get_account_config(env: Env) -> Option<AccountConfig> {
        env.storage().instance().get(&DataKey::Account)
    }

    /// Clears every signer's approval of `action_id` and resets its count
    /// to zero — for whoever executed the underlying action to call once
    /// it's done, so the same votes can't linger indefinitely and be
    /// silently reused if `action_id` is ever reused for a future action
    /// (e.g. a recurring "pause" request reusing the same id). Only
    /// callable once `action_id` is ALREADY approved — clearing an action
    /// that hasn't reached threshold yet would just be a way to grief
    /// other signers' pending votes for no reason, so this isn't open to
    /// just anyone at just any time.
    pub fn reset(env: Env, action_id: u64) {
        if !Self::is_approved(env.clone(), action_id) {
            panic_with_error!(&env, Error::NotYetApproved);
        }

        let signers: Vec<Address> = env.storage().instance().get(&DataKey::Signers).unwrap();
        for signer in signers.iter() {
            env.storage()
                .persistent()
                .remove(&DataKey::Approval(action_id, signer));
        }
        env.storage()
            .persistent()
            .remove(&DataKey::ReachedAt(action_id));
        events::reset(&env, action_id);
    }
}

#[contractimpl]
impl CustomAccountInterface for Multisig {
    type Signature = Vec<AccountSignature>;
    type Error = Error;

    /// Accepts the call if at least `threshold` distinct configured keys
    /// signed `signature_payload`. Signatures must be sorted strictly
    /// ascending by public key, which also rules out duplicates. If the
    /// account has an `allowed_contracts` policy, every authorized
    /// context must be a call into one of those contracts.
    #[allow(non_snake_case)]
    fn __check_auth(
        env: Env,
        signature_payload: Hash<32>,
        signatures: Vec<AccountSignature>,
        auth_contexts: Vec<Context>,
    ) -> Result<(), Error> {
        let account: AccountConfig = env
            .storage()
            .instance()
            .get(&DataKey::Account)
            .ok_or(Error::AccountNotConfigured)?;

        if signatures.len() < account.threshold {
            return Err(Error::InsufficientSignatures);
        }
        let payload = signature_payload.to_bytes().into();
        for i in 0..signatures.len() {
            let sig = signatures.get(i).unwrap();
            if i > 0 && signatures.get(i - 1).unwrap().public_key >= sig.public_key {
                return Err(Error::UnsortedSignatures);
            }
            if !account.signers.contains(&sig.public_key) {
                return Err(Error::NotASigner);
            }
            env.crypto()
                .ed25519_verify(&sig.public_key, &payload, &sig.signature);
        }

        if !account.allowed_contracts.is_empty() {
            for context in auth_contexts.iter() {
                let allowed = match context {
                    Context::Contract(c) => account.allowed_contracts.contains(&c.contract),
                    Context::CreateContractHostFn(_) => false,
                };
                if !allowed {
                    return Err(Error::ContextNotAllowed);
                }
            }
        }
        Ok(())
    }
}
