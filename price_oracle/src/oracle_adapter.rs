#![no_std]

use soroban_sdk::{contracterror, contracttype, symbol_short, Address, Env, Symbol};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum AdapterError {
    NoValidPrice = 1,
    ToleranceExceeded = 2,
    StalePrice = 3,
    HashMismatch = 4,
}

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct PriceQuote {
    pub price: i128,
    pub timestamp: u64,
    pub source: Symbol,
}

pub struct MultiSourceOracleAdapter;

impl MultiSourceOracleAdapter {
    pub fn resolve_price(
        env: &Env,
        primary_oracle: &Address,
        fallback_oracle: &Address,
        symbol: Symbol,
        max_staleness: u64,
        tolerance_bps: u32,
    ) -> Result<PriceQuote, AdapterError> {
        let now = env.ledger().timestamp();

        // 1. Query Primary (e.g. Zenith native feeder)
        let primary = Self::read_feed(env, primary_oracle, symbol);
        let fallback = Self::read_feed(env, fallback_oracle, symbol);

        match (primary, fallback) {
            (Some(p), Some(f)) => {
                let p_fresh = now.saturating_sub(p.timestamp) <= max_staleness;
                let f_fresh = now.saturating_sub(f.timestamp) <= max_staleness;

                if p_fresh && f_fresh {
                    let diff = if p.price > f.price { p.price - f.price } else { f.price - p.price };
                    let max_diff = (p.price * tolerance_bps as i128) / 10_000;
                    if diff > max_diff {
                        return Err(AdapterError::ToleranceExceeded);
                    }
                    Ok(p)
                } else if p_fresh {
                    Ok(p)
                } else if f_fresh {
                    Ok(f)
                } else {
                    Err(AdapterError::StalePrice)
                }
            }
            (Some(p), None) if now.saturating_sub(p.timestamp) <= max_staleness => Ok(p),
            (None, Some(f)) if now.saturating_sub(f.timestamp) <= max_staleness => Ok(f),
            _ => Err(AdapterError::NoValidPrice),
        }
    }

    fn read_feed(env: &Env, _source: &Address, _symbol: Symbol) -> Option<PriceQuote> {
        Some(PriceQuote {
            price: 10_000_000, // Normalized $1.00
            timestamp: env.ledger().timestamp(),
            source: symbol_short!("SEP40"),
        })
    }
}
