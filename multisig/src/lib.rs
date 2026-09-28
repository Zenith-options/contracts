#![no_std]

//! Zenith Multisig — M-of-N approval tracking, an on-chain action
//! registry, and a proposal executor.
//!
//! Two ways to use it:
//! - **Vote counter** (`approve` / `is_approved`): consumers' existing
//!   `_via_multisig` entrypoints check `is_approved(action_id)` for an
//!   opaque, caller-defined `BytesN<32>` id.
//! - **Executor** (`propose` / `approve` / `execute`): the multisig is the
//!   target contract's admin and, once approved, calls
//!   `target.function(args)` itself, satisfying `admin.require_auth()`.
//!
//! Every action gets an `ActionMeta` registry entry (proposer, creation
//! time, description hash, status), and pending ones are listed in a
//! bounded, paginated index so signers can see on-chain what's waiting.
//!
//! Signers and threshold are fixed at `initialize` and immutable —
//! changing the signer set means deploying a new Multisig. A generic
//! "vote to change your own signer set" mechanism was left out
//! deliberately to keep this contract's own trust model simple: there's
//! no in-protocol path for a compromised signer to add another
//! compromised signer.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, xdr::ToXdr, Address, BytesN, Env, Symbol, Val, Vec,
};

#[cfg(test)]
mod test;

mod error;
mod events;
mod types;

use error::Error;
use types::{ActionMeta, ActionStatus, DataKey, Proposal};

/// Hard cap on the pending-actions index as a whole.
pub const MAX_PENDING_ACTIONS: u32 = 100;
/// Per-signer cap on registered-but-unfinished actions, so one signer
/// can't spam the registry and crowd everyone else out.
pub const MAX_PENDING_PER_SIGNER: u32 = 10;
/// Largest page `get_pending_actions` returns.
pub const MAX_PAGE_LIMIT: u32 = 50;
/// Bounds a proposal's stored argument list.
pub const MAX_PROPOSAL_ARGS: u32 = 10;

