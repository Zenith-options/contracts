#![no_std]

//! Zenith Protocol — Decentralized Options Market on Stellar Soroban
//!
//! Supports European-style put and call options on XLM, BTC, ETH, and SOL.
//! Premium is set by the admin (computed off-chain via Black-Scholes).
//! Writers lock collateral; buyers pay premium. Settlement at expiry via oracle.

use soroban_sdk::{
    contract, contractimpl,
    Address, Env, Symbol, Vec, token,
    panic_with_error,
};

#[cfg(test)]
mod test;

mod error;
mod math;
mod types;

use error::Error;
use math::{calc_payout, MIN_COLLATERAL_RATIO, PRICE_PRECISION, RATE_PRECISION, SETTLEMENT_WINDOW};
use types::{DataKey, OptionPosition, OptionSeries, OptionType, PositionSide, SeriesState};

// ─── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct OptionsMarket;

#[contractimpl]
impl OptionsMarket {
    // ── Initialization ────────────────────────────────────────────────────────

    pub fn initialize(
        env: Env,
        admin: Address,
        oracle: Address,
        collateral_token: Address,
        fee_recipient: Address,
    ) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage().instance().set(&DataKey::Oracle, &oracle);
        env.storage().instance().set(&DataKey::CollateralToken, &collateral_token);
        env.storage().instance().set(&DataKey::FeeRecipient, &fee_recipient);
        env.storage().instance().set(&DataKey::SeriesCounter, &0u64);
        env.storage().instance().set(&DataKey::PositionCounter, &0u64);
        env.storage().instance().set(&DataKey::TotalPremiumsCollected, &0i128);
        env.storage().instance().set(&DataKey::TotalOpenInterest, &0i128);
    }

    // ── Series Management (Admin) ─────────────────────────────────────────────

    /// Admin lists a new option series (strike + expiry + type)
    /// Premium is computed off-chain via Black-Scholes and passed in
    pub fn create_series(
        env: Env,
        underlying: Symbol,
        option_type: OptionType,
        strike_price: i128,
        expiry: u64,
        premium: i128,
        implied_vol: i128,
    ) -> u64 {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let now = env.ledger().timestamp();
        if expiry <= now + 3600 {
            panic_with_error!(&env, Error::ExpiryTooSoon);
        }

        let counter: u64 = env.storage().instance().get(&DataKey::SeriesCounter).unwrap();
        let series_id = counter + 1;

        let series = OptionSeries {
            series_id,
            underlying,
            option_type,
            strike_price,
            expiry,
            premium,
            implied_vol,
            open_interest: 0,
            state: SeriesState::Active,
            settlement_price: None,
            created_at: now,
        };

        env.storage().persistent().set(&DataKey::Series(series_id), &series);
        env.storage().instance().set(&DataKey::SeriesCounter, &series_id);

        env.events().publish(
            (Symbol::new(&env, "series_created"),),
            (series_id, strike_price, expiry, premium),
        );

        series_id
    }

    /// Admin updates premium (e.g. after volatility changes)
    pub fn update_premium(env: Env, series_id: u64, new_premium: i128, new_implied_vol: i128) {
        let admin: Address = env.storage().instance().get(&DataKey::Admin).unwrap();
        admin.require_auth();

        let mut series: OptionSeries = env.storage().persistent()
            .get(&DataKey::Series(series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        if series.state != SeriesState::Active {
            panic_with_error!(&env, Error::SeriesNotActive);
        }

        series.premium = new_premium;
        series.implied_vol = new_implied_vol;
        env.storage().persistent().set(&DataKey::Series(series_id), &series);

        env.events().publish(
            (Symbol::new(&env, "premium_updated"), series_id),
            (new_premium, new_implied_vol),
        );
    }

    // ── Buying Options (Long) ─────────────────────────────────────────────────

    /// Buy `contracts` options in a series (pay premium in USDC)
    /// `contracts` is in PRICE_PRECISION scale (1 contract = 1e7)
    /// `max_premium` is slippage protection
    pub fn buy_option(
        env: Env,
        buyer: Address,
        series_id: u64,
        contracts: i128,
        max_premium: i128,
    ) -> u64 {
        buyer.require_auth();

        if contracts <= 0 {
            panic_with_error!(&env, Error::ZeroContracts);
        }

        let mut series: OptionSeries = Self::require_active_series(&env, series_id);

        let total_premium = contracts
            .checked_mul(series.premium)
            .unwrap()
            .checked_div(PRICE_PRECISION)
            .unwrap();

        if total_premium > max_premium {
            panic_with_error!(&env, Error::InsufficientPremium);
        }

        // Collect premium
        let collateral_token: Address = env.storage().instance().get(&DataKey::CollateralToken).unwrap();
        let usdc = token::Client::new(&env, &collateral_token);

        // Protocol fee: 0.5% of premium
        let fee = total_premium * 5 / 1000;
        let premium_after_fee = total_premium - fee;

        // Premium goes to vault (covers writer payouts on exercise)
        usdc.transfer(&buyer, &env.current_contract_address(), &total_premium);

        // Fee to protocol
        if fee > 0 {
            let fee_recipient: Address = env.storage().instance().get(&DataKey::FeeRecipient).unwrap();
            usdc.transfer(&env.current_contract_address(), &fee_recipient, &fee);
        }

        // Create position
        let pos_id = Self::next_position_id(&env);
        let position = OptionPosition {
            position_id: pos_id,
            series_id,
            owner: buyer.clone(),
            side: PositionSide::Long,
            contracts,
            premium_paid: total_premium,
            collateral_locked: 0,
            is_exercised: false,
            is_settled: false,
            opened_at: env.ledger().timestamp(),
        };

        env.storage().persistent().set(&DataKey::Position(pos_id), &position);
        Self::add_user_position(&env, &buyer, pos_id);

        // Update OI
        series.open_interest += contracts;
        env.storage().persistent().set(&DataKey::Series(series_id), &series);

        let total_collected: i128 = env.storage().instance().get(&DataKey::TotalPremiumsCollected).unwrap_or(0);
        env.storage().instance().set(&DataKey::TotalPremiumsCollected, &(total_collected + premium_after_fee));

        env.events().publish(
            (Symbol::new(&env, "option_bought"), buyer),
            (pos_id, series_id, contracts, total_premium),
        );

        pos_id
    }

    // ── Writing Options (Short / Covered) ─────────────────────────────────────

    /// Write (sell) options — lock collateral, receive premium
    /// For calls: collateral = contracts × underlying price (covered call)
    /// For puts:  collateral = contracts × strike price × 110% (cash-secured put)
    pub fn write_option(
        env: Env,
        writer: Address,
        series_id: u64,
        contracts: i128,
        collateral_amount: i128,
    ) -> u64 {
        writer.require_auth();

        if contracts <= 0 {
            panic_with_error!(&env, Error::ZeroContracts);
        }

        let mut series: OptionSeries = Self::require_active_series(&env, series_id);

        // Required collateral depends on option type
        let required_collateral = match series.option_type {
            // Covered call: lock collateral equal to notional (underlying * contracts)
            OptionType::Call => {
                let underlying_price: i128 = env.storage().persistent()
                    .get(&DataKey::UnderlyingPrice(series.underlying.clone()))
                    .unwrap_or(series.strike_price);
                contracts
                    .checked_mul(underlying_price)
                    .unwrap()
                    .checked_div(PRICE_PRECISION)
                    .unwrap()
            }
            // Cash-secured put: lock strike × contracts × 110%
            OptionType::Put => {
                let nominal = contracts
                    .checked_mul(series.strike_price)
                    .unwrap()
                    .checked_div(PRICE_PRECISION)
                    .unwrap();
                nominal
                    .checked_mul(MIN_COLLATERAL_RATIO)
                    .unwrap()
                    .checked_div(RATE_PRECISION)
                    .unwrap()
            }
        };

        if collateral_amount < required_collateral {
            panic_with_error!(&env, Error::InsufficientCollateral);
        }

        let collateral_token: Address = env.storage().instance().get(&DataKey::CollateralToken).unwrap();
        let usdc = token::Client::new(&env, &collateral_token);

        // Writer locks collateral
        usdc.transfer(&writer, &env.current_contract_address(), &required_collateral);

        // Writer receives premium (from vault balance)
        let total_premium = contracts
            .checked_mul(series.premium)
            .unwrap()
            .checked_div(PRICE_PRECISION)
            .unwrap();
        let fee = total_premium * 5 / 1000;
        let writer_premium = total_premium - fee;

        usdc.transfer(&env.current_contract_address(), &writer, &writer_premium);

        let pos_id = Self::next_position_id(&env);
        let position = OptionPosition {
            position_id: pos_id,
            series_id,
            owner: writer.clone(),
            side: PositionSide::Short,
            contracts,
            premium_paid: writer_premium,
            collateral_locked: required_collateral,
            is_exercised: false,
            is_settled: false,
            opened_at: env.ledger().timestamp(),
        };

        env.storage().persistent().set(&DataKey::Position(pos_id), &position);
        Self::add_user_position(&env, &writer, pos_id);

        series.open_interest += contracts;
        env.storage().persistent().set(&DataKey::Series(series_id), &series);

        env.events().publish(
            (Symbol::new(&env, "option_written"), writer),
            (pos_id, series_id, contracts, writer_premium, required_collateral),
        );

        pos_id
    }

    // ── Exercise ──────────────────────────────────────────────────────────────

    /// Exercise a long position before or at expiry (European = only at expiry)
    /// Settlement price must be set by oracle first
    pub fn exercise(env: Env, owner: Address, position_id: u64) {
        owner.require_auth();

        let mut position: OptionPosition = env.storage().persistent()
            .get(&DataKey::Position(position_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::PositionNotFound));

        if position.owner != owner {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if position.side != PositionSide::Long {
            panic_with_error!(&env, Error::WrongSide);
        }
        if position.is_exercised {
            panic_with_error!(&env, Error::AlreadyExercised);
        }

        let series: OptionSeries = env.storage().persistent()
            .get(&DataKey::Series(position.series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        let now = env.ledger().timestamp();

        // European: exercise only after expiry and within settlement window
        if now < series.expiry {
            panic_with_error!(&env, Error::SeriesNotExpired);
        }
        if now > series.expiry + SETTLEMENT_WINDOW {
            panic_with_error!(&env, Error::ExerciseWindowClosed);
        }

        let settlement_price = series.settlement_price
            .unwrap_or_else(|| panic_with_error!(&env, Error::PriceNotSet));

        // Determine payout
        let payout = calc_payout(&series.option_type, series.strike_price, settlement_price, position.contracts);

        if payout <= 0 {
            panic_with_error!(&env, Error::NotInTheMoney);
        }

        // Pay out to option holder
        let collateral_token: Address = env.storage().instance().get(&DataKey::CollateralToken).unwrap();
        let usdc = token::Client::new(&env, &collateral_token);
        usdc.transfer(&env.current_contract_address(), &owner, &payout);

        position.is_exercised = true;
        env.storage().persistent().set(&DataKey::Position(position_id), &position);

        env.events().publish(
            (Symbol::new(&env, "option_exercised"), owner),
            (position_id, settlement_price, payout),
        );
    }

    /// Oracle sets the settlement price for a series
    pub fn set_settlement_price(env: Env, series_id: u64, price: i128) {
        let oracle: Address = env.storage().instance().get(&DataKey::Oracle).unwrap();
        oracle.require_auth();

        let mut series: OptionSeries = env.storage().persistent()
            .get(&DataKey::Series(series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        let now = env.ledger().timestamp();
        if now < series.expiry {
            panic_with_error!(&env, Error::SeriesNotExpired);
        }

        series.settlement_price = Some(price);
        series.state = SeriesState::Settled;
        env.storage().persistent().set(&DataKey::Series(series_id), &series);
        env.storage().persistent().set(&DataKey::UnderlyingPrice(series.underlying.clone()), &price);

        env.events().publish(
            (Symbol::new(&env, "settlement_price_set"), series_id),
            price,
        );
    }

    /// Writers reclaim unused collateral after settlement
    pub fn reclaim_collateral(env: Env, writer: Address, position_id: u64) {
        writer.require_auth();

        let mut position: OptionPosition = env.storage().persistent()
            .get(&DataKey::Position(position_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::PositionNotFound));

        if position.owner != writer {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if position.side != PositionSide::Short {
            panic_with_error!(&env, Error::WrongSide);
        }
        if position.is_settled {
            panic_with_error!(&env, Error::AlreadySettled);
        }

        let series: OptionSeries = env.storage().persistent()
            .get(&DataKey::Series(position.series_id))
            .unwrap_or_else(|| panic_with_error!(&env, Error::SeriesNotFound));

        if series.state != SeriesState::Settled {
            panic_with_error!(&env, Error::SeriesNotExpired);
        }

        let settlement_price = series.settlement_price.unwrap_or_else(|| panic_with_error!(&env, Error::PriceNotSet));

        // Compute how much of collateral was consumed by exercised long positions
        let max_loss = calc_payout(&series.option_type, series.strike_price, settlement_price, position.contracts);
        let reclaim = (position.collateral_locked - max_loss).max(0);

        if reclaim > 0 {
            let collateral_token: Address = env.storage().instance().get(&DataKey::CollateralToken).unwrap();
            let usdc = token::Client::new(&env, &collateral_token);
            usdc.transfer(&env.current_contract_address(), &writer, &reclaim);
        }

        position.is_settled = true;
        env.storage().persistent().set(&DataKey::Position(position_id), &position);

        env.events().publish(
            (Symbol::new(&env, "collateral_reclaimed"), writer),
            (position_id, reclaim),
        );
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    pub fn get_series(env: Env, series_id: u64) -> Option<OptionSeries> {
        env.storage().persistent().get(&DataKey::Series(series_id))
    }

    pub fn get_position(env: Env, position_id: u64) -> Option<OptionPosition> {
        env.storage().persistent().get(&DataKey::Position(position_id))
    }

    pub fn get_user_positions(env: Env, user: Address) -> Vec<u64> {
        env.storage().persistent()
            .get(&DataKey::UserPositions(user))
            .unwrap_or_else(|| Vec::new(&env))
    }

    pub fn get_underlying_price(env: Env, underlying: Symbol) -> Option<i128> {
        env.storage().persistent().get(&DataKey::UnderlyingPrice(underlying))
    }

    pub fn get_stats(env: Env) -> (i128, i128, u64) {
        let premiums: i128 = env.storage().instance().get(&DataKey::TotalPremiumsCollected).unwrap_or(0);
        let oi: i128 = env.storage().instance().get(&DataKey::TotalOpenInterest).unwrap_or(0);
        let series_count: u64 = env.storage().instance().get(&DataKey::SeriesCounter).unwrap_or(0);
        (premiums, oi, series_count)
    }

    // ── Internal Helpers ──────────────────────────────────────────────────────

    fn require_active_series(env: &Env, series_id: u64) -> OptionSeries {
        let series: OptionSeries = env.storage().persistent()
            .get(&DataKey::Series(series_id))
            .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound));
        if series.state != SeriesState::Active {
            panic_with_error!(env, Error::SeriesNotActive);
        }
        if env.ledger().timestamp() >= series.expiry {
            panic_with_error!(env, Error::SeriesNotActive);
        }
        series
    }

    fn next_position_id(env: &Env) -> u64 {
        let counter: u64 = env.storage().instance().get(&DataKey::PositionCounter).unwrap_or(0);
        let next = counter + 1;
        env.storage().instance().set(&DataKey::PositionCounter, &next);
        next
    }

    fn add_user_position(env: &Env, user: &Address, position_id: u64) {
        let key = DataKey::UserPositions(user.clone());
        let mut positions: Vec<u64> = env.storage().persistent()
            .get(&key)
            .unwrap_or_else(|| Vec::new(env));
        positions.push_back(position_id);
        env.storage().persistent().set(&key, &positions);
    }
}
