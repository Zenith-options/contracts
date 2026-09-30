//! Resource-budget snapshots (#105): CPU, memory, and read/write entries
//! and bytes for options_market's public entrypoints, compared against
//! `snapshots/resources/*.json` (see tools/resource-snapshot).
//!
//! Mode: with `--features resource-wasm` (what CI runs, after building
//! `target/wasm32-unknown-unknown/release/zenith_options_market.wasm`)
//! the contract is registered from its wasm, so the numbers include VM
//! instantiation and match on-chain execution much more closely. Without
//! the feature it's registered natively; those numbers are recorded as
//! `native` and never compared against `wasm` snapshots.
//!
//! Regenerate: `UPDATE_SNAPSHOTS=1 cargo test --features resource-wasm resource_snapshots`.
//!
//! Scenarios needing vault or price_oracle (escrow_series_to_vault,
//! claim_refund_from_vault, set_settlement_price_from_oracle) are not
//! covered yet: those crates' sources are empty on main.

#![cfg(test)]

extern crate std;

use crate::math::{MAX_BATCH_SIZE, MAX_PAGE_SCAN, PRUNE_RETENTION};
use crate::types::DataKey;
use crate::{OptionType, OptionsMarketClient};
use resource_snapshot::{measure, Suite};
use soroban_sdk::{testutils::Address as _, testutils::Ledger, token, Address, Env, Symbol, Vec};

const UNIT: i128 = 10_000_000;

#[cfg(feature = "resource-wasm")]
const MODE: &str = "wasm";
#[cfg(not(feature = "resource-wasm"))]
const MODE: &str = "native";

#[cfg(feature = "resource-wasm")]
fn register(env: &Env) -> Address {
    const WASM: &[u8] =
        include_bytes!("../target/wasm32-unknown-unknown/release/zenith_options_market.wasm");
    env.register_contract_wasm(None, WASM)
}

#[cfg(not(feature = "resource-wasm"))]
fn register(env: &Env) -> Address {
    env.register_contract(None, crate::OptionsMarket)
}

fn suite() -> Suite {
    Suite::new(env!("CARGO_MANIFEST_DIR"), MODE)
}

struct H<'a> {
    env: Env,
    client: OptionsMarketClient<'a>,
    token: Address,
}

fn setup<'a>() -> H<'a> {
    let env = Env::default();
    env.mock_all_auths();
    env.budget().reset_unlimited();
    let admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(Address::generate(&env))
        .address();
    let client = OptionsMarketClient::new(&env, &register(&env));
    client.initialize(&admin, &Address::generate(&env), &token, &admin);
    H { env, client, token }
}

impl H<'_> {
    fn funded(&self, amount: i128) -> Address {
        let who = Address::generate(&self.env);
        token::StellarAssetClient::new(&self.env, &self.token).mint(&who, &amount);
        who
    }

    fn series(&self, option_type: OptionType, strike: i128) -> u64 {
        self.client.create_series(
            &Symbol::new(&self.env, "XLM"),
            &option_type,
            &strike,
            &(self.env.ledger().timestamp() + 30 * 86_400),
            &(40 * UNIT / 10),
            &450_000_000,
        )
    }

    fn buy(&self, who: &Address, series_id: u64) -> u64 {
        self.client
            .buy_option(who, &series_id, &UNIT, &(1_000 * UNIT))
    }

    fn write(&self, who: &Address, series_id: u64) -> u64 {
        self.client
            .write_option(who, &series_id, &UNIT, &(10_000 * UNIT), &0)
    }

    fn settle(&self, series_id: u64, price: i128) {
        let expiry = self.client.get_series(&series_id).unwrap().expiry;
        self.env.ledger().set_timestamp(expiry + 1);
        self.client.set_settlement_price(&series_id, &price);
    }

    fn past_retention(&self, series_id: u64) {
        let expiry = self.client.get_series(&series_id).unwrap().expiry;
        self.env
            .ledger()
            .set_timestamp(expiry + 86_400 + PRUNE_RETENTION + 1);
    }
}

#[test]
fn resource_snapshots_create_series() {
    let h = setup();
    h.series(OptionType::Call, 700 * UNIT); // warm the per-underlying keys
    let (_, m) = measure(&h.env, || h.series(OptionType::Call, 710 * UNIT));
    suite().check("create_series", m);
}

#[test]
fn resource_snapshots_buy_option() {
    let h = setup();
    let s = h.series(OptionType::Call, 700 * UNIT);
    let buyer = h.funded(10_000 * UNIT);
    h.buy(&buyer, s);
    let (_, m) = measure(&h.env, || h.buy(&buyer, s));
    suite().check("buy_option", m);
}

#[test]
fn resource_snapshots_write_option() {
    let h = setup();
    let s = h.series(OptionType::Put, 700 * UNIT);
    let buyer = h.funded(10_000 * UNIT);
    h.buy(&buyer, s);
    let writer = h.funded(10_000 * UNIT);
    let (_, m) = measure(&h.env, || h.write(&writer, s));
    suite().check("write_option", m);
}

#[test]
fn resource_snapshots_set_settlement_price() {
    let h = setup();
    let s = h.series(OptionType::Call, 700 * UNIT);
    let expiry = h.client.get_series(&s).unwrap().expiry;
    h.env.ledger().set_timestamp(expiry + 1);
    let (_, m) = measure(&h.env, || h.client.set_settlement_price(&s, &(750 * UNIT)));
    suite().check("set_settlement_price", m);
}

