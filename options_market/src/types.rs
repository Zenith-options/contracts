use soroban_sdk::{contracttype, Address};

/// Consolidated hot configuration, loaded once per call instead of five
/// separate instance reads (CollateralToken, FeeRecipient, FeeRateBps,
/// PremiumPool, TotalPremiumsCollected). See issue #102.
#[contracttype]
#[derive(Clone)]
pub struct Config {
    pub admin: Address,
    pub oracle: Address,
    pub token: Address,
    pub fee_recipient: Address,
    pub fee_bps: u32,
    pub paused: bool,
}

/// Aggregated counters, written once per call instead of several
/// read-modify-write single-counter entries. See issue #102.
#[contracttype]
#[derive(Clone)]
pub struct Stats {
    pub premiums_collected: i128,
    pub open_interest: i128,
    pub premium_pool: i128,
    pub series_count: u64,
    pub position_counter: u64,
}

/// `Open`/`Exercised`/`Settled`/`Refunded` replaces the two `bool` flags
/// (`is_exercised`, `is_settled`) that used to live directly on
/// `OptionPosition`. See issue #103.
#[contracttype]
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PositionStatus {
    Open,
    Exercised,
    Settled,
    Refunded,
}

/// Compact position layout (issue #103): the boolean flags are folded into
/// `status`, and the struct only carries the fields every reader needs.
/// `opened_at` is a cold field kept here rather than split into a second
/// entry — splitting it out is left for a follow-up once it's clear readers
/// need `OptionPosition` without paying for it.
#[contracttype]
#[derive(Clone)]
pub struct OptionPosition {
    pub series_id: u64,
    pub owner: Address,
    pub side_is_buyer: bool,
    pub amount: i128,
    pub collateral_locked: i128,
    pub status: PositionStatus,
    pub opened_at: u64,
}

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Config,
    Stats,
    Position(u64),
    UserPositionCount(Address),
    UserPositionPage(Address, u32),
}

/// Number of position ids stored per `UserPositionPage` bucket (issue #100).
pub const POSITIONS_PER_PAGE: u32 = 64;
