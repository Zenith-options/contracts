use soroban_sdk::{contracttype, Address, Symbol, Val, Vec};

#[contracttype]
#[derive(Clone)]
pub struct Operation {
    pub targets: Vec<Address>,
    pub fns: Vec<Symbol>,
    pub args: Vec<Vec<Val>>,
    /// Ledger timestamp from which the operation may be executed.
    pub ready_at: u64,
    pub done: bool,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    /// The only address that can schedule and execute operations (the
    /// multisig during handover, the governor after).
    Proposer,
    /// Can cancel any pending operation and, until lock-in, reassign the
    /// proposer. Removed permanently by `lock_in`.
    Guardian,
    MinDelay,
    /// How long after `ready_at` an operation stays executable.
    GracePeriod,
    Operation(soroban_sdk::BytesN<32>),
}
