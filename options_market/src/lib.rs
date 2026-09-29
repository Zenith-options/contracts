#![no_std]

//! Zenith Protocol — Decentralized Options Market on Stellar Soroban
//!
//! Supports European-style put and call options on XLM, BTC, ETH, and SOL.
//! Premium is set by the admin (computed off-chain via Black-Scholes).
//! Writers lock collateral; buyers pay premium. Settlement at expiry via oracle.
//!
//! Size note: `///` docs on `#[contractimpl]` functions are embedded in the
//! wasm's contract spec, so entrypoints carry a short `///` summary and
//! keep longer rationale in `//` comments (see the README reference).

use soroban_sdk::{
    contract, contractimpl, panic_with_error, token, Address, BytesN, Env, Symbol, Vec,
};
use zenith_common::{ActionClass, Auth};

#[cfg(test)]
mod test;
#[cfg(test)]
mod test_resources;

mod error;
mod events;
mod math;
mod params_client;
mod price_oracle_client;
mod storage;
mod ttl;
mod types;
mod vault_client;

use error::Error;
use math::{
    calc_fee, calc_payout, DEFAULT_FEE_RATE_BPS, FORFEITURE_WINDOW, MAX_BATCH_SIZE,
    MAX_FEE_RATE_BPS, MAX_PAGE_SCAN, MIN_COLLATERAL_RATIO, PRICE_PRECISION, PRUNE_RETENTION,
    RATE_PRECISION,
};
use storage::{
    active_series_count, add_orphaned_liability, add_user_position, claim_series_index,
    count_position_closed, count_position_opened, count_position_pruned, deduct_orphaned_liability,
    fee_rate_bps, is_slot_releasable, mark_slot_released, max_active_series, migration_done,
    migration_state, next_position_id, remove_user_position, require_active_series,
    require_migrated, require_not_paused, series_counts, set_active_series_count,
    set_migration_state, set_series_counts, settlement_window,
};
pub use types::{
    DataKey, MigrationState, OptionPosition, OptionSeries, OptionType, PositionSide,
    PositionStatus, SeriesPositionCounts, SeriesState, SeriesStatus,
};

// ─── Contract ─────────────────────────────────────────────────────────────────

#[contract]
pub struct OptionsMarket;

// ─── Internal helpers (not exported) ─────────────────────────────────────────
//
// Every admin action has an admin-signed entrypoint and a `_via_multisig`
// twin. Both call the same `*_inner(env, Auth, ...)`, which starts with
// `authorize` — one copy of the validation and state change instead of two.

fn authorize(env: &Env, auth: &Auth, class: ActionClass) {
    zenith_common::require_admin_or_multisig(
        env,
        &DataKey::Admin,
        auth,
        class,
        Error::Unauthorized,
    );
}

fn usdc(env: &Env) -> token::Client<'_> {
    let collateral_token: Address = env
        .storage()
        .instance()
        .get(&DataKey::CollateralToken)
        .unwrap();
    token::Client::new(env, &collateral_token)
}

fn load_series(env: &Env, series_id: u64) -> OptionSeries {
    ttl::get_persistent(env, &DataKey::Series(series_id))
        .unwrap_or_else(|| panic_with_error!(env, Error::SeriesNotFound))
}

fn load_position(env: &Env, position_id: u64) -> OptionPosition {
    ttl::get_persistent(env, &DataKey::Position(position_id))
        .unwrap_or_else(|| panic_with_error!(env, Error::PositionNotFound))
}

fn save_series(env: &Env, series: &OptionSeries) {
    ttl::set_persistent(env, &DataKey::Series(series.series_id), series);
}

fn save_position(env: &Env, position: &OptionPosition) {
    ttl::set_persistent(env, &DataKey::Position(position.position_id), position);
}

fn require_batch(env: &Env, ids: &Vec<u64>) {
    if ids.is_empty() || ids.len() > MAX_BATCH_SIZE {
        panic_with_error!(env, Error::InvalidBatchSize);
    }
}

fn add_instance_i128(env: &Env, key: &DataKey, delta: i128) {
    let current: i128 = env.storage().instance().get(key).unwrap_or(0);
    env.storage()
        .instance()
        .set(key, &current.checked_add(delta).unwrap());
}

fn add_series_escrow(env: &Env, series_id: u64, delta: i128) {
    let key = DataKey::SeriesEscrow(series_id);
    let outstanding: i128 = ttl::get_persistent(env, &key).unwrap_or(0);
    ttl::set_persistent(env, &key, &outstanding.checked_add(delta).unwrap());
}

/// Premium for `contracts` at the series' current premium.
fn premium_for(series: &OptionSeries, contracts: i128) -> i128 {
    contracts
        .checked_mul(series.premium)
        .unwrap()
        .checked_div(PRICE_PRECISION)
        .unwrap()
}

fn series_payout(series: &OptionSeries, settlement_price: i128, contracts: i128) -> i128 {
    calc_payout(
        &series.option_type,
        series.strike_price,
        settlement_price,
        contracts,
    )
}

/// Releases `series`' active-series slot if it's releasable and hasn't
/// been released yet, emitting `series_slot_released`.
fn try_release_slot(env: &Env, series: &OptionSeries) -> bool {
    let counts = series_counts(env, series.series_id);
    if is_slot_releasable(env, series, &counts) && mark_slot_released(env, series) {
        events::series_slot_released(
            env,
            series.underlying.clone(),
            series.series_id,
            active_series_count(env, &series.underlying),
        );
        return true;
    }
    false
}

// ── Retention policy ──
//
// A series is terminal once Settled or Cancelled. Its retention period
// (PRUNE_RETENTION, 30 days) runs from its anchor:
// - Settled: the later of the settlement time and the end of the exercise
//   window (expiry + settlement_window).
// - Cancelled: the cancellation time.
// Series closed before this code was deployed have no SeriesClosedAt and
// fall back to expiry, which is never earlier than the real close for a
// cancelled series and never later than the exercise-window end for a
// settled one.
fn retention_anchor(env: &Env, series: &OptionSeries) -> u64 {
    let closed_at: Option<u64> =
        ttl::get_persistent(env, &DataKey::SeriesClosedAt(series.series_id));
    match series.state {
        SeriesState::Settled => closed_at
            .unwrap_or(0)
            .max(series.expiry.saturating_add(settlement_window(env))),
        SeriesState::Cancelled => closed_at.unwrap_or(series.expiry),
        SeriesState::Active | SeriesState::Expired => {
            panic_with_error!(env, Error::NotPrunable)
        }
    }
}

fn require_retention_elapsed(env: &Env, anchor: u64) {
    if env.ledger().timestamp() < anchor.saturating_add(PRUNE_RETENTION) {
        panic_with_error!(env, Error::RetentionNotElapsed);
    }
}

