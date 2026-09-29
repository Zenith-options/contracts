#![no_std]

use soroban_sdk::{contracterror, symbol_short, Address, Env};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum StakingError {
    BelowMinimumBond = 1,
    Unauthorized = 2,
    InsufficientStake = 3,
    FeederInactive = 4,
}

pub struct FeederStakingManager;

impl FeederStakingManager {
    const MIN_BOND_AMOUNT: i128 = 10_000_000_000; // 1,000 token bond

    pub fn deposit_feeder_bond(env: &Env, feeder: &Address, amount: i128) -> Result<(), StakingError> {
        feeder.require_auth();
        if amount < Self::MIN_BOND_AMOUNT {
            return Err(StakingError::BelowMinimumBond);
        }

        let current: i128 = env.storage().persistent().get(&(symbol_short!("BOND"), feeder)).unwrap_or(0);
        env.storage().persistent().set(&(symbol_short!("BOND"), feeder), &(current + amount));
        env.events().publish((symbol_short!("BOND_DEP"), feeder), (amount,));
        Ok(())
    }

    pub fn slash_malicious_feeder(env: &Env, admin: &Address, feeder: &Address, slash_amount: i128, insurance_fund: &Address) -> Result<(), StakingError> {
        admin.require_auth();
        let current: i128 = env.storage().persistent().get(&(symbol_short!("BOND"), feeder)).unwrap_or(0);
        if current < slash_amount {
            return Err(StakingError::InsufficientStake);
        }

        env.storage().persistent().set(&(symbol_short!("BOND"), feeder), &(current - slash_amount));
        env.events().publish((symbol_short!("SLASHED"), feeder), (slash_amount, insurance_fund));
        Ok(())
    }
}
