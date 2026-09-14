use soroban_sdk::{contracttype, Address};

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
}
