use soroban_sdk::{contracttype, Address, Symbol};

// ─── Storage Keys ─────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone)]
pub enum DataKey {
    Admin,
    Oracle,
    CollateralToken, // USDC
    FeeRecipient,
    Series(u64),            // series_id -> OptionSeries
    Position(u64),          // position_id -> OptionPosition
    UserPositions(Address), // address -> Vec<u64>
    SeriesCounter,
    PositionCounter,
    UnderlyingPrice(Symbol),
    TotalPremiumsCollected,
    TotalOpenInterest,
    Paused,
    FeeRateBps,
    /// Lifetime number of series ever listed on an underlying. Kept for
    /// `get_series_count_for_underlying`; no longer a cap — see
    /// `ActiveSeriesCount`.
    SeriesCountForUnderlying(Symbol),
    PremiumPool,
    /// Running total of refund-eligible principal still outstanding for
    /// this series: every buy_option/write_option adds its position's own
    /// refund amount (premium net of fee, or full collateral), every
    /// claim_refund/claim_refund_from_vault subtracts what it actually
    /// paid out. Read once by escrow_series_to_vault to size
    /// exactly how much of options_market's own balance to quarantine
    /// into vault for this series — see that function's doc comment.
    SeriesEscrow(u64),
    /// Running total of unexercised ITM obligations across settled positions
    OrphanedLiabilities,
    ParamsRegistry,
    ParamsVersion,
    SettlementWindow,
    /// `(underlying, option_type, strike_price, expiry) -> series_id`.
    /// Removed again by `prune_series`, which frees the spec for relisting.
    SeriesIndex(Symbol, OptionType, i128, u64),
    /// Series on this underlying that still hold a listing slot (created
    /// and not yet released). Capped by `max_active_series`.
    ActiveSeriesCount(Symbol),
    /// Local override of `DEFAULT_MAX_ACTIVE_SERIES` (instance).
    MaxActiveSeries,
    /// Present once a series has released its `ActiveSeriesCount` slot —
    /// the "counted_inactive" guard that makes the decrement happen
    /// exactly once per series. Removed by `prune_series`.
    SeriesReleased(u64),
    /// Per-series position bookkeeping, see `SeriesPositionCounts`.
    SeriesPositions(u64),
    /// Ledger timestamp at which a series was settled or cancelled, the
    /// anchor for the prune retention period.
    SeriesClosedAt(u64),
    /// Progress of the one-off counter migration run after upgrading a
    /// contract that predates `ActiveSeriesCount`/`SeriesPositions`.
    Migration,
}

// ─── Data Types ───────────────────────────────────────────────────────────────

#[contracttype]
#[derive(Clone, PartialEq)]
pub enum OptionType {
    Call, // right to BUY at strike
    Put,  // right to SELL at strike
}

#[contracttype]
#[derive(Clone, PartialEq)]
pub enum SeriesState {
    Active,    // accepting trades
    Expired,   // past expiry, awaiting settlement
    Settled,   // final settlement price set
    Cancelled, // admin cancelled
}

#[contracttype]
#[derive(Clone, PartialEq)]
pub enum PositionSide {
    Long,  // bought option (paid premium)
    Short, // wrote option (received premium, locked collateral)
}

/// One option series = one expiry × one strike × one type × one underlying
#[contracttype]
#[derive(Clone)]
pub struct OptionSeries {
    pub series_id: u64,
    pub underlying: Symbol, // "XLM" | "BTC" | "ETH" | "SOL"
    pub option_type: OptionType,
    pub strike_price: i128, // PRICE_PRECISION scale
    pub expiry: u64,        // unix timestamp
    /// Premium per contract (1 contract = 1 unit of underlying, PRICE_PRECISION scale)
    pub premium: i128,
    /// Implied volatility used to price (RATE_PRECISION scale, e.g. 0.45 = 450_000_000)
    pub implied_vol: i128,
    pub open_interest: i128, // total contracts outstanding
    pub state: SeriesState,
    pub settlement_price: Option<i128>,
    pub created_at: u64,
}

/// One user's option position in a series
#[contracttype]
#[derive(Clone)]
pub struct OptionPosition {
    pub position_id: u64,
    pub series_id: u64,
    pub owner: Address,
    pub side: PositionSide,
    pub contracts: i128,         // PRICE_PRECISION scale (1.0 = 10_000_000)
    pub premium_paid: i128,      // total premium paid or received (gross, includes fee for longs)
    pub fee_paid: i128, // protocol fee actually deducted at open time (longs only; 0 for shorts)
    pub collateral_locked: i128, // for writers only
    pub is_exercised: bool,
    pub is_settled: bool,
    pub opened_at: u64,
}

/// Stored separately from `OptionSeries` so existing series entries keep
/// decoding after an upgrade.
#[contracttype]
#[derive(Clone, Default)]
pub struct SeriesPositionCounts {
    /// Positions in this series not yet pruned. `prune_series` requires 0.
    pub live: u32,
    /// Positions not yet closed by a refund, exercise, reclaim or
    /// forfeiture. A Cancelled series releases its slot when this hits 0.
    pub open: u32,
}

/// Result of `get_position_status`. Ids are allocated from a counter, so
/// a missing entry with an id at or below the counter was pruned.
#[contracttype]
#[derive(Clone)]
pub enum PositionStatus {
    Live(OptionPosition),
    Pruned,
    None,
}

#[contracttype]
#[derive(Clone)]
pub enum SeriesStatus {
    Live(OptionSeries),
    Pruned,
    None,
}

/// Cursor state for `migrate_counters`. Series ids at or below
/// `series_cursor` (and position ids at or below `position_cursor`) are
/// already reflected in the new counters; anything above is picked up by
/// a later page.
#[contracttype]
#[derive(Clone, Default)]
pub struct MigrationState {
    pub series_cursor: u64,
    pub position_cursor: u64,
    pub done: bool,
}