// A position is terminal (and carries no liability) when:
// - its series is Cancelled and it has taken its refund (is_settled);
// - its series is Settled and it is a Short whose collateral was
//   reclaimed (is_settled), or a Long that was exercised or forfeited
//   (is_exercised) or expired worthless (payout 0).
// An unexercised ITM long is NOT terminal until sweep_forfeited pays its
// payout out, and an unreclaimed short never is.
fn is_position_terminal(series: &OptionSeries, position: &OptionPosition) -> bool {
    match series.state {
        SeriesState::Cancelled => position.is_settled,
        SeriesState::Settled => match position.side {
            PositionSide::Short => position.is_settled,
            PositionSide::Long => {
                position.is_exercised
                    || series_payout(
                        series,
                        series.settlement_price.unwrap_or(0),
                        position.contracts,
                    ) == 0
            }
        },
        SeriesState::Active | SeriesState::Expired => false,
    }
}

#[contractimpl]
impl OptionsMarket {
    /// Permissionless keeper entrypoint: extends the instance and every
    /// named persistent entry that exists.
    pub fn bump(env: Env, keys: Vec<DataKey>) {
        ttl::extend_instance(&env);
        for key in keys.iter() {
            ttl::extend_persistent_if_present(&env, &key);
        }
    }

    // ── Initialization ────────────────────────────────────────────────────────

    pub fn initialize(
        env: Env,
        admin: Address,
        oracle: Address,
        collateral_token: Address,
        fee_recipient: Address,
    ) {
        ttl::extend_instance(&env);
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        let instance = env.storage().instance();
        instance.set(&DataKey::Admin, &admin);
        instance.set(&DataKey::Oracle, &oracle);
        instance.set(&DataKey::CollateralToken, &collateral_token);
        instance.set(&DataKey::FeeRecipient, &fee_recipient);
        instance.set(&DataKey::SeriesCounter, &0u64);
        instance.set(&DataKey::PositionCounter, &0u64);
        instance.set(&DataKey::TotalPremiumsCollected, &0i128);
        instance.set(&DataKey::TotalOpenInterest, &0i128);
        instance.set(&DataKey::FeeRateBps, &DEFAULT_FEE_RATE_BPS);
        instance.set(&DataKey::PremiumPool, &0i128);
        // A fresh deployment has nothing to migrate.
        set_migration_state(
            &env,
            &MigrationState {
                series_cursor: u64::MAX,
                position_cursor: u64::MAX,
            },
        );
    }

    // ── Admin (each with a `_via_multisig` twin) ──────────────────────────────

    /// Sets the protocol fee rate in bps, capped at MAX_FEE_RATE_BPS.
    pub fn set_fee_rate(env: Env, new_bps: u32) {
        Self::set_fee_rate_inner(&env, Auth::Admin, new_bps);
    }

