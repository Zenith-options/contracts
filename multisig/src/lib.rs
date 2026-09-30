#![no_std]

//! Zenith Multisig — weighted approval tracking, an on-chain action
//! registry, a proposal executor, and governed signer rotation.
//!
//! Two ways to use it:
//! - **Vote counter** (`approve` / `is_approved`): consumers' existing
//!   `_via_multisig` entrypoints check `is_approved(action_id)` for an
//!   opaque, caller-defined `BytesN<32>` id.
//! - **Executor** (`propose` / `approve` / `execute`): the multisig is the
//!   target contract's admin and, once approved, calls
//!   `target.function(args)` itself, satisfying `admin.require_auth()`.
//!
//! Every signer carries a weight, and an action is approved once the
//! summed weight of its fresh approvals reaches `threshold`. The legacy
//! `initialize` gives every signer weight 1, which is plain M-of-N.
//!
//! Every action gets an `ActionMeta` registry entry (proposer, creation
//! time, description hash, status), and pending ones are listed in a
//! bounded, paginated index so signers can see on-chain what's waiting.
//!
//! The signer set can only change through `propose_signer_change`, which
//! needs the higher `rotation_threshold`, then waits out a mandatory
//! `rotation_delay` during which any single signer can veto. Every
//! executed change bumps the signer-set epoch, which invalidates every
//! outstanding approval.

use soroban_sdk::{
    contract, contractimpl, panic_with_error, xdr::ToXdr, Address, BytesN, Env, Map, Symbol, Val,
    Vec,
};

#[cfg(test)]
mod test;

mod error;
mod events;
mod ttl;
mod types;

use error::Error;
use types::{ActionMeta, ActionStatus, DataKey, Proposal, SignerChange, SignerWeight};

/// Hard cap on the pending-actions index as a whole.
pub const MAX_PENDING_ACTIONS: u32 = 100;
/// Per-signer cap on registered-but-unfinished actions, so one signer
/// can't spam the registry and crowd everyone else out.
pub const MAX_PENDING_PER_SIGNER: u32 = 10;
/// Largest page `get_pending_actions` returns.
pub const MAX_PAGE_LIMIT: u32 = 50;
/// Bounds a proposal's stored argument list.
pub const MAX_PROPOSAL_ARGS: u32 = 10;
/// Rotation delay the legacy equal-weight `initialize` uses.
pub const DEFAULT_ROTATION_DELAY: u64 = 3 * 24 * 60 * 60;

const DAY_IN_LEDGERS: u32 = 17_280;
const TTL_THRESHOLD: u32 = 7 * DAY_IN_LEDGERS;
const TTL_EXTEND_TO: u32 = 30 * DAY_IN_LEDGERS;

#[contract]
pub struct Multisig;

#[contractimpl]
impl Multisig {
    /// Permissionless keeper entrypoint: extends the contract instance and
    /// every named persistent entry that exists, per the TTL policy in
    /// ttl.rs. Anyone may pay the rent to keep long-lived entries alive.
    pub fn bump(env: Env, keys: Vec<DataKey>) {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
    }

    /// Deliberately does NOT call require_auth() on any signer, unlike
    /// every other contract's initialize() here requiring its incoming
    /// admin's signature. Naming an address as a signer costs an
    /// attacker nothing without that address's cooperation: approve()
    /// still requires the REAL signer's own signature, so a Multisig
    /// initialized with signers who never consented can simply never
    /// reach is_approved() unless enough of them independently choose
    /// to approve — at which point they could have deployed an honest
    /// instance themselves anyway.
    ///
    /// Backward-compatible equal-weight constructor: every signer gets
    /// weight 1, so `threshold` is a plain M-of-N count. The rotation
    /// threshold is `threshold + 1` (or unanimity when `threshold` is
    /// already every signer) with `DEFAULT_ROTATION_DELAY`.
    ///
    /// `approval_ttl` of zero means approvals never expire; a nonzero
    /// value means an approval older than that many seconds stops
    /// counting toward `is_approved`.
    pub fn initialize(env: Env, signers: Vec<Address>, threshold: u32, approval_ttl: u64) {
        let mut weighted = Vec::new(&env);
        for signer in signers.iter() {
            weighted.push_back((signer, 1u32));
        }
        let rotation_threshold = if threshold >= signers.len() {
            signers.len()
        } else {
            threshold + 1
        };
        Self::initialize_weighted(
            env,
            weighted,
            threshold,
            approval_ttl,
            rotation_threshold,
            DEFAULT_ROTATION_DELAY,
        );
    }