#[test]
fn resource_snapshots_exercise() {
    let h = setup();
    let s = h.series(OptionType::Call, 700 * UNIT);
    let buyer = h.funded(10_000 * UNIT);
    let pos = h.buy(&buyer, s);
    token::StellarAssetClient::new(&h.env, &h.token).mint(&h.client.address, &(1_000 * UNIT));
    h.settle(s, 750 * UNIT);
    let (_, m) = measure(&h.env, || h.client.exercise(&buyer, &pos));
    suite().check("exercise", m);
}

/// Worst case: MAX_BATCH_SIZE ITM longs in one call.
#[test]
fn resource_snapshots_exercise_batch_max() {
    let h = setup();
    let s = h.series(OptionType::Call, 700 * UNIT);
    let buyer = h.funded(100_000 * UNIT);
    let mut ids = Vec::new(&h.env);
    for _ in 0..MAX_BATCH_SIZE {
        ids.push_back(h.buy(&buyer, s));
    }
    // Contract balance must cover every payout (premiums alone don't).
    token::StellarAssetClient::new(&h.env, &h.token).mint(&h.client.address, &(10_000 * UNIT));
    h.settle(s, 750 * UNIT);
    let (_, m) = measure(&h.env, || h.client.exercise_batch(&buyer, &ids));
    suite().check("exercise_batch_25", m);
}

/// Worst case: MAX_BATCH_SIZE shorts reclaimed in one call.
#[test]
fn resource_snapshots_reclaim_batch_max() {
    let h = setup();
    let s = h.series(OptionType::Put, 700 * UNIT);
    let buyer = h.funded(100_000 * UNIT);
    let writer = h.funded(1_000_000 * UNIT);
    let mut ids = Vec::new(&h.env);
    for _ in 0..MAX_BATCH_SIZE {
        h.buy(&buyer, s);
        ids.push_back(h.write(&writer, s));
    }
    h.settle(s, 750 * UNIT);
    let (_, m) = measure(&h.env, || h.client.reclaim_batch(&writer, &ids));
    suite().check("reclaim_batch_25", m);
}

#[test]
fn resource_snapshots_cancel_and_claim_refund() {
    let h = setup();
    let s = h.series(OptionType::Call, 700 * UNIT);
    let buyer = h.funded(10_000 * UNIT);
    let pos = h.buy(&buyer, s);
    h.buy(&buyer, s);
    let (_, m) = measure(&h.env, || h.client.cancel_series(&s));
    suite().check("cancel_series", m);
    let (_, m) = measure(&h.env, || h.client.claim_refund(&buyer, &pos));
    suite().check("claim_refund", m);
}

#[test]
fn resource_snapshots_transfer_and_split_position() {
    let h = setup();
    let s = h.series(OptionType::Call, 700 * UNIT);
    let buyer = h.funded(10_000 * UNIT);
    let pos = h
        .client
        .buy_option(&buyer, &s, &(4 * UNIT), &(1_000 * UNIT));
    let to = Address::generate(&h.env);
    let (_, m) = measure(&h.env, || h.client.transfer_position(&buyer, &to, &pos));
    suite().check("transfer_position", m);
    let (_, m) = measure(&h.env, || h.client.split_position(&to, &pos, &UNIT));
    suite().check("split_position", m);
}

/// Worst case: MAX_BATCH_SIZE positions pruned in one call, all owned by
/// one user so every removal rewrites the same (largest) index entry.
#[test]
fn resource_snapshots_prune_positions_max() {
    let h = setup();
    let s = h.series(OptionType::Call, 700 * UNIT);
    let buyer = h.funded(100_000 * UNIT);
    let mut ids = Vec::new(&h.env);
    for _ in 0..MAX_BATCH_SIZE {
        ids.push_back(h.buy(&buyer, s));
    }
    h.settle(s, 650 * UNIT); // OTM: every long is terminal
    h.past_retention(s);
    let (_, m) = measure(&h.env, || h.client.prune_positions(&ids));
    suite().check("prune_positions_25", m);
    let (_, m) = measure(&h.env, || h.client.prune_series(&s));
    suite().check("prune_series", m);
}

/// Worst case: one full MAX_PAGE_SCAN page of the post-upgrade migration.
#[test]
fn resource_snapshots_migrate_counters_page() {
    let h = setup();
    let s = h.series(OptionType::Call, 700 * UNIT);
    let buyer = h.funded(1_000_000 * UNIT);
    for _ in 0..MAX_PAGE_SCAN {
        h.buy(&buyer, s);
    }
    h.env.as_contract(&h.client.address, || {
        h.env.storage().instance().remove(&DataKey::Migration);
    });
    let (_, m) = measure(&h.env, || h.client.migrate_counters(&MAX_PAGE_SCAN));
    suite().check("migrate_counters_200", m);
}

#[test]
fn resource_snapshots_views() {
    let h = setup();
    let s = h.series(OptionType::Call, 700 * UNIT);
    let buyer = h.funded(100_000 * UNIT);
    let mut last = 0;
    for _ in 0..MAX_BATCH_SIZE {
        last = h.buy(&buyer, s);
    }
    let (_, m) = measure(&h.env, || h.client.get_position_status(&last));
    suite().check("get_position_status", m);
    let (_, m) = measure(&h.env, || h.client.get_user_positions(&buyer));
    suite().check("get_user_positions_25", m);
    let (_, m) = measure(&h.env, || h.client.get_stats());
    suite().check("get_stats", m);
}
