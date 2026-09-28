#![no_std]

//! Zenith Price Oracle — on-chain price feed for the options_market contract.
//!
//! options_market currently trusts a bare `oracle: Address` with no on-chain
//! logic behind it at all — this contract is that logic: a small set of
//! admin-authorized feeders report prices per symbol, and the aggregate
//! (median across fresh reports) is what callers read.

use soroban_sdk::{contract, contractimpl, panic_with_error, Address, Env, Symbol, Vec};
use zenith_common::{require_admin_or_multisig, ActionClass, Auth};

#[cfg(test)]
mod test;

mod error;
mod events;
mod math;
mod types;

use error::Error;
use math::{median, MAX_FEEDERS};
use types::DataKey;

const DEFAULT_MAX_STALENESS: u64 = 3600; // 1 hour
const DEFAULT_MIN_REPORTS: u32 = 1; // any single fresh report is enough, the original behavior

fn require_not_paused(env: &Env) {
    zenith_common::require_not_paused(env, &DataKey::Paused, Error::ContractPaused);
}

fn require_auth(env: &Env, auth: &Auth, class: ActionClass) {
    require_admin_or_multisig(env, &DataKey::Admin, auth, class, Error::Unauthorized);
}

#[contract]
pub struct PriceOracle;

impl PriceOracle {
    fn set_max_staleness_inner(env: Env, auth: Auth, seconds: u64) {
        require_auth(&env, &auth, ActionClass::Standard);
        if seconds == 0 {
            panic_with_error!(&env, Error::InvalidStaleness);
        }
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &seconds);
        events::max_staleness_updated(&env, seconds);
    }

    fn set_min_reports_inner(env: Env, auth: Auth, count: u32) {
        require_auth(&env, &auth, ActionClass::Standard);
        if count == 0 {
            panic_with_error!(&env, Error::InvalidMinReports);
        }
        env.storage().instance().set(&DataKey::MinReports, &count);
        events::min_reports_updated(&env, count);
    }

    fn transfer_admin_inner(env: Env, auth: Auth, new_admin: Address) {
        require_auth(&env, &auth, ActionClass::Critical);
        let admin = zenith_common::set_admin(&env, &DataKey::Admin, &new_admin);
        events::admin_transferred(&env, admin, new_admin);
    }

    fn set_paused_inner(env: Env, auth: Auth, paused: bool) {
        let class = if paused {
            ActionClass::Emergency
        } else {
            ActionClass::Standard
        };
        require_auth(&env, &auth, class);
        zenith_common::set_paused(&env, &DataKey::Paused, paused);
        if paused {
            events::paused(&env);
        } else {
            events::unpaused(&env);
        }
    }

    fn add_feeder_inner(env: Env, auth: Auth, feeder: Address) {
        require_not_paused(&env);
        require_auth(&env, &auth, ActionClass::Standard);

        let mut feeders: Vec<Address> = env.storage().instance().get(&DataKey::Feeders).unwrap();
        if feeders.contains(&feeder) {
            panic_with_error!(&env, Error::FeederAlreadyAdded);
        }
        if feeders.len() >= MAX_FEEDERS {
            panic_with_error!(&env, Error::TooManyFeeders);
        }
        feeders.push_back(feeder.clone());
        env.storage().instance().set(&DataKey::Feeders, &feeders);
        events::feeder_added(&env, feeder);
    }

    fn remove_feeder_inner(env: Env, auth: Auth, feeder: Address) {
        require_not_paused(&env);
        require_auth(&env, &auth, ActionClass::Standard);

        let mut feeders: Vec<Address> = env.storage().instance().get(&DataKey::Feeders).unwrap();
        match feeders.first_index_of(&feeder) {
            Some(i) => {
                feeders.remove(i).unwrap();
                env.storage().instance().set(&DataKey::Feeders, &feeders);
                events::feeder_removed(&env, feeder);
            }
            None => panic_with_error!(&env, Error::FeederNotFound),
        }
    }
}

#[contractimpl]
impl PriceOracle {
    pub fn initialize(env: Env, admin: Address) {
        if env.storage().instance().has(&DataKey::Admin) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        admin.require_auth();
        env.storage().instance().set(&DataKey::Admin, &admin);
        env.storage()
            .instance()
            .set(&DataKey::Feeders, &Vec::<Address>::new(&env));
        env.storage()
            .instance()
            .set(&DataKey::MaxStaleness, &DEFAULT_MAX_STALENESS);
        env.storage()
            .instance()
            .set(&DataKey::MinReports, &DEFAULT_MIN_REPORTS);
    }

