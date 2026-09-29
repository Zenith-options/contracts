use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    InvalidAmount = 2,
    InvalidSchedule = 3,
    StreamNotFound = 4,
    Unauthorized = 5,
    InsufficientBalance = 6,
    StreamCanceled = 7,
}