    /// Weighted constructor. Every weight must be above zero, the summed
    /// weight must fit in a `u32` and reach `threshold_weight`, and the
    /// `rotation_threshold` must satisfy the rotation invariant (see
    /// `validate_thresholds`).
    ///
    /// Policy for a signer whose weight alone meets `threshold_weight`:
    /// allowed (e.g. a cold key that can act solo), but announced with a
    /// `single_key_quorum` event so it's visible on-chain.
    pub fn initialize_weighted(
        env: Env,
        signers: Vec<(Address, u32)>,
        threshold_weight: u32,
        approval_ttl: u64,
        rotation_threshold: u32,
        rotation_delay: u64,
    ) {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Signers) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        let mut map: Map<Address, u32> = Map::new(&env);
        for (signer, weight) in signers.iter() {
            if weight == 0 {
                panic_with_error!(&env, Error::InvalidWeight);
            }
            if map.contains_key(signer.clone()) {
                panic_with_error!(&env, Error::DuplicateSigner);
            }
            map.set(signer, weight);
        }
        Self::validate_thresholds(&env, &map, threshold_weight, rotation_threshold);

        env.storage().instance().set(&DataKey::Signers, &map);
        env.storage()
            .instance()
            .set(&DataKey::Threshold, &threshold_weight);
        env.storage()
            .instance()
            .set(&DataKey::ApprovalTtl, &approval_ttl);
        env.storage()
            .instance()
            .set(&DataKey::RotationThreshold, &rotation_threshold);
        env.storage()
            .instance()
            .set(&DataKey::RotationDelay, &rotation_delay);
        env.storage().instance().set(&DataKey::SignerEpoch, &0u32);
        Self::warn_single_key_quorum(&env, &map, threshold_weight);
    }

    pub fn is_signer(env: Env, address: Address) -> bool {
        ttl::extend_instance(&env);
        Self::signers(&env).contains_key(address)
    }

    /// Weight of `address`, or zero if it isn't a signer.
    pub fn get_signer_weight(env: Env, address: Address) -> u32 {
        ttl::extend_instance(&env);
        Self::signers(&env).get(address).unwrap_or(0)
    }

    pub fn get_signers(env: Env) -> Map<Address, u32> {
        ttl::extend_instance(&env);
        Self::signers(&env)
    }

    pub fn get_signer_count(env: Env) -> u32 {
        ttl::extend_instance(&env);
        Self::signers(&env).len()
    }

    pub fn get_total_weight(env: Env) -> u32 {
        ttl::extend_instance(&env);
        Self::total_weight(&env, &Self::signers(&env))
    }

    /// Approval weight `is_approved` requires.
    pub fn get_threshold(env: Env) -> u32 {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::Threshold).unwrap()
    }

    pub fn get_rotation_threshold(env: Env) -> u32 {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::RotationThreshold)
            .unwrap()
    }

    pub fn get_rotation_delay(env: Env) -> u64 {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::RotationDelay)
            .unwrap()
    }

    pub fn get_signer_epoch(env: Env) -> u32 {
        ttl::extend_instance(&env);
        Self::epoch(&env)
    }

    pub fn get_approval_ttl(env: Env) -> u64 {
        ttl::extend_instance(&env);
        env.storage().instance().get(&DataKey::ApprovalTtl).unwrap()
    }

    /// Records `signer`'s approval of `action_id`, stamped with the
    /// current ledger timestamp. Requires the signer's own signature and
    /// current membership in the signer set. A signer whose previous
    /// approval of this `action_id` has already expired is treated the
    /// same as one who never approved — they can approve again, which
    /// simply refreshes the timestamp.
    ///
    /// The first approval of an unregistered `action_id` registers it
    /// (proposer = this signer, zero description hash). An action that
    /// was already executed can never be approved again.
    pub fn approve(env: Env, signer: Address, action_id: BytesN<32>) {
        ttl::extend_instance(&env);
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
        ttl::set_persistent(
            &env,
            &DataKey::Approval(Self::epoch(&env), action_id.clone(), signer.clone()),
            &env.ledger().timestamp(),
        );
        events::approved(&env, signer, action_id, meta.description_hash);
    }

    /// Withdraws `signer`'s own approval of `action_id` — e.g. they
    /// approved before new information came in and want to reconsider.
    /// An already-expired approval has nothing left to withdraw, so this
    /// rejects it the same as a signer who never approved at all.
    pub fn revoke(env: Env, signer: Address, action_id: BytesN<32>) {
        ttl::extend_instance(&env);
        signer.require_auth();

        if !Self::has_approved(env.clone(), action_id.clone(), signer.clone()) {
            panic_with_error!(&env, Error::NotYetApproved);
        }
        env.storage().persistent().remove(&DataKey::Approval(
            Self::epoch(&env),
            action_id.clone(),
            signer.clone(),
        ));
        let hash = Self::description_hash(&env, &action_id);
        events::revoked(&env, signer, action_id, hash);
    }

    /// True only if `signer` approved `action_id` in the current
    /// signer-set epoch AND that approval hasn't expired under
    /// `approval_ttl` (zero ttl = never expires).
    pub fn has_approved(env: Env, action_id: BytesN<32>, signer: Address) -> bool {
        ttl::extend_instance(&env);
        let approved_at: Option<u64> = ttl::get_persistent(
            &env,
            &DataKey::Approval(Self::epoch(&env), action_id, signer),
        );
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

    /// Number of signers with a fresh approval of `action_id`, regardless
    /// of their weight.
    pub fn get_approval_count(env: Env, action_id: BytesN<32>) -> u32 {
        ttl::extend_instance(&env);
        let mut count = 0u32;
        for (signer, _) in Self::signers(&env).iter() {
            if Self::has_approved(env.clone(), action_id.clone(), signer) {
                count += 1;
            }
        }
        count
    }

    /// Summed weight of every fresh approval of `action_id` — recomputed
    /// from each signer's freshness rather than an incremental counter,
    /// since an approval can go stale purely from time passing.
    pub fn get_approval_weight(env: Env, action_id: BytesN<32>) -> u32 {
        ttl::extend_instance(&env);
        let mut weight = 0u32;
        for (signer, w) in Self::signers(&env).iter() {
            if Self::has_approved(env.clone(), action_id.clone(), signer) {
                // Bounded by the total weight, which initialize and every
                // rotation checked fits in a u32.
                weight += w;
            }
        }
        weight
    }

    pub fn is_approved(env: Env, action_id: BytesN<32>) -> bool {
        ttl::extend_instance(&env);
        let weight = Self::get_approval_weight(env.clone(), action_id);
        let threshold: u32 = env.storage().instance().get(&DataKey::Threshold).unwrap();
        weight >= threshold
    }

    /// Clears every signer's approval of `action_id` — for whoever
    /// executed the underlying action to call once it's done, so the
    /// same votes can't linger and be silently reused if `action_id` is
    /// ever reused. Only callable once `action_id` is ALREADY approved,
    /// so it can't be used to grief other signers' pending votes. Also
    /// drops it from the pending index.
    pub fn reset(env: Env, action_id: BytesN<32>) {
        if !Self::is_approved(env.clone(), action_id.clone()) {
            panic_with_error!(&env, Error::NotYetApproved);
        }
        let storage = env.storage().persistent();
        if storage.has(&DataKey::Proposal(action_id.clone()))
            || storage.has(&DataKey::SignerChange(action_id.clone()))
        {
            // Proposals and signer changes leave the registry only
            // through their own execute/veto paths.
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

        let proposal = Proposal {
            target,
            function,
            args,
            nonce: Self::next_nonce(&env),
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
        Self::require_pending(&env, &proposal_id);
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

    // ── Signer rotation ───────────────────────────────────────────────────────

    /// Proposes adding at most one signer and removing at most one (each
    /// list holds zero or one entries), plus
    /// the new approval and rotation thresholds. Passing the same address
    /// as both `add` and `remove` changes that signer's weight. The
    /// returned id is the sha256 of the change's XDR, so approvals bind
    /// to the exact payload. Validated now against the resulting set,
    /// and again at execution.
    ///
    /// Lifecycle: signers `approve(id)` until the approval weight reaches
    /// `rotation_threshold`; anyone calls `queue_signer_change`, starting
    /// the `rotation_delay`; once it elapses anyone calls
    /// `execute_signer_change`. Any single signer can
    /// `veto_signer_change` until it executes.
    pub fn propose_signer_change(
        env: Env,
        proposer: Address,
        add: Vec<SignerWeight>,
        remove: Vec<Address>,
        new_threshold: u32,
        new_rotation_threshold: u32,
    ) -> BytesN<32> {
        ttl::extend_instance(&env);
        proposer.require_auth();
        Self::require_signer(&env, &proposer);

        let change = SignerChange {
            add,
            remove,
            new_threshold,
            new_rotation_threshold,
            epoch: Self::epoch(&env),
            nonce: Self::next_nonce(&env),
        };
        Self::apply_change(&env, &change);

        let id: BytesN<32> = env.crypto().sha256(&change.clone().to_xdr(&env)).into();
        ttl::set_persistent(&env, &DataKey::SignerChange(id.clone()), &change);
        Self::register(&env, &proposer, &id, &id);
        events::signer_change_proposed(&env, proposer, id.clone(), change);
        id
    }

    /// Starts the mandatory delay once `change_id`'s approval weight has
    /// reached `rotation_threshold`. Callable by anyone.
    pub fn queue_signer_change(env: Env, change_id: BytesN<32>) -> u64 {
        ttl::extend_instance(&env);
        Self::require_live_change(&env, &change_id);
        let ready_key = DataKey::SignerChangeReadyAt(change_id.clone());
        if env.storage().persistent().has(&ready_key) {
            panic_with_error!(&env, Error::ActionAlreadyRegistered);
        }
        Self::require_rotation_approved(&env, &change_id);
        let delay: u64 = env
            .storage()
            .instance()
            .get(&DataKey::RotationDelay)
            .unwrap();
        let ready_at = env.ledger().timestamp().saturating_add(delay);
        ttl::set_persistent(&env, &ready_key, &ready_at);
        events::signer_change_queued(&env, change_id, ready_at);
        ready_at
    }

    /// Any single signer can kill a pending signer change, before or
    /// during its delay.
    pub fn veto_signer_change(env: Env, signer: Address, change_id: BytesN<32>) {
        ttl::extend_instance(&env);
        signer.require_auth();
        Self::require_signer(&env, &signer);
        Self::require_live_change(&env, &change_id);
        Self::clear_approvals(&env, &change_id);
        Self::finish(&env, &change_id, ActionStatus::Reset);
        events::signer_change_vetoed(&env, signer, change_id);
    }

    /// Applies a queued signer change once its delay has elapsed, as long
    /// as its approval weight still meets `rotation_threshold`. Bumps the
    /// signer-set epoch, which invalidates every outstanding approval
    /// (including those of a removed signer). Callable by anyone.
    pub fn execute_signer_change(env: Env, change_id: BytesN<32>) {
        ttl::extend_instance(&env);
        let change = Self::require_live_change(&env, &change_id);
        let ready_at: u64 =
            ttl::get_persistent(&env, &DataKey::SignerChangeReadyAt(change_id.clone()))
                .unwrap_or_else(|| panic_with_error!(&env, Error::SignerChangeNotQueued));
        if env.ledger().timestamp() < ready_at {
            panic_with_error!(&env, Error::RotationDelayActive);
        }
        Self::require_rotation_approved(&env, &change_id);

        let signers = Self::apply_change(&env, &change);
        Self::clear_approvals(&env, &change_id);
        Self::finish(&env, &change_id, ActionStatus::Executed);

        let epoch = change.epoch + 1;
        let storage = env.storage().instance();
        storage.set(&DataKey::Signers, &signers);
        storage.set(&DataKey::Threshold, &change.new_threshold);
        storage.set(&DataKey::RotationThreshold, &change.new_rotation_threshold);
        storage.set(&DataKey::SignerEpoch, &epoch);
        Self::warn_single_key_quorum(&env, &signers, change.new_threshold);
        events::signers_rotated(&env, change_id, epoch);
    }

    pub fn get_signer_change(env: Env, change_id: BytesN<32>) -> Option<SignerChange> {
        ttl::get_persistent(&env, &DataKey::SignerChange(change_id))
    }

    /// Timestamp at which a queued change becomes executable.
    pub fn get_signer_change_ready_at(env: Env, change_id: BytesN<32>) -> Option<u64> {
        ttl::get_persistent(&env, &DataKey::SignerChangeReadyAt(change_id))
    }
}

impl Multisig {
    fn signers(env: &Env) -> Map<Address, u32> {
        env.storage().instance().get(&DataKey::Signers).unwrap()
    }

    fn epoch(env: &Env) -> u32 {
        env.storage()
            .instance()
            .get(&DataKey::SignerEpoch)
            .unwrap_or(0)
    }

    fn next_nonce(env: &Env) -> u64 {
        let nonce: u64 = env
            .storage()
            .instance()
            .get(&DataKey::ProposalNonce)
            .unwrap_or(0);
        env.storage()
            .instance()
            .set(&DataKey::ProposalNonce, &(nonce + 1));
        nonce
    }

    /// Overflow-safe sum of every signer's weight.
    fn total_weight(env: &Env, signers: &Map<Address, u32>) -> u32 {
        let mut total = 0u32;
        for (_, weight) in signers.iter() {
            total = total
                .checked_add(weight)
                .unwrap_or_else(|| panic_with_error!(env, Error::WeightOverflow));
        }
        total
    }

    /// Invariants for any signer set: non-empty, `1 <= threshold <=
    /// total weight`, and a rotation threshold strictly above
    /// `threshold` — or, when `threshold` is already the total weight,
    /// equal to it (unanimity) — that is still reachable.
    fn validate_thresholds(
        env: &Env,
        signers: &Map<Address, u32>,
        threshold: u32,
        rotation_threshold: u32,
    ) {
        let total = Self::total_weight(env, signers);
        if signers.is_empty() || threshold == 0 || threshold > total {
            panic_with_error!(env, Error::InvalidThreshold);
        }
        let above = rotation_threshold > threshold;
        let unanimous = rotation_threshold == total;
        if rotation_threshold > total || !(above || unanimous) {
            panic_with_error!(env, Error::InvalidThreshold);
        }
    }

    /// Returns the signer set `change` would produce, panicking on any
    /// invariant violation.
    fn apply_change(env: &Env, change: &SignerChange) -> Map<Address, u32> {
        if change.add.len() > 1 || change.remove.len() > 1 {
            panic_with_error!(env, Error::InvalidSignerChange);
        }
        let mut signers = Self::signers(env);
        if let Some(remove) = change.remove.first() {
            if !signers.contains_key(remove.clone()) {
                panic_with_error!(env, Error::NotASigner);
            }
            signers.remove(remove);
        }
        if let Some(add) = change.add.first() {
            if add.weight == 0 {
                panic_with_error!(env, Error::InvalidWeight);
            }
            if signers.contains_key(add.address.clone()) {
                panic_with_error!(env, Error::DuplicateSigner);
            }
            signers.set(add.address, add.weight);
        }
        Self::validate_thresholds(
            env,
            &signers,
            change.new_threshold,
            change.new_rotation_threshold,
        );
        signers
    }

    fn require_live_change(env: &Env, change_id: &BytesN<32>) -> SignerChange {
        let change: SignerChange =
            ttl::get_persistent(env, &DataKey::SignerChange(change_id.clone()))
                .unwrap_or_else(|| panic_with_error!(env, Error::SignerChangeNotFound));
        Self::require_pending(env, change_id);
        if change.epoch != Self::epoch(env) {
            panic_with_error!(env, Error::StaleSignerChange);
        }
        change
    }

    fn require_rotation_approved(env: &Env, change_id: &BytesN<32>) {
        let weight = Self::get_approval_weight(env.clone(), change_id.clone());
        let needed: u32 = env
            .storage()
            .instance()
            .get(&DataKey::RotationThreshold)
            .unwrap();
        if weight < needed {
            panic_with_error!(env, Error::NotYetApproved);
        }
    }

    fn warn_single_key_quorum(env: &Env, signers: &Map<Address, u32>, threshold: u32) {
        if signers.len() < 2 {
            return;
        }
        for (signer, weight) in signers.iter() {
            if weight >= threshold {
                events::single_key_quorum(env, signer, weight, threshold);
            }
        }
    }

    fn require_signer(env: &Env, address: &Address) {
        if !Self::signers(env).contains_key(address.clone()) {
            panic_with_error!(env, Error::NotASigner);
        }
    }

    fn require_pending(env: &Env, action_id: &BytesN<32>) {
        let pending = Self::get_action(env.clone(), action_id.clone())
            .map(|m| m.status == ActionStatus::Pending)
            .unwrap_or(false);
        if !pending {
            panic_with_error!(env, Error::ActionNotPending);
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
    /// from the index. Returns its description hash for events.
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
        let epoch = Self::epoch(env);
        for (signer, _) in Self::signers(env).iter() {
            env.storage()
                .persistent()
                .remove(&DataKey::Approval(epoch, action_id.clone(), signer));
        }
    }
}
