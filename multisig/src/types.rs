use soroban_sdk::{contracttype, Address};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Signers,
    Threshold,
    /// Whether `signer` has approved `action_id`. `action_id` is entirely
    /// caller-defined — this contract never interprets what the action
    /// actually does, only how many of the fixed signer set have signed
    /// off on it.
    Approval(u64, Address),
    ApprovalCount(u64),
}
