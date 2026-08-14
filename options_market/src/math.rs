use crate::types::OptionType;

pub const PRICE_PRECISION: i128 = 10_000_000; // 1e7
pub const RATE_PRECISION: i128 = 1_000_000_000; // 1e9
pub const MIN_COLLATERAL_RATIO: i128 = 1_100_000_000; // 110% over-collateralization for puts
pub const SETTLEMENT_WINDOW: u64 = 86_400; // 24h window after expiry to exercise

pub const DEFAULT_FEE_RATE_BPS: i128 = 50; // 0.5%
pub const MAX_FEE_RATE_BPS: i128 = 1_000; // 10% hard ceiling, even for the admin

/// Caps how many series can ever be listed for a given underlying, so a
/// single symbol can't accumulate unbounded storage entries over the
/// contract's lifetime. This counts every series ever created, not
/// currently-active ones — cancelling or letting a series expire doesn't
/// free up room, since nothing about storage usage shrinks when that
/// happens either.
pub const MAX_SERIES_PER_UNDERLYING: u32 = 50;

/// Protocol fee on an amount, given a rate in basis points (1 bps = 0.01%).
pub fn calc_fee(amount: i128, fee_rate_bps: i128) -> i128 {
    amount.checked_mul(fee_rate_bps).unwrap().checked_div(10_000).unwrap()
}

/// Cash payout at settlement:
/// Call: max(0, settlement - strike) × contracts / PRICE_PRECISION
/// Put:  max(0, strike - settlement) × contracts / PRICE_PRECISION
pub fn calc_payout(option_type: &OptionType, strike: i128, settlement: i128, contracts: i128) -> i128 {
    let intrinsic = match option_type {
        OptionType::Call => (settlement - strike).max(0),
        OptionType::Put => (strike - settlement).max(0),
    };
    contracts.checked_mul(intrinsic).unwrap().checked_div(PRICE_PRECISION).unwrap()
}
