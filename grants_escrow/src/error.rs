use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    InvalidMilestones = 1,
    InvalidReviewers = 2,
    InvalidQuorum = 3,
    ReviewerIsGrantee = 4,
    GrantNotFound = 5,
    MilestoneNotFound = 6,
    InvalidMilestoneState = 7,
    DeadlinePassed = 8,
    NotAReviewer = 9,
    AlreadyApproved = 10,
    NothingToReclaim = 11,
}
