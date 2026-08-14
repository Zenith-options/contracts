use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    FeederAlreadyAdded = 2,
    FeederNotFound = 3,
    NotAFeeder = 4,
    InvalidPrice = 5,
    NoFreshPrice = 6,
    ContractPaused = 7,
    InvalidStaleness = 8,
    TooManyFeeders = 9,
}
