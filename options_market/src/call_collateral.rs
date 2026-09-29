#![no_std]

use soroban_sdk::{contracterror, symbol_short, Env};

#[contracterror]
#[derive(Copy, Clone, Debug, Eq, PartialEq)]
#[repr(u32)]
pub enum CollateralizationError {
    Undercollateralized = 1,
    InvalidContractSize = 2,
    SolvencyInvariantBreached = 3,
}

pub struct CallCollateralValidator;

impl CallCollateralValidator {
    /// Validates that minted call option contracts are 100% physically or cash collateralized
    /// against unbounded underlying spot price appreciation S.
    pub fn enforce_call_collateral(
        env: &Env,
        series_id: u64,
        collateral_deposited: i128,
        contracts_to_mint: i128,
        contract_size: i128,
    ) -> Result<i128, CollateralizationError> {
        if contracts_to_mint <= 0 || contract_size <= 0 {
            return Err(CollateralizationError::InvalidContractSize);
        }

        // Each call contract requires 1 full unit of underlying asset per unit size
        let required_backing = contracts_to_mint
            .checked_mul(contract_size)
            .ok_or(CollateralizationError::SolvencyInvariantBreached)?;

        if collateral_deposited < required_backing {
            return Err(CollateralizationError::Undercollateralized);
        }

        env.events().publish((symbol_short!("COLLAT_OK"), series_id), (required_backing,));
        Ok(required_backing)
    }
}
