use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    Unauthorized = 2,
    InsufficientDelay = 3,
    AlreadyScheduled = 4,
    NotReady = 5,
    PredecessorNotDone = 6,
    NotPending = 7,
    UnknownSelfCall = 8,
    EmptyOperation = 9,
}
