use soroban_sdk::{contracttype, Address, BytesN, Symbol, Val, Vec};

/// One contract call inside a scheduled operation.
#[contracttype]
#[derive(Clone)]
pub struct Call {
    pub target: Address,
    pub function: Symbol,
    pub args: Vec<Val>,
}

#[contracttype]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    Proposer,
    Executor,
    Canceller,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    MinDelay,
    /// Anyone may execute a ready operation.
    OpenExecutor,
    Role(Role, Address),
    /// 0/absent = unset, 1 = done, otherwise the ledger timestamp at which
    /// the operation becomes executable.
    Timestamp(BytesN<32>),
}
