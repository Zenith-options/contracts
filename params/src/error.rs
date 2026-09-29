use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    ParamNotFound = 2,
    ParamAlreadyDefined = 3,
    OutOfBounds = 4,
    InvalidBounds = 5,
    NoPendingBounds = 6,
    BoundsDelayNotElapsed = 7,
}
