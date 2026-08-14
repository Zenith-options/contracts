use soroban_sdk::contracterror;

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum Error {
    AlreadyInitialized = 1,
    Unauthorized = 2,
    SeriesNotFound = 3,
    SeriesNotActive = 4,
    SeriesNotExpired = 5,
    PositionNotFound = 6,
    InsufficientPremium = 7,
    InsufficientCollateral = 8,
    AlreadyExercised = 9,
    AlreadySettled = 10,
    ExerciseWindowClosed = 11,
    ZeroContracts = 12,
    PriceNotSet = 13,
    NotInTheMoney = 14,
    WrongSide = 15,
    ExpiryTooSoon = 16,
    ContractPaused = 17,
    SeriesNotCancelled = 18,
    InvalidFeeRate = 19,
    TooManySeriesForUnderlying = 20,
    InsufficientPremiumPool = 21,
}