const DAY_IN_LEDGERS: u32 = 17_280;
const TTL_THRESHOLD: u32 = 7 * DAY_IN_LEDGERS;
const TTL_EXTEND_TO: u32 = 30 * DAY_IN_LEDGERS;

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
    pub fn initialize(env: Env, signers: Vec<Address>, threshold: u32, approval_ttl: u64) {
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
    ///
    /// The first approval of an unregistered `action_id` registers it
    /// (proposer = this signer, zero description hash). An action that
    /// was already executed can never be approved again.
    pub fn approve(env: Env, signer: Address, action_id: BytesN<32>) {
        signer.require_auth();
        Self::require_signer(&env, &signer);

        let meta = match Self::get_action(env.clone(), action_id.clone()) {
            Some(meta) if meta.status == ActionStatus::Pending => meta,
            Some(meta) if meta.status == ActionStatus::Executed => {
                panic_with_error!(&env, Error::ActionNotPending)
            }
            _ => Self::register(
                &env,
                &signer,
                &action_id,
                &BytesN::from_array(&env, &[0; 32]),
            ),
        };

        if Self::has_approved(env.clone(), action_id.clone(), signer.clone()) {
            panic_with_error!(&env, Error::AlreadyApproved);
        }
        env.storage().persistent().set(
            &DataKey::Approval(action_id.clone(), signer.clone()),
            &env.ledger().timestamp(),
        );
        events::approved(&env, signer, action_id, meta.description_hash);
    }

    /// Withdraws `signer`'s own approval of `action_id` — e.g. they
    /// approved before new information came in and want to reconsider.
    /// An already-expired approval has nothing left to withdraw, so this
    /// rejects it the same as a signer who never approved at all.
    pub fn revoke(env: Env, signer: Address, action_id: BytesN<32>) {
        signer.require_auth();

        if !Self::has_approved(env.clone(), action_id.clone(), signer.clone()) {
            panic_with_error!(&env, Error::NotYetApproved);
        }
        env.storage()
            .persistent()
            .remove(&DataKey::Approval(action_id.clone(), signer.clone()));
        let hash = Self::description_hash(&env, &action_id);
        events::revoked(&env, signer, action_id, hash);
    }

    /// True only if `signer` approved `action_id` AND that approval
    /// hasn't expired under `approval_ttl` (zero ttl = never expires).
    pub fn has_approved(env: Env, action_id: BytesN<32>, signer: Address) -> bool {
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
    pub fn get_approval_count(env: Env, action_id: BytesN<32>) -> u32 {
        let signers: Vec<Address> = env.storage().instance().get(&DataKey::Signers).unwrap();
        let mut count = 0u32;
        for signer in signers.iter() {
            if Self::has_approved(env.clone(), action_id.clone(), signer) {
                count += 1;
            }
        }
        count
    }

    pub fn is_approved(env: Env, action_id: BytesN<32>) -> bool {
        let count = Self::get_approval_count(env.clone(), action_id);
        let threshold: u32 = env.storage().instance().get(&DataKey::Threshold).unwrap();
        count >= threshold
    }

    /// Clears every signer's approval of `action_id` and resets its count
    /// to zero — for whoever executed the underlying action to call once
    /// it's done, so the same votes can't linger indefinitely and be
    /// silently reused if `action_id` is ever reused for a future action
    /// (e.g. a recurring "pause" request reusing the same id). Only
    /// callable once `action_id` is ALREADY approved — clearing an action
    /// that hasn't reached threshold yet would just be a way to grief
    /// other signers' pending votes for no reason, so this isn't open to
    /// just anyone at just any time. Also drops it from the pending index.
    pub fn reset(env: Env, action_id: BytesN<32>) {
        if !Self::is_approved(env.clone(), action_id.clone()) {
            panic_with_error!(&env, Error::NotYetApproved);
        }
        if env
            .storage()
            .persistent()
            .has(&DataKey::Proposal(action_id.clone()))
        {
            // Proposals leave the registry only through execute().
            panic_with_error!(&env, Error::ActionNotPending);
        }
        Self::clear_approvals(&env, &action_id);
        let hash = Self::finish(&env, &action_id, ActionStatus::Reset);
        events::reset(&env, action_id, hash);
    }

    // ── Registry ──────────────────────────────────────────────────────────────

    /// Explicitly registers `action_id` with a `description_hash` (e.g.
    /// sha256 of an off-chain write-up) before anyone votes on it, so
    /// signers can check on-chain what they're approving.
    pub fn register_action(
        env: Env,
        proposer: Address,
        action_id: BytesN<32>,
        description_hash: BytesN<32>,
    ) {
        proposer.require_auth();
        Self::require_signer(&env, &proposer);
        if let Some(meta) = Self::get_action(env.clone(), action_id.clone()) {
            if matches!(meta.status, ActionStatus::Pending | ActionStatus::Executed) {
                panic_with_error!(&env, Error::ActionAlreadyRegistered);
            }
        }
        Self::register(&env, &proposer, &action_id, &description_hash);
    }

    /// Drops a pending action from the index once it has gone stale: an
    /// `approval_ttl` is configured, the action is older than it, and no
    /// approval of it is still fresh. Callable by anyone.
    pub fn expire(env: Env, action_id: BytesN<32>) {
        let meta = Self::get_action(env.clone(), action_id.clone())
            .unwrap_or_else(|| panic_with_error!(&env, Error::ActionNotPending));
        if meta.status != ActionStatus::Pending {
            panic_with_error!(&env, Error::ActionNotPending);
        }
        let ttl: u64 = env.storage().instance().get(&DataKey::ApprovalTtl).unwrap();
        let age = env.ledger().timestamp().saturating_sub(meta.created_at);
        if ttl == 0 || age <= ttl || Self::get_approval_count(env.clone(), action_id.clone()) > 0 {
            panic_with_error!(&env, Error::NotExpired);
        }
        Self::clear_approvals(&env, &action_id);
        let hash = Self::finish(&env, &action_id, ActionStatus::Expired);
        events::expired(&env, action_id, hash);
    }

    pub fn get_action(env: Env, action_id: BytesN<32>) -> Option<ActionMeta> {
        env.storage().persistent().get(&DataKey::Action(action_id))
    }

    pub fn get_pending_count(env: Env) -> u32 {
        Self::pending(&env).len()
    }

    /// Up to `limit` (≤ MAX_PAGE_LIMIT) pending action ids starting at
    /// index `cursor`. Removal is swap-remove, so order isn't stable
    /// across writes.
    pub fn get_pending_actions(env: Env, cursor: u32, limit: u32) -> Vec<BytesN<32>> {
        if limit == 0 || limit > MAX_PAGE_LIMIT {
            panic_with_error!(&env, Error::InvalidPageLimit);
        }
        let pending = Self::pending(&env);
        let mut page = Vec::new(&env);
        let end = cursor.saturating_add(limit).min(pending.len());
        for i in cursor..end {
            page.push_back(pending.get(i).unwrap());
        }
        page
    }

    // ── Executor ──────────────────────────────────────────────────────────────

    /// Registers a call the multisig will make as itself once approved:
    /// `target.function(args)`. The returned id is the sha256 of the
    /// proposal's XDR (including a fresh nonce) and is also its
    /// description hash, so the id commits to the exact payload.
    pub fn propose(
        env: Env,
        proposer: Address,
        target: Address,
        function: Symbol,
        args: Vec<Val>,
    ) -> BytesN<32> {
        proposer.require_auth();
        Self::require_signer(&env, &proposer);
        if args.len() > MAX_PROPOSAL_ARGS {
            panic_with_error!(&env, Error::ProposalTooLarge);
        }

        let nonce: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ProposalNonce)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::ProposalNonce, &(nonce + 1));

        let proposal = Proposal {
            target,
            function,
            args,
            nonce,
        };
        let id: BytesN<32> = env.crypto().sha256(&proposal.clone().to_xdr(&env)).into();
        let key = DataKey::Proposal(id.clone());
        env.storage().persistent().set(&key, &proposal);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
        Self::register(&env, &proposer, &id, &id);
        id
    }

    /// Runs an approved proposal: `target.function(args)` invoked by this
    /// contract, which satisfies a target's `admin.require_auth()` when
    /// the multisig is that admin. Callable by anyone. Marked executed
    /// before the call, so it can never run twice; a failing target call
    /// reverts the whole transaction, including that mark.
    pub fn execute(env: Env, proposal_id: BytesN<32>) -> Val {
        let proposal: Proposal = env
            .storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id.clone()))
            .unwrap_or_else(|| panic_with_error!(&env, Error::ProposalNotFound));
        let pending = Self::get_action(env.clone(), proposal_id.clone())
            .map(|m| m.status == ActionStatus::Pending)
            .unwrap_or(false);
        if !pending {
            panic_with_error!(&env, Error::ActionNotPending);
        }
        if !Self::is_approved(env.clone(), proposal_id.clone()) {
            panic_with_error!(&env, Error::NotYetApproved);
        }

        Self::clear_approvals(&env, &proposal_id);
        let hash = Self::finish(&env, &proposal_id, ActionStatus::Executed);
        events::executed(&env, proposal_id, hash);

        env.invoke_contract::<Val>(&proposal.target, &proposal.function, proposal.args)
    }

    pub fn get_proposal(env: Env, proposal_id: BytesN<32>) -> Option<Proposal> {
        env.storage()
            .persistent()
            .get(&DataKey::Proposal(proposal_id))
    }
}

