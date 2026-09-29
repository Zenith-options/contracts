use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    InvalidProposal = 2,
    ProposalExists = 3,
    ProposalNotFound = 4,
    BelowProposalThreshold = 5,
    InvalidState = 6,
    AlreadyVoted = 7,
    InvalidSupport = 8,
    OutOfBounds = 9,
    Unauthorized = 10,
}
