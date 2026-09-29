use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    Unauthorized = 2,
    InvalidOperation = 3,
    DelayTooShort = 4,
    OperationExists = 5,
    OperationNotFound = 6,
    NotReady = 7,
    Expired = 8,
    AlreadyDone = 9,
    LockedIn = 10,
    UnknownSelfCall = 11,
}
