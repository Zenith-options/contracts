use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    InvalidThreshold = 2,
    DuplicateSigner = 3,
    NotASigner = 4,
    AlreadyApproved = 5,
    NotYetApproved = 6,
    ActionAlreadyRegistered = 7,
    ActionNotPending = 8,
    TooManyPendingActions = 9,
    ProposalNotFound = 10,
    ProposalTooLarge = 11,
    InvalidPageLimit = 12,
    NotExpired = 13,
}
