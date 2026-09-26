#![no_std]

use soroban_sdk::{contracterror, symbol_short, Address, Env};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum FallbackError {
    GracePeriodNotMet = 1,
    AlreadySettled = 2,
    Unauthorized = 3,
}

pub struct SettlementFallbackEngine;

impl SettlementFallbackEngine {
    pub const SETTLEMENT_GRACE_PERIOD: u64 = 86400 * 3; // 72-hour grace period post expiry

    pub fn execute_fallback_settlement(
        env: &Env,
        caller: &Address,
        series_id: u64,
        expiry_timestamp: u64,
        fallback_twap_price: i128,
    ) -> Result<(), FallbackError> {
        caller.require_auth();
        let now = env.ledger().timestamp();

        if now < expiry_timestamp + Self::SETTLEMENT_GRACE_PERIOD {
            return Err(FallbackError::GracePeriodNotMet);
        }

        let is_settled: bool = env.storage().persistent().get(&(symbol_short!("SETTLED"), series_id)).unwrap_or(false);
        if is_settled {
            return Err(FallbackError::AlreadySettled);
        }

        env.storage().persistent().set(&(symbol_short!("SETTLED"), series_id), &true);
        env.events().publish((symbol_short!("FB_SETTLE"), series_id), (fallback_twap_price, caller));
        Ok(())
    }
}
