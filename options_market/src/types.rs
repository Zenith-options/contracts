use soroban_sdk::{contracttype, Address, Symbol, Vec};

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
    /// underlying -> every series_id listed on it, appended on create.
    /// Bounded by MAX_SERIES_PER_UNDERLYING.
    SeriesByUnderlying(Symbol),
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

// ─── View Types ───────────────────────────────────────────────────────────────

/// Filters for get_series_page. Each field matches a series whose value
/// is any of the listed ones; an empty list matches anything.
#[contracttype]
#[derive(Clone)]
pub struct SeriesFilter {
    pub state: Vec<SeriesState>,
    pub underlying: Vec<Symbol>,
    pub option_type: Vec<OptionType>,
}

/// One page of series. `next_cursor` is the cursor to pass for the next
/// page, or 0 once there are no more series to scan.
#[contracttype]
#[derive(Clone)]
pub struct SeriesPage {
    pub items: Vec<OptionSeries>,
    pub next_cursor: u64,
}

/// One page of a user's positions. `next_cursor` is the cursor to pass
/// for the next page, or 0 once there are no more positions to scan.
#[contracttype]
#[derive(Clone)]
pub struct PositionPage {
    pub items: Vec<OptionPosition>,
    pub next_cursor: u32,
}

/// INDICATIVE ONLY — a mark-to-market snapshot for frontends, never used
/// for settlement. Values are intrinsic value at the settlement price if
/// set, else at the last admin-set underlying price.
#[contracttype]
#[derive(Clone)]
pub struct AccountSummary {
    pub open_positions: u32,
    pub long_value: i128,
    pub short_liability: i128,
    pub collateral_locked: i128,
    pub net_value: i128,
}