    /// `set_fee_rate` authorized by an executable multisig action (Standard).
    pub fn set_fee_rate_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_bps: u32,
    ) {
        Self::set_fee_rate_inner(&env, Auth::Multisig(multisig_contract, action_id), new_bps);
    }

    fn set_fee_rate_inner(env: &Env, auth: Auth, new_bps: u32) {
        ttl::extend_instance(env);
        authorize(env, &auth, ActionClass::Standard);
        // The cap applies to multisig approvals too: M-of-N changes who may
        // call this, not which rates are sane.
        let new_bps = new_bps as i128;
        if new_bps > MAX_FEE_RATE_BPS {
            panic_with_error!(env, Error::InvalidFeeRate);
        }
        env.storage().instance().set(&DataKey::FeeRateBps, &new_bps);
        events::fee_rate_updated(env, new_bps);
    }

    /// Sets the per-underlying cap on concurrently active series
    /// (1..=MAX_ACTIVE_SERIES_CEILING). A params registry value (`max_ser`)
    /// overrides it on the next registry version change.
    pub fn set_max_active_series(env: Env, cap: u32) {
        Self::set_max_active_series_inner(&env, Auth::Admin, cap);
    }

    /// `set_max_active_series` authorized by a multisig action (Standard).
    pub fn set_max_active_series_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        cap: u32,
    ) {
        Self::set_max_active_series_inner(&env, Auth::Multisig(multisig_contract, action_id), cap);
    }

    fn set_max_active_series_inner(env: &Env, auth: Auth, cap: u32) {
        ttl::extend_instance(env);
        authorize(env, &auth, ActionClass::Standard);
        storage::set_max_active_series(env, cap);
        events::max_active_series_updated(env, cap);
    }

    /// Points fee rate, settlement window and the active-series cap at a
    /// params registry. Clears the cached registry version.
    pub fn set_params_registry(env: Env, registry: Address) {
        ttl::extend_instance(&env);
        authorize(&env, &Auth::Admin, ActionClass::Standard);
        env.storage()
            .instance()
            .set(&DataKey::ParamsRegistry, &registry);
        env.storage().instance().remove(&DataKey::ParamsVersion);
    }

    pub fn get_settlement_window(env: Env) -> u64 {
        ttl::extend_instance(&env);
        settlement_window(&env)
    }

    /// Hands admin to `new_admin`; the current admin signs.
    pub fn transfer_admin(env: Env, new_admin: Address) {
        Self::transfer_admin_inner(&env, Auth::Admin, new_admin);
    }

    /// `transfer_admin` authorized by a multisig action (Critical).
    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_admin: Address,
    ) {
        Self::transfer_admin_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            new_admin,
        );
    }

    fn transfer_admin_inner(env: &Env, auth: Auth, new_admin: Address) {
        ttl::extend_instance(env);
        authorize(env, &auth, ActionClass::Critical);
        let old = zenith_common::set_admin(env, &DataKey::Admin, &new_admin);
        events::admin_transferred(env, old, new_admin);
    }

    /// Swaps this contract's wasm, keeping its address and storage. After
    /// upgrading from a version without active-series counters, run
    /// `migrate_counters` until it returns true.
    pub fn upgrade(env: Env, new_wasm_hash: BytesN<32>) {
        Self::upgrade_inner(&env, Auth::Admin, new_wasm_hash);
    }

    /// `upgrade` authorized by a multisig action (Critical).
    pub fn upgrade_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_wasm_hash: BytesN<32>,
    ) {
        Self::upgrade_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            new_wasm_hash,
        );
    }

    fn upgrade_inner(env: &Env, auth: Auth, new_wasm_hash: BytesN<32>) {
        ttl::extend_instance(env);
        authorize(env, &auth, ActionClass::Critical);
        env.deployer().update_current_contract_wasm(new_wasm_hash);
    }

    /// Emergency stop for create_series, update_premium, buy_option,
    /// write_option, transfer_position and split_position. Exercise,
    /// settlement, reclaim, refunds and pruning keep working.
    pub fn pause(env: Env) {
        Self::set_paused_inner(&env, Auth::Admin, true);
    }

    pub fn unpause(env: Env) {
        Self::set_paused_inner(&env, Auth::Admin, false);
    }

    /// `pause` authorized by a multisig action (Emergency, no delay).
    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        Self::set_paused_inner(&env, Auth::Multisig(multisig_contract, action_id), true);
    }

    /// `unpause` authorized by a multisig action (Standard).
    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        Self::set_paused_inner(&env, Auth::Multisig(multisig_contract, action_id), false);
    }

    fn set_paused_inner(env: &Env, auth: Auth, paused: bool) {
        ttl::extend_instance(env);
        let class = if paused {
            ActionClass::Emergency
        } else {
            ActionClass::Standard
        };
        authorize(env, &auth, class);
        zenith_common::set_paused(env, &DataKey::Paused, paused);
        if paused {
            events::paused(env);
        } else {
            events::unpaused(env);
        }
    }

    // ── Series Management (Admin) ─────────────────────────────────────────────

    /// Lists a new series. Capped at `max_active_series` concurrently
    /// active series per underlying; each spec may be listed once until
    /// its series is pruned.
    pub fn create_series(
        env: Env,
        underlying: Symbol,
        option_type: OptionType,
        strike_price: i128,
        expiry: u64,
        premium: i128,
        implied_vol: i128,
    ) -> u64 {
        Self::create_series_inner(
            &env,
            Auth::Admin,
            underlying,
            option_type,
            strike_price,
            expiry,
            premium,
            implied_vol,
        )
    }

    /// `create_series` authorized by a multisig action (Standard).
    #[allow(clippy::too_many_arguments)]
    pub fn create_series_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        underlying: Symbol,
        option_type: OptionType,
        strike_price: i128,
        expiry: u64,
        premium: i128,
        implied_vol: i128,
    ) -> u64 {
        Self::create_series_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            underlying,
            option_type,
            strike_price,
            expiry,
            premium,
            implied_vol,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn create_series_inner(
        env: &Env,
        auth: Auth,
        underlying: Symbol,
        option_type: OptionType,
        strike_price: i128,
        expiry: u64,
        premium: i128,
        implied_vol: i128,
    ) -> u64 {
        ttl::extend_instance(env);
        require_not_paused(env);
        authorize(env, &auth, ActionClass::Standard);
        // Active counts are only trustworthy once an upgraded contract
        // has recounted its existing series.
        require_migrated(env);

        let now = env.ledger().timestamp();
        if expiry <= now + 3600 {
            panic_with_error!(env, Error::ExpiryTooSoon);
        }
        if strike_price <= 0 || premium <= 0 || implied_vol < 0 {
            panic_with_error!(env, Error::InvalidSeriesParams);
        }

        // Rolling cap: counts series that still hold a slot, not every
        // series ever listed (see storage::is_slot_releasable).
        let active = active_series_count(env, &underlying);
        if active >= max_active_series(env) {
            panic_with_error!(env, Error::TooManySeriesForUnderlying);
        }
        set_active_series_count(env, &underlying, active.checked_add(1).unwrap());

        // Lifetime counter, informational only.
        let lifetime_key = DataKey::SeriesCountForUnderlying(underlying.clone());
        let lifetime: u32 = ttl::get_persistent(env, &lifetime_key).unwrap_or(0);
        ttl::set_persistent(env, &lifetime_key, &lifetime.saturating_add(1));

        let counter: u64 = env
            .storage()
            .instance()
            .get(&DataKey::SeriesCounter)
            .unwrap();
        let series_id = counter.checked_add(1).unwrap();
        claim_series_index(
            env,
            &underlying,
            &option_type,
            strike_price,
            expiry,
            series_id,
        );

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

        save_series(env, &series);
        env.storage()
            .instance()
            .set(&DataKey::SeriesCounter, &series_id);

        events::series_created(env, series_id, strike_price, expiry, premium);

        series_id
    }

    /// Re-prices an Active series. `new_premium` must be > 0.
    pub fn update_premium(env: Env, series_id: u64, new_premium: i128, new_implied_vol: i128) {
        Self::update_premium_inner(&env, Auth::Admin, series_id, new_premium, new_implied_vol);
    }

    /// `update_premium` authorized by a multisig action (Standard).
    pub fn update_premium_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        series_id: u64,
        new_premium: i128,
        new_implied_vol: i128,
    ) {
        Self::update_premium_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            series_id,
            new_premium,
            new_implied_vol,
        );
    }

    fn update_premium_inner(
        env: &Env,
        auth: Auth,
        series_id: u64,
        new_premium: i128,
        new_implied_vol: i128,
    ) {
        ttl::extend_instance(env);
        require_not_paused(env);
        authorize(env, &auth, ActionClass::Standard);

        let mut series = load_series(env, series_id);
        if series.state != SeriesState::Active {
            panic_with_error!(env, Error::SeriesNotActive);
        }
        // A zero (or negative) premium would let buyers open positions for
        // free and writers lock collateral for nothing (#47).
        if new_premium <= 0 {
            panic_with_error!(env, Error::InvalidSeriesParams);
        }

        series.premium = new_premium;
        series.implied_vol = new_implied_vol;
        save_series(env, &series);

        events::premium_updated(env, series_id, new_premium, new_implied_vol);
    }

    /// Cancels an Active series; holders then pull refunds with
    /// `claim_refund`. A series with no positions releases its
    /// active-series slot immediately.
    pub fn cancel_series(env: Env, series_id: u64) {
        Self::cancel_series_inner(&env, Auth::Admin, series_id);
    }

    /// `cancel_series` authorized by a multisig action (Standard).
    pub fn cancel_series_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        series_id: u64,
    ) {
        Self::cancel_series_inner(
            &env,
            Auth::Multisig(multisig_contract, action_id),
            series_id,
        );
    }

    // Holders pull their own refunds rather than the admin pushing funds
    // to everyone in one call — an unbounded push would scale badly
    // against Soroban's per-call resource limits.
    fn cancel_series_inner(env: &Env, auth: Auth, series_id: u64) {
        ttl::extend_instance(env);
        authorize(env, &auth, ActionClass::Standard);

        let mut series = load_series(env, series_id);
        if series.state != SeriesState::Active {
            panic_with_error!(env, Error::SeriesNotActive);
        }

        series.state = SeriesState::Cancelled;
        save_series(env, &series);
        ttl::set_persistent(
            env,
            &DataKey::SeriesClosedAt(series_id),
            &env.ledger().timestamp(),
        );

        events::series_cancelled(env, series_id);
        try_release_slot(env, &series);
    }

    // ── Refunds ───────────────────────────────────────────────────────────────

    /// Refund for a position in a Cancelled series, paid from this
    /// contract's balance: longs get premium net of fee, shorts their
    /// collateral.
    pub fn claim_refund(env: Env, owner: Address, position_id: u64) {
        Self::refund_inner(&env, None, owner, position_id);
    }

    /// Same as `claim_refund`, paid from the series' vault escrow (see
    /// `escrow_series_to_vault`).
    pub fn claim_refund_from_vault(
        env: Env,
        vault_contract: Address,
        owner: Address,
        position_id: u64,
    ) {
        Self::refund_inner(&env, Some(vault_contract), owner, position_id);
    }

    // Longs only ever had premium_paid (gross, including the protocol fee)
    // here for a moment before the fee went to fee_recipient, so refunding
    // the gross amount would draw down other positions' funds. Refund net
    // of the fee actually deducted at buy time (fee_paid, not a
    // recomputation off the current rate). The fee isn't clawed back.
    //
    // The vault path only changes where the payout is funded from, not who
    // is entitled to what.
    fn refund_inner(env: &Env, vault_contract: Option<Address>, owner: Address, position_id: u64) {
        ttl::extend_instance(env);
        owner.require_auth();

        let mut position = load_position(env, position_id);
        if position.owner != owner {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.is_settled {
            panic_with_error!(env, Error::AlreadySettled);
        }

        let series = load_series(env, position.series_id);
        if series.state != SeriesState::Cancelled {
            panic_with_error!(env, Error::SeriesNotCancelled);
        }

        let refund = match position.side {
            PositionSide::Long => position
                .premium_paid
                .checked_sub(position.fee_paid)
                .unwrap(),
            PositionSide::Short => position.collateral_locked,
        };

        if refund > 0 {
            match &vault_contract {
                None => usdc(env).transfer(&env.current_contract_address(), &owner, &refund),
                Some(vault_contract) => {
                    let token: Address = env
                        .storage()
                        .instance()
                        .get(&DataKey::CollateralToken)
                        .unwrap();
                    vault_client::Client::new(env, vault_contract).withdraw(
                        &token,
                        &vault_client::series_tag(env, position.series_id),
                        &owner,
                        &refund,
                    );
                }
            }
        }

        add_series_escrow(env, position.series_id, -refund);

        position.is_settled = true;
        save_position(env, &position);
        count_position_closed(env, position.series_id, position_id);

        events::refund_claimed(env, owner, position_id, refund);

        // The last refund of a Cancelled series frees its listing slot.
        try_release_slot(env, &series);
    }

    /// Moves a Cancelled series' remaining refund liability (SeriesEscrow)
    /// from this contract's shared balance into `vault`, tagged by series.
    /// Permissionless; at most once per series.
    //
    // Once quarantined, vault's own balance check bounds what
    // claim_refund_from_vault can pay out against this series' tag.
    // Requires `vault` to have this contract registered as an integrator
    // and the collateral token allowlisted; this contract then owns the
    // `(self, "series", id)` tag, which is what lets its deposit/withdraw
    // calls pass vault's tag-owner auth without a human signature.
    pub fn escrow_series_to_vault(env: Env, vault_contract: Address, series_id: u64) {
        ttl::extend_instance(&env);
        let series = load_series(&env, series_id);
        if series.state != SeriesState::Cancelled {
            panic_with_error!(&env, Error::SeriesNotCancelled);
        }

        let series_escrow_key = DataKey::SeriesEscrow(series_id);
        let amount: i128 = ttl::get_persistent(&env, &series_escrow_key).unwrap_or(0);
        if amount <= 0 {
            panic_with_error!(&env, Error::NothingToEscrow);
        }
        ttl::set_persistent(&env, &series_escrow_key, &0i128);

        let token: Address = env
            .storage()
            .instance()
            .get(&DataKey::CollateralToken)
            .unwrap();
        vault_client::Client::new(&env, &vault_contract).deposit(
            &env.current_contract_address(),
            &token,
            &vault_client::series_tag(&env, series_id),
            &amount,
        );

        events::series_escrowed_to_vault(&env, series_id, amount);
    }

    pub fn get_series_escrow(env: Env, series_id: u64) -> i128 {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::SeriesEscrow(series_id)).unwrap_or(0)
    }

    // ── Buying Options (Long) ─────────────────────────────────────────────────

    /// Buys `contracts` (PRICE_PRECISION scale) of a series, paying at most
    /// `max_premium`. Returns the new position id.
    pub fn buy_option(
        env: Env,
        buyer: Address,
        series_id: u64,
        contracts: i128,
        max_premium: i128,
    ) -> u64 {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        buyer.require_auth();

        if contracts <= 0 {
            panic_with_error!(&env, Error::ZeroContracts);
        }

        let mut series = require_active_series(&env, series_id);

        let total_premium = premium_for(&series, contracts);
        if total_premium > max_premium {
            panic_with_error!(&env, Error::InsufficientPremium);
        }

        let usdc = usdc(&env);
        let fee = calc_fee(total_premium, fee_rate_bps(&env));
        let premium_after_fee = total_premium - fee;

        // Premium comes here (covers writer payouts on exercise).
        usdc.transfer(&buyer, &env.current_contract_address(), &total_premium);
        if fee > 0 {
            let fee_recipient: Address = env
                .storage()
                .instance()
                .get(&DataKey::FeeRecipient)
                .unwrap();
            usdc.transfer(&env.current_contract_address(), &fee_recipient, &fee);
        }

        let pos_id = next_position_id(&env);
        let position = OptionPosition {
            position_id: pos_id,
            series_id,
            owner: buyer.clone(),
            side: PositionSide::Long,
            contracts,
            premium_paid: total_premium,
            fee_paid: fee,
            collateral_locked: 0,
            is_exercised: false,
            is_settled: false,
            opened_at: env.ledger().timestamp(),
        };
        save_position(&env, &position);
        add_user_position(&env, &buyer, pos_id);
        count_position_opened(&env, series_id, pos_id);

        series.open_interest = series.open_interest.checked_add(contracts).unwrap();
        save_series(&env, &series);

        add_instance_i128(&env, &DataKey::TotalPremiumsCollected, premium_after_fee);
        // Funds the pool writers get paid out of — see write_option.
        add_instance_i128(&env, &DataKey::PremiumPool, premium_after_fee);
        // This position's refund-eligible principal, see SeriesEscrow.
        add_series_escrow(&env, series_id, premium_after_fee);

        events::option_bought(&env, buyer, pos_id, series_id, contracts, total_premium);

        pos_id
    }

    // ── Writing Options (Short / Covered) ─────────────────────────────────────

    /// Writes `contracts` of a series, locking collateral (notional for
    /// calls, 110% of strike for puts) and receiving the premium net of
    /// fee from the buyer-funded pool. `min_premium` is slippage
    /// protection (0 opts out). Returns the new position id.
    //
    // The writer's premium is drawn from PremiumPool, which buyers fund —
    // not the raw token balance, which would let a writer's own collateral
    // fund their "premium" and leave the contract unable to return it.
    pub fn write_option(
        env: Env,
        writer: Address,
        series_id: u64,
        contracts: i128,
        collateral_amount: i128,
        min_premium: i128,
    ) -> u64 {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        writer.require_auth();

        if contracts <= 0 {
            panic_with_error!(&env, Error::ZeroContracts);
        }

        let mut series = require_active_series(&env, series_id);

        let required_collateral = match series.option_type {
            // Covered call: notional at the last recorded underlying price.
            OptionType::Call => {
                let underlying_price: i128 =
                    ttl::get_persistent(&env, &DataKey::UnderlyingPrice(series.underlying.clone()))
                        .unwrap_or(series.strike_price);
                contracts
                    .checked_mul(underlying_price)
                    .unwrap()
                    .checked_div(PRICE_PRECISION)
                    .unwrap()
            }
            // Cash-secured put: strike × contracts × 110%.
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

        let usdc = usdc(&env);
        usdc.transfer(
            &writer,
            &env.current_contract_address(),
            &required_collateral,
        );

        let total_premium = premium_for(&series, contracts);
        let fee = calc_fee(total_premium, fee_rate_bps(&env));
        let writer_premium = total_premium - fee;
        if writer_premium < min_premium {
            panic_with_error!(&env, Error::PremiumBelowMinimum);
        }

        let pool: i128 = env
            .storage()
            .instance()
            .get(&DataKey::PremiumPool)
            .unwrap_or(0);
        if pool < writer_premium {
            panic_with_error!(&env, Error::InsufficientPremiumPool);
        }
        env.storage().instance().set(
            &DataKey::PremiumPool,
            &(pool.checked_sub(writer_premium).unwrap()),
        );

        usdc.transfer(&env.current_contract_address(), &writer, &writer_premium);

        let pos_id = next_position_id(&env);
        let position = OptionPosition {
            position_id: pos_id,
            series_id,
            owner: writer.clone(),
            side: PositionSide::Short,
            contracts,
            premium_paid: writer_premium,
            fee_paid: 0,
            collateral_locked: required_collateral,
            is_exercised: false,
            is_settled: false,
            opened_at: env.ledger().timestamp(),
        };
        save_position(&env, &position);
        add_user_position(&env, &writer, pos_id);
        count_position_opened(&env, series_id, pos_id);

        series.open_interest = series.open_interest.checked_add(contracts).unwrap();
        save_series(&env, &series);

        add_series_escrow(&env, series_id, required_collateral);

        events::option_written(
            &env,
            writer,
            pos_id,
            series_id,
            contracts,
            writer_premium,
            required_collateral,
        );

        pos_id
    }

    // ── Position Management ───────────────────────────────────────────────────

    /// Moves an open position from `from` (who signs and owns it) to `to`,
    /// with every right attached to it. `to == from` is a no-op.
    //
    // Shorts are transferable too: their collateral is fully locked here,
    // so `to` only gains the right to reclaim what's left after settlement,
    // not an unfunded obligation.
    pub fn transfer_position(env: Env, from: Address, to: Address, position_id: u64) {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        from.require_auth();

        let mut position = Self::load_open_position(&env, &from, position_id);
        if to == from {
            return;
        }

        remove_user_position(&env, &from, position_id);
        add_user_position(&env, &to, position_id);
        position.owner = to.clone();
        save_position(&env, &position);

        events::position_transferred(&env, from, to, position_id);
    }

    /// Splits `split_contracts` (0 < split < contracts) off an open
    /// position into a new one with the same series, side and owner.
    /// Premium, fee and collateral are divided pro rata, rounded down for
    /// the new position. Returns the new position id.
    //
    // Every field's total across the two positions equals the original, so
    // open interest and SeriesEscrow are unchanged. Payouts are computed
    // per position, so two halves can pay (or keep) at most one unit
    // less (more) than the unsplit position would.
    pub fn split_position(
        env: Env,
        owner: Address,
        position_id: u64,
        split_contracts: i128,
    ) -> u64 {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        owner.require_auth();

        let mut position = Self::load_open_position(&env, &owner, position_id);
        if split_contracts <= 0 || split_contracts >= position.contracts {
            panic_with_error!(&env, Error::InvalidSplitAmount);
        }

        let share = |amount: i128| -> i128 {
            amount
                .checked_mul(split_contracts)
                .unwrap()
                .checked_div(position.contracts)
                .unwrap()
        };
        let premium_part = share(position.premium_paid);
        let fee_part = share(position.fee_paid);
        let collateral_part = share(position.collateral_locked);

        let new_id = next_position_id(&env);
        let new_position = OptionPosition {
            position_id: new_id,
            series_id: position.series_id,
            owner: owner.clone(),
            side: position.side.clone(),
            contracts: split_contracts,
            premium_paid: premium_part,
            fee_paid: fee_part,
            collateral_locked: collateral_part,
            is_exercised: false,
            is_settled: false,
            opened_at: position.opened_at,
        };

        position.contracts = position.contracts.checked_sub(split_contracts).unwrap();
        position.premium_paid = position.premium_paid.checked_sub(premium_part).unwrap();
        position.fee_paid = position.fee_paid.checked_sub(fee_part).unwrap();
        position.collateral_locked = position
            .collateral_locked
            .checked_sub(collateral_part)
            .unwrap();

        save_position(&env, &position);
        save_position(&env, &new_position);
        add_user_position(&env, &owner, new_id);
        count_position_opened(&env, position.series_id, new_id);

        events::position_split(&env, owner, position_id, new_id, split_contracts);
        new_id
    }

    fn load_open_position(env: &Env, owner: &Address, position_id: u64) -> OptionPosition {
        let position = load_position(env, position_id);
        if position.owner != *owner {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.is_exercised {
            panic_with_error!(env, Error::AlreadyExercised);
        }
        if position.is_settled {
            panic_with_error!(env, Error::AlreadySettled);
        }
        position
    }

    // ── Exercise ──────────────────────────────────────────────────────────────

    /// Exercises an ITM long after settlement, within the exercise window.
    pub fn exercise(env: Env, owner: Address, position_id: u64) {
        ttl::extend_instance(&env);
        owner.require_auth();
        Self::exercise_one(&env, &owner, position_id, &usdc(&env));
    }

    /// `exercise` for up to MAX_BATCH_SIZE positions, all-or-nothing.
    /// Returns the summed payout.
    pub fn exercise_batch(env: Env, owner: Address, position_ids: Vec<u64>) -> i128 {
        ttl::extend_instance(&env);
        owner.require_auth();
        require_batch(&env, &position_ids);
        let usdc = usdc(&env);

        let mut total_payout: i128 = 0;
        for position_id in position_ids.iter() {
            let payout = Self::exercise_one(&env, &owner, position_id, &usdc);
            total_payout = total_payout.checked_add(payout).unwrap();
        }
        total_payout
    }

    /// Permissionless auto-exercise of an ITM long; the payout always goes
    /// to the position owner.
    pub fn settle_long(env: Env, position_id: u64) -> i128 {
        ttl::extend_instance(&env);
        require_not_paused(&env);
        let position = load_position(&env, position_id);
        let payout = Self::exercise_one(&env, &position.owner, position_id, &usdc(&env));

        let series = load_series(&env, position.series_id);
        let settlement_price = series.settlement_price.unwrap_or(0);
        events::option_auto_exercised(&env, position.owner, position_id, settlement_price, payout);

        payout
    }

    // Shared core of exercise/exercise_batch/settle_long. The caller has
    // already checked `owner`'s authorization (once per batch).
    fn exercise_one(env: &Env, owner: &Address, position_id: u64, usdc: &token::Client) -> i128 {
        let mut position = load_position(env, position_id);
        if position.owner != *owner {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.side != PositionSide::Long {
            panic_with_error!(env, Error::WrongSide);
        }
        if position.is_exercised {
            panic_with_error!(env, Error::AlreadyExercised);
        }

        let series = load_series(env, position.series_id);

        // European: exercise only after expiry, within the window.
        let now = env.ledger().timestamp();
        if now < series.expiry {
            panic_with_error!(env, Error::SeriesNotExpired);
        }
        if now > series.expiry + settlement_window(env) {
            panic_with_error!(env, Error::ExerciseWindowClosed);
        }

        let settlement_price = series
            .settlement_price
            .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet));

        let payout = series_payout(&series, settlement_price, position.contracts);
        if payout <= 0 {
            panic_with_error!(env, Error::NotInTheMoney);
        }

        usdc.transfer(&env.current_contract_address(), owner, &payout);

        position.is_exercised = true;
        save_position(env, &position);
        count_position_closed(env, position.series_id, position_id);

        deduct_orphaned_liability(env, payout);

        events::option_exercised(env, owner.clone(), position_id, settlement_price, payout);
        payout
    }

    // ── Settlement ────────────────────────────────────────────────────────────

    /// The trusted oracle address sets an expired Active series' price and
    /// flips it to Settled. Rejects a Cancelled (SeriesNotActive) or
    /// already Settled (AlreadySettled) series.
    pub fn set_settlement_price(env: Env, series_id: u64, price: i128) {
        ttl::extend_instance(&env);
        let oracle: Address = env.storage().instance().get(&DataKey::Oracle).unwrap();
        oracle.require_auth();
        Self::settle_inner(&env, series_id, None, price);
    }

    /// Permissionless: settles an expired series at the price a live
    /// price_oracle deployment reports.
    //
    // No require_auth — the price is already backed by that contract's own
    // feeder-authenticated aggregate.
    pub fn set_settlement_price_from_oracle(env: Env, series_id: u64, oracle_contract: Address) {
        ttl::extend_instance(&env);
        Self::settle_inner(&env, series_id, Some(oracle_contract), 0);
    }

    // Settling is one-shot: re-settling would move the price under
    // positions that already exercised or reclaimed, and settling a
    // Cancelled series would strand its refunds.
    fn settle_inner(env: &Env, series_id: u64, oracle_contract: Option<Address>, price: i128) {
        let mut series = load_series(env, series_id);

        let now = env.ledger().timestamp();
        if now < series.expiry {
            panic_with_error!(env, Error::SeriesNotExpired);
        }
        match series.state {
            SeriesState::Settled => panic_with_error!(env, Error::AlreadySettled),
            SeriesState::Cancelled => panic_with_error!(env, Error::SeriesNotActive),
            SeriesState::Active | SeriesState::Expired => {}
        }

        let price = match oracle_contract {
            None => price,
            Some(oracle_contract) => price_oracle_client::Client::new(env, &oracle_contract)
                .get_price(&series.underlying)
                .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet)),
        };

        series.settlement_price = Some(price);
        series.state = SeriesState::Settled;
        save_series(env, &series);
        ttl::set_persistent(env, &DataKey::SeriesClosedAt(series_id), &now);
        ttl::set_persistent(
            env,
            &DataKey::UnderlyingPrice(series.underlying.clone()),
            &price,
        );

        events::settlement_price_set(env, series_id, price);
    }

    // ── Reclaim ───────────────────────────────────────────────────────────────

    /// Returns a writer's locked collateral minus the max loss owed to
    /// longs, after settlement.
    pub fn reclaim_collateral(env: Env, writer: Address, position_id: u64) {
        ttl::extend_instance(&env);
        writer.require_auth();
        Self::reclaim_one(&env, &writer, position_id, &usdc(&env));
    }

    /// `reclaim_collateral` for up to MAX_BATCH_SIZE positions,
    /// all-or-nothing. Returns the summed reclaim.
    pub fn reclaim_batch(env: Env, writer: Address, position_ids: Vec<u64>) -> i128 {
        ttl::extend_instance(&env);
        writer.require_auth();
        require_batch(&env, &position_ids);
        let usdc = usdc(&env);

        let mut total_reclaim: i128 = 0;
        for position_id in position_ids.iter() {
            let reclaim = Self::reclaim_one(&env, &writer, position_id, &usdc);
            total_reclaim = total_reclaim.checked_add(reclaim).unwrap();
        }
        total_reclaim
    }

    fn reclaim_one(env: &Env, writer: &Address, position_id: u64, usdc: &token::Client) -> i128 {
        let mut position = load_position(env, position_id);
        if position.owner != *writer {
            panic_with_error!(env, Error::Unauthorized);
        }
        if position.side != PositionSide::Short {
            panic_with_error!(env, Error::WrongSide);
        }
        if position.is_settled {
            panic_with_error!(env, Error::AlreadySettled);
        }

        let series = load_series(env, position.series_id);
        if series.state != SeriesState::Settled {
            panic_with_error!(env, Error::SeriesNotExpired);
        }

        let settlement_price = series
            .settlement_price
            .unwrap_or_else(|| panic_with_error!(env, Error::PriceNotSet));

        // Collateral consumed by what longs can claim against this short.
        let max_loss = series_payout(&series, settlement_price, position.contracts);
        let reclaim = position
            .collateral_locked
            .checked_sub(max_loss)
            .unwrap()
            .max(0);

        if reclaim > 0 {
            usdc.transfer(&env.current_contract_address(), writer, &reclaim);
        }
        add_orphaned_liability(env, max_loss);

        position.is_settled = true;
        save_position(env, &position);
        count_position_closed(env, position.series_id, position_id);

        events::collateral_reclaimed(env, writer.clone(), position_id, reclaim);
        reclaim
    }

    /// Admin sweeps an unexercised ITM long's payout to the fee recipient
    /// once the 90-day forfeiture window has passed.
    pub fn sweep_forfeited(env: Env, position_id: u64) -> i128 {
        ttl::extend_instance(&env);
        let admin = zenith_common::get_admin(&env, &DataKey::Admin);
        admin.require_auth();

        let mut position = load_position(&env, position_id);
        if position.side != PositionSide::Long {
            panic_with_error!(&env, Error::WrongSide);
        }
        if position.is_exercised {
            panic_with_error!(&env, Error::AlreadyExercised);
        }

        let series = load_series(&env, position.series_id);
        if series.state != SeriesState::Settled {
            panic_with_error!(&env, Error::SeriesNotExpired);
        }
        if env.ledger().timestamp() <= series.expiry + FORFEITURE_WINDOW {
            panic_with_error!(&env, Error::NotEligibleForForfeiture);
        }

        let settlement_price = series
            .settlement_price
            .unwrap_or_else(|| panic_with_error!(&env, Error::PriceNotSet));
        let payout = series_payout(&series, settlement_price, position.contracts);

        if payout > 0 {
            let fee_recipient: Address = env
                .storage()
                .instance()
                .get(&DataKey::FeeRecipient)
                .unwrap_or_else(|| admin.clone());
            usdc(&env).transfer(&env.current_contract_address(), &fee_recipient, &payout);
        }

        position.is_exercised = true;
        save_position(&env, &position);
        count_position_closed(&env, position.series_id, position_id);

        deduct_orphaned_liability(&env, payout);
        events::payout_forfeited(&env, position.owner.clone(), position_id, payout);

        payout
    }

    /// Running total of unexercised ITM obligations across settled positions.
    pub fn get_orphaned_liabilities(env: Env) -> i128 {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::OrphanedLiabilities)
            .unwrap_or(0)
    }

    /// Admin initializes/synchronizes historical orphaned liabilities.
    pub fn sync_orphaned_liabilities(env: Env, amount: i128) {
        ttl::extend_instance(&env);
        authorize(&env, &Auth::Admin, ActionClass::Standard);
        env.storage()
            .instance()
            .set(&DataKey::OrphanedLiabilities, &amount);
    }

    // ── Pruning and the active-series cap ─────────────────────────────────────

    /// Permissionless. Removes up to MAX_BATCH_SIZE terminal positions
    /// whose series' retention period (PRUNE_RETENTION) has passed, and
    /// their owners' index entries, emitting `position_pruned` with the
    /// full record. All-or-nothing. Returns how many were pruned.
    pub fn prune_positions(env: Env, position_ids: Vec<u64>) -> u32 {
        ttl::extend_instance(&env);
        require_migrated(&env);
        require_batch(&env, &position_ids);

        for position_id in position_ids.iter() {
            let position = load_position(&env, position_id);
            let series = load_series(&env, position.series_id);

            // Terminal and liability-free first, so a live position always
            // reports NotPrunable rather than RetentionNotElapsed.
            let anchor = retention_anchor(&env, &series);
            if !is_position_terminal(&series, &position) {
                panic_with_error!(&env, Error::NotPrunable);
            }
            require_retention_elapsed(&env, anchor);

            env.storage()
                .persistent()
                .remove(&DataKey::Position(position_id));
            remove_user_position(&env, &position.owner, position_id);
            // An OTM long is pruned without ever having been closed.
            let was_open = !(position.is_settled || position.is_exercised);
            count_position_pruned(&env, position.series_id, was_open);

            events::position_pruned(&env, position);
        }
        position_ids.len()
    }

    /// Permissionless. Removes a terminal series once every one of its
    /// positions has been pruned and its retention period has passed,
    /// along with its escrow, spec index and counters, freeing its
    /// active-series slot and its spec for relisting. Emits
    /// `series_pruned` with the full record.
    pub fn prune_series(env: Env, series_id: u64) {
        ttl::extend_instance(&env);
        require_migrated(&env);

        let series = load_series(&env, series_id);
        let anchor = retention_anchor(&env, &series);
        if series_counts(&env, series_id).live > 0 {
            panic_with_error!(&env, Error::SeriesHasPositions);
        }
        require_retention_elapsed(&env, anchor);

        // Pruning always frees the slot, whichever rule would have.
        if mark_slot_released(&env, &series) {
            events::series_slot_released(
                &env,
                series.underlying.clone(),
                series_id,
                active_series_count(&env, &series.underlying),
            );
        }

        let persistent = env.storage().persistent();
        persistent.remove(&DataKey::Series(series_id));
        persistent.remove(&DataKey::SeriesEscrow(series_id));
        persistent.remove(&DataKey::SeriesPositions(series_id));
        persistent.remove(&DataKey::SeriesClosedAt(series_id));
        persistent.remove(&DataKey::SeriesReleased(series_id));
        persistent.remove(&DataKey::SeriesIndex(
            series.underlying.clone(),
            series.option_type.clone(),
            series.strike_price,
            series.expiry,
        ));

        events::series_pruned(&env, series);
    }

    /// Permissionless. Frees a series' active-series slot if it no longer
    /// needs one (Settled with the exercise window closed, or Cancelled
    /// and fully refunded). Returns whether it released a slot.
    pub fn release_series_slot(env: Env, series_id: u64) -> bool {
        ttl::extend_instance(&env);
        require_migrated(&env);
        try_release_slot(&env, &load_series(&env, series_id))
    }

    /// Permissionless, bounded migration after upgrading from a version
    /// without active-series counters. Reads at most `limit`
    /// (1..=MAX_PAGE_SCAN) entries per call: first every position (to
    /// rebuild per-series counts), then every series (to rebuild
    /// ActiveSeriesCount). Returns true once complete; create_series,
    /// pruning and slot releases are blocked until then.
    //
    // Positions/series above the cursors are counted by the scan, those at
    // or below by live code, so concurrent trading can't double count.
    // A scanned series that becomes releasable before the scan finishes
    // keeps its slot until someone calls release_series_slot.
    pub fn migrate_counters(env: Env, limit: u32) -> bool {
        ttl::extend_instance(&env);
        if limit == 0 || limit > MAX_PAGE_SCAN {
            panic_with_error!(&env, Error::InvalidPageLimit);
        }
        let mut state = migration_state(&env);
        if state.series_cursor == u64::MAX {
            return true;
        }
        let mut budget = limit;

        if state.position_cursor != u64::MAX {
            let last: u64 = env
                .storage()
                .instance()
                .get(&DataKey::PositionCounter)
                .unwrap_or(0);
            while budget > 0 && state.position_cursor < last {
                let id = state.position_cursor + 1;
                let position: Option<OptionPosition> =
                    ttl::get_persistent(&env, &DataKey::Position(id));
                if let Some(position) = position {
                    let mut counts = series_counts(&env, position.series_id);
                    counts.live = counts.live.checked_add(1).unwrap();
                    if !(position.is_settled || position.is_exercised) {
                        counts.open = counts.open.checked_add(1).unwrap();
                    }
                    set_series_counts(&env, position.series_id, &counts);
                }
                state.position_cursor = id;
                budget -= 1;
            }
            if state.position_cursor >= last {
                state.position_cursor = u64::MAX;
            }
        }

        if state.position_cursor == u64::MAX {
            let last: u64 = env
                .storage()
                .instance()
                .get(&DataKey::SeriesCounter)
                .unwrap_or(0);
            while budget > 0 && state.series_cursor < last {
                let id = state.series_cursor + 1;
                let series: Option<OptionSeries> = ttl::get_persistent(&env, &DataKey::Series(id));
                if let Some(series) = series {
                    if is_slot_releasable(&env, &series, &series_counts(&env, id)) {
                        ttl::set_persistent(&env, &DataKey::SeriesReleased(id), &true);
                    } else {
                        let active = active_series_count(&env, &series.underlying);
                        set_active_series_count(
                            &env,
                            &series.underlying,
                            active.checked_add(1).unwrap(),
                        );
                    }
                }
                state.series_cursor = id;
                budget -= 1;
            }
            if state.series_cursor >= last {
                state.series_cursor = u64::MAX;
            }
        }

        set_migration_state(&env, &state);
        let done = state.series_cursor == u64::MAX;
        if done {
            events::counters_migrated(&env);
        }
        done
    }

    // ── Views ─────────────────────────────────────────────────────────────────

    pub fn get_admin(env: Env) -> Address {
        ttl::extend_instance(&env);
        zenith_common::get_admin(&env, &DataKey::Admin)
    }

    pub fn is_paused(env: Env) -> bool {
        ttl::extend_instance(&env);
        zenith_common::is_paused(&env, &DataKey::Paused)
    }

    pub fn get_fee_rate(env: Env) -> i128 {
        ttl::extend_instance(&env);
        fee_rate_bps(&env)
    }

    pub fn get_premium_pool(env: Env) -> i128 {
        ttl::extend_instance(&env);
        env.storage()
            .instance()
            .get(&DataKey::PremiumPool)
            .unwrap_or(0)
    }

    /// Lifetime number of series ever listed on `underlying` (not a cap).
    pub fn get_series_count_for_underlying(env: Env, underlying: Symbol) -> u32 {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::SeriesCountForUnderlying(underlying)).unwrap_or(0)
    }

    /// Series on `underlying` currently holding an active-series slot.
    pub fn get_active_series_count(env: Env, underlying: Symbol) -> u32 {
        ttl::extend_instance(&env);
        active_series_count(&env, &underlying)
    }

    pub fn get_max_active_series(env: Env) -> u32 {
        ttl::extend_instance(&env);
        max_active_series(&env)
    }

    pub fn get_series_position_counts(env: Env, series_id: u64) -> SeriesPositionCounts {
        ttl::extend_instance(&env);
        series_counts(&env, series_id)
    }

    pub fn get_migration_state(env: Env) -> MigrationState {
        ttl::extend_instance(&env);
        migration_state(&env)
    }

    pub fn is_migrated(env: Env) -> bool {
        ttl::extend_instance(&env);
        migration_done(&env)
    }

    /// Series id listed for this exact spec, or `None` (also after the
    /// series was pruned).
    pub fn get_series_id(
        env: Env,
        underlying: Symbol,
        option_type: OptionType,
        strike_price: i128,
        expiry: u64,
    ) -> Option<u64> {
        ttl::extend_instance(&env);
        ttl::get_persistent(
            &env,
            &DataKey::SeriesIndex(underlying, option_type, strike_price, expiry),
        )
    }

    /// `None` for an unknown or pruned id; see `get_series_status`.
    pub fn get_series(env: Env, series_id: u64) -> Option<OptionSeries> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::Series(series_id))
    }

    /// `Live(series)`, `Pruned` for an allocated id whose entry was
    /// pruned, or `None` for an id never allocated.
    pub fn get_series_status(env: Env, series_id: u64) -> SeriesStatus {
        ttl::extend_instance(&env);
        if let Some(series) = ttl::get_persistent(&env, &DataKey::Series(series_id)) {
            return SeriesStatus::Live(series);
        }
        let counter: u64 = env
            .storage()
            .instance()
            .get(&DataKey::SeriesCounter)
            .unwrap_or(0);
        if (1..=counter).contains(&series_id) {
            SeriesStatus::Pruned
        } else {
            SeriesStatus::None
        }
    }

    /// `None` for an unknown or pruned id; see `get_position_status`.
    pub fn get_position(env: Env, position_id: u64) -> Option<OptionPosition> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::Position(position_id))
    }

    /// `Live(position)`, `Pruned` for an allocated id whose entry was
    /// pruned, or `None` for an id never allocated.
    pub fn get_position_status(env: Env, position_id: u64) -> PositionStatus {
        ttl::extend_instance(&env);
        if let Some(position) = ttl::get_persistent(&env, &DataKey::Position(position_id)) {
            return PositionStatus::Live(position);
        }
        let counter: u64 = env
            .storage()
            .instance()
            .get(&DataKey::PositionCounter)
            .unwrap_or(0);
        if (1..=counter).contains(&position_id) {
            PositionStatus::Pruned
        } else {
            PositionStatus::None
        }
    }

    /// `user`'s unpruned position ids.
    pub fn get_user_positions(env: Env, user: Address) -> Vec<u64> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::UserPositions(user)).unwrap_or_else(|| Vec::new(&env))
    }

    pub fn get_underlying_price(env: Env, underlying: Symbol) -> Option<i128> {
        ttl::extend_instance(&env);
        ttl::get_persistent(&env, &DataKey::UnderlyingPrice(underlying))
    }

    /// (total premiums collected, total open interest, series count).
    pub fn get_stats(env: Env) -> (i128, i128, u64) {
        ttl::extend_instance(&env);
        let instance = env.storage().instance();
        let premiums: i128 = instance.get(&DataKey::TotalPremiumsCollected).unwrap_or(0);
        let oi: i128 = instance.get(&DataKey::TotalOpenInterest).unwrap_or(0);
        let series_count: u64 = instance.get(&DataKey::SeriesCounter).unwrap_or(0);
        (premiums, oi, series_count)
    }
}
