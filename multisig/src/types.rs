use soroban_sdk::{contracttype, Address};

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// O(1) membership check — see issue #101.
    IsSigner(Address),
    /// Enumeration only (e.g. listing configured signers); persistent
    /// storage instead of instance storage, same reasoning as `IsSigner`.
    Signers,
}
