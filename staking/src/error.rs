use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    InvalidAmount = 2,
    InsufficientStake = 3,
    NothingToWithdraw = 4,
    CooldownActive = 5,
    TooManyRewardTokens = 6,
}