    /// Admin adjusts how old a feeder's report can be and still count
    /// toward get_price's aggregate.
    pub fn set_max_staleness(env: Env, seconds: u64) {
        Self::set_max_staleness_inner(env, Auth::Admin, seconds);
    }

    /// Permissionless alternative to set_max_staleness: cross-calls a
    /// deployed Multisig and checks is_executable(action_id, class) instead of
    /// requiring the admin's own signature. Worth gating the same way as
    /// pause/transfer_admin — a compromised admin widening max_staleness
    /// is a quiet way to make get_price accept increasingly stale, and
    /// therefore increasingly manipulable, prices.
    pub fn set_max_staleness_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        seconds: u64,
    ) {
        Self::set_max_staleness_inner(env, Auth::Multisig(multisig_contract, action_id), seconds);
    }

    pub fn get_max_staleness(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::MaxStaleness)
            .unwrap()
    }

    /// Admin adjusts how many CURRENTLY-fresh feeder reports get_price
    /// requires before it returns an aggregate at all. Without this,
    /// get_price will happily return a "median" backed by a single fresh
    /// report the moment every other feeder's report goes stale (or is
    /// removed) — that one feeder then fully determines the price with
    /// no averaging effect at all, defeating the point of aggregating
    /// across multiple feeders in the first place.
    pub fn set_min_reports(env: Env, count: u32) {
        Self::set_min_reports_inner(env, Auth::Admin, count);
    }

    /// Permissionless alternative to set_min_reports: cross-calls a
    /// deployed Multisig and checks is_executable(action_id, class) instead of
    /// requiring the admin's own signature. Worth gating the same way as
    /// set_max_staleness_via_multisig — a compromised admin lowering
    /// min_reports back to 1 is a quiet way to make get_price trust a
    /// single feeder again.
    pub fn set_min_reports_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        count: u32,
    ) {
        Self::set_min_reports_inner(env, Auth::Multisig(multisig_contract, action_id), count);
    }

    pub fn get_min_reports(env: Env) -> u32 {
        env.storage().instance().get(&DataKey::MinReports).unwrap()
    }

    pub fn get_admin(env: Env) -> Address {
        zenith_common::get_admin(&env, &DataKey::Admin)
    }

    /// Admin hands off control to a new address. Requires the CURRENT
    /// admin's signature, not the incoming one.
    pub fn transfer_admin(env: Env, new_admin: Address) {
        Self::transfer_admin_inner(env, Auth::Admin, new_admin);
    }

    /// Permissionless alternative to transfer_admin: cross-calls a
    /// deployed Multisig and checks is_executable(action_id, class) instead of
    /// requiring the current admin's own signature — same pattern as
    /// options_market's transfer_admin_via_multisig.
    pub fn transfer_admin_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        new_admin: Address,
    ) {
        Self::transfer_admin_inner(env, Auth::Multisig(multisig_contract, action_id), new_admin);
    }

    /// Emergency stop: blocks add_feeder, remove_feeder, and report_price.
    /// Does NOT block get_price or any other view — a pause should freeze
    /// further changes to the feed, not hide the last-known price from
    /// callers still reading it.
    pub fn pause(env: Env) {
        Self::set_paused_inner(env, Auth::Admin, true);
    }

    pub fn unpause(env: Env) {
        Self::set_paused_inner(env, Auth::Admin, false);
    }

    /// Permissionless alternative to pause(), same rationale and pattern
    /// as options_market's pause_via_multisig.
    pub fn pause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        Self::set_paused_inner(env, Auth::Multisig(multisig_contract, action_id), true);
    }

    pub fn unpause_via_multisig(env: Env, multisig_contract: Address, action_id: u64) {
        Self::set_paused_inner(env, Auth::Multisig(multisig_contract, action_id), false);
    }

    pub fn is_paused(env: Env) -> bool {
        zenith_common::is_paused(&env, &DataKey::Paused)
    }

    /// Admin authorizes a new price feeder. Feeders are the only addresses
    /// allowed to call report_price.
    pub fn add_feeder(env: Env, feeder: Address) {
        Self::add_feeder_inner(env, Auth::Admin, feeder);
    }

    /// Permissionless alternative to add_feeder: cross-calls a deployed
    /// Multisig and checks is_executable(action_id, class) instead of requiring
    /// the admin's own signature. Adding a feeder expands who can move
    /// get_price's aggregate, so gating it behind M-of-N is at least as
    /// warranted as pause.
    pub fn add_feeder_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        feeder: Address,
    ) {
        Self::add_feeder_inner(env, Auth::Multisig(multisig_contract, action_id), feeder);
    }

    /// Admin revokes a feeder's authorization. Their most recent price
    /// report is left in storage (for audit purposes) but is no longer
    /// counted toward the aggregate once removed.
    pub fn remove_feeder(env: Env, feeder: Address) {
        Self::remove_feeder_inner(env, Auth::Admin, feeder);
    }

    /// Permissionless alternative to remove_feeder: cross-calls a
    /// deployed Multisig and checks is_executable(action_id, class) instead of
    /// requiring the admin's own signature. Same rationale as
    /// add_feeder_via_multisig.
    pub fn remove_feeder_via_multisig(
        env: Env,
        multisig_contract: Address,
        action_id: u64,
        feeder: Address,
    ) {
        Self::remove_feeder_inner(env, Auth::Multisig(multisig_contract, action_id), feeder);
    }

    pub fn is_feeder(env: Env, address: Address) -> bool {
        let feeders: Vec<Address> = env.storage().instance().get(&DataKey::Feeders).unwrap();
        feeders.contains(&address)
    }

    pub fn get_feeder_count(env: Env) -> u32 {
        let feeders: Vec<Address> = env.storage().instance().get(&DataKey::Feeders).unwrap();
        feeders.len()
    }

    /// A feeder reports the current price for `symbol`. Only records this
    /// feeder's own report — see the aggregation step (still to come) for
    /// how per-feeder reports become the single price callers read.
    pub fn report_price(env: Env, feeder: Address, symbol: Symbol, price: i128) {
        require_not_paused(&env);
        feeder.require_auth();

        let feeders: Vec<Address> = env.storage().instance().get(&DataKey::Feeders).unwrap();
        if !feeders.contains(&feeder) {
            panic_with_error!(&env, Error::NotAFeeder);
        }
        if price <= 0 {
            panic_with_error!(&env, Error::InvalidPrice);
        }

        let now = env.ledger().timestamp();
        env.storage().persistent().set(
            &DataKey::PriceReport(symbol.clone(), feeder.clone()),
            &(price, now),
        );
        events::price_reported(&env, feeder, symbol, price);
    }

    pub fn get_latest_report(env: Env, symbol: Symbol, feeder: Address) -> Option<(i128, u64)> {
        env.storage()
            .persistent()
            .get(&DataKey::PriceReport(symbol, feeder))
    }

    /// The aggregate price for `symbol`: the median across every
    /// CURRENTLY authorized feeder's report that isn't older than
    /// max_staleness. Returns None if no feeder has a fresh report —
    /// callers must not treat that the same as "price is zero."
    pub fn get_price(env: Env, symbol: Symbol) -> Option<i128> {
        let feeders: Vec<Address> = env.storage().instance().get(&DataKey::Feeders).unwrap();
        let max_staleness: u64 = env
            .storage()
            .instance()
            .get(&DataKey::MaxStaleness)
            .unwrap();
        let now = env.ledger().timestamp();

        let mut buffer = [0i128; MAX_FEEDERS as usize];
        let mut count = 0usize;
        for feeder in feeders.iter() {
            let report: Option<(i128, u64)> = env
                .storage()
                .persistent()
                .get(&DataKey::PriceReport(symbol.clone(), feeder));
            if let Some((price, reported_at)) = report {
                if now.checked_sub(reported_at).unwrap_or(u64::MAX) <= max_staleness {
                    buffer[count] = price;
                    count += 1;
                }
            }
        }

        let min_reports: u32 = env.storage().instance().get(&DataKey::MinReports).unwrap();
        if count == 0 || (count as u32) < min_reports {
            None
        } else {
            Some(median(&mut buffer, count))
        }
    }
}
