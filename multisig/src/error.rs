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
    InvalidDelays = 7,
    AccountNotConfigured = 8,
    UnsortedSignatures = 9,
    InsufficientSignatures = 10,
    ContextNotAllowed = 11,
    ActionAlreadyRegistered = 12,
    ActionNotPending = 13,
    TooManyPendingActions = 14,
    ProposalNotFound = 15,
    ProposalTooLarge = 16,
    InvalidPageLimit = 17,
    NotExpired = 18,
    // Schema migrations (zenith_common::migrations) — same codes in
    // every Zenith contract. See docs/migrations.md.
    SchemaVersionMismatch = 100,
    InvalidMigrationTarget = 101,
    MigrationInProgress = 102,
    NoMigrationInProgress = 103,
    InvalidMigrationBatch = 104,
    // Self-upgrade (upgrade.rs).
    UpgradeNotUnanimous = 105,
    UpgradeDelayNotElapsed = 106,
}