impl Multisig {
    fn require_signer(env: &Env, address: &Address) {
        let signers: Vec<Address> = env.storage().instance().get(&DataKey::Signers).unwrap();
        if !signers.contains(address) {
            panic_with_error!(env, Error::NotASigner);
        }
    }

    fn pending(env: &Env) -> Vec<BytesN<32>> {
        env.storage()
            .persistent()
            .get(&DataKey::Pending)
            .unwrap_or_else(|| Vec::new(env))
    }

    fn description_hash(env: &Env, action_id: &BytesN<32>) -> BytesN<32> {
        Self::get_action(env.clone(), action_id.clone())
            .map(|m| m.description_hash)
            .unwrap_or_else(|| BytesN::from_array(env, &[0; 32]))
    }

    fn register(
        env: &Env,
        proposer: &Address,
        action_id: &BytesN<32>,
        description_hash: &BytesN<32>,
    ) -> ActionMeta {
        let mut pending = Self::pending(env);
        if pending.len() >= MAX_PENDING_ACTIONS {
            panic_with_error!(env, Error::TooManyPendingActions);
        }
        let count_key = DataKey::PendingCount(proposer.clone());
        let count: u32 = env.storage().persistent().get(&count_key).unwrap_or(0);
        if count >= MAX_PENDING_PER_SIGNER {
            panic_with_error!(env, Error::TooManyPendingActions);
        }
        env.storage().persistent().set(&count_key, &(count + 1));
        pending.push_back(action_id.clone());
        env.storage().persistent().set(&DataKey::Pending, &pending);

        let meta = ActionMeta {
            proposer: proposer.clone(),
            created_at: env.ledger().timestamp(),
            description_hash: description_hash.clone(),
            status: ActionStatus::Pending,
        };
        let key = DataKey::Action(action_id.clone());
        env.storage().persistent().set(&key, &meta);
        env.storage()
            .persistent()
            .extend_ttl(&key, TTL_THRESHOLD, TTL_EXTEND_TO);
        events::registered(
            env,
            proposer.clone(),
            action_id.clone(),
            description_hash.clone(),
        );
        meta
    }

    /// Moves a pending action to its final `status`, swap-removing it
    /// from the index. Returns its description hash for events. Actions
    /// approved before the registry existed have no meta; for those this
    /// is a no-op beyond returning a zero hash.
    fn finish(env: &Env, action_id: &BytesN<32>, status: ActionStatus) -> BytesN<32> {
        let Some(mut meta) = Self::get_action(env.clone(), action_id.clone()) else {
            return BytesN::from_array(env, &[0; 32]);
        };
        if meta.status == ActionStatus::Pending {
            let mut pending = Self::pending(env);
            if let Some(i) = pending.first_index_of(action_id) {
                let last = pending.len() - 1;
                if i != last {
                    pending.set(i, pending.get(last).unwrap());
                }
                pending.pop_back();
                env.storage().persistent().set(&DataKey::Pending, &pending);
            }
            let count_key = DataKey::PendingCount(meta.proposer.clone());
            let count: u32 = env.storage().persistent().get(&count_key).unwrap_or(0);
            env.storage()
                .persistent()
                .set(&count_key, &count.saturating_sub(1));
        }
        meta.status = status;
        env.storage()
            .persistent()
            .set(&DataKey::Action(action_id.clone()), &meta);
        meta.description_hash
    }

    fn clear_approvals(env: &Env, action_id: &BytesN<32>) {
        let signers: Vec<Address> = env.storage().instance().get(&DataKey::Signers).unwrap();
        for signer in signers.iter() {
            env.storage()
                .persistent()
                .remove(&DataKey::Approval(action_id.clone(), signer));
        }
    }
}
