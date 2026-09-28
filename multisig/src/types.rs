use soroban_sdk::{contracttype, Address, BytesN, Vec};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Signers,
    Threshold,
    /// How long an approval counts for after it's recorded, in seconds.
    /// Fixed at initialize, immutable — same rationale as Signers and
    /// Threshold. Zero means approvals never expire.
    ApprovalTtl,
    /// The ledger timestamp at which `signer` approved `action_id`.
    /// Absent means never approved, or approved and then revoked.
    /// `action_id` is entirely caller-defined — this contract never
    /// interprets what the action actually does, only how many of the
    /// fixed signer set have signed off on it, and since when.
    Approval(u64, Address),
    /// Per-class timelock delays, fixed at initialize and immutable.
    Delays,
    /// The ledger timestamp at which `action_id` most recently crossed
    /// the threshold. Set by the approve() that crosses it, cleared by a
    /// revoke() that drops below it and by reset(). An approval that
    /// expires during the delay drops the action below threshold, and
    /// the approve() that restores it restarts the timer.
    ReachedAt(u64),
    /// Custom-account (`__check_auth`) configuration, if any.
    Account,
}

/// Timelock tier of an action, declared by the consumer for each of its
/// `_via_multisig` functions and checked by `is_executable`.
#[contracttype]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum ActionClass {
    /// Exempt from any delay (e.g. pause).
    Emergency = 0,
    /// Routine parameter changes (e.g. set_fee_rate).
    Standard = 1,
    /// Changes that can take control or funds (upgrade, transfer_admin).
    Critical = 2,
}

/// Seconds between an action reaching threshold and becoming executable,
/// per class. Emergency is always zero.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Delays {
    pub standard: u64,
    pub critical: u64,
}

/// Configuration for using this contract as a Soroban custom account:
/// M-of-N ed25519 keys authorize calls through `__check_auth`.
#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountConfig {
    pub signers: Vec<BytesN<32>>,
    pub threshold: u32,
    /// If non-empty, the account only authorizes calls into these
    /// contracts (and never contract creation).
    pub allowed_contracts: Vec<Address>,
}

#[contracttype]
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AccountSignature {
    pub public_key: BytesN<32>,
    pub signature: BytesN<64>,
}
