#![cfg(test)]

use crate::{PriceOracle, PriceOracleClient};
use soroban_sdk::{testutils::Address as _, Address, Env, Symbol};

struct Harness<'a> {
    env: Env,
    client: PriceOracleClient<'a>,
    admin: Address,
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let contract_id = env.register_contract(None, PriceOracle);
    let client = PriceOracleClient::new(&env, &contract_id);
    client.initialize(&admin);

    Harness { env, client, admin }
}

#[test]
fn initialize_sets_admin() {
    setup();
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // AlreadyInitialized
fn initialize_twice_panics() {
    let h = setup();
    h.client.initialize(&h.admin);
}

// ─── feeder management ───────────────────────────────────────────────────────

#[test]
fn add_feeder_authorizes_and_counts_it() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    assert_eq!(h.client.get_feeder_count(), 0);

    h.client.add_feeder(&feeder);
    assert!(h.client.is_feeder(&feeder));
    assert_eq!(h.client.get_feeder_count(), 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // FeederAlreadyAdded
fn add_feeder_rejects_a_duplicate() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    h.client.add_feeder(&feeder);
}

#[test]
fn remove_feeder_revokes_authorization() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);

    h.client.remove_feeder(&feeder);
    assert!(!h.client.is_feeder(&feeder));
    assert_eq!(h.client.get_feeder_count(), 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // FeederNotFound
fn remove_feeder_rejects_an_unknown_address() {
    let h = setup();
    let stranger = Address::generate(&h.env);
    h.client.remove_feeder(&stranger);
}

#[test]
fn is_feeder_is_false_for_an_unauthorized_address() {
    let h = setup();
    let stranger = Address::generate(&h.env);
    assert!(!h.client.is_feeder(&stranger));
}

// ─── report_price ────────────────────────────────────────────────────────────

#[test]
fn report_price_stores_the_feeders_report() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);

    let xlm = Symbol::new(&h.env, "XLM");
    h.client.report_price(&feeder, &xlm, &1_200_000);

    let (price, timestamp) = h.client.get_latest_report(&xlm, &feeder).unwrap();
    assert_eq!(price, 1_200_000);
    assert_eq!(timestamp, h.env.ledger().timestamp());
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // NotAFeeder
fn report_price_rejects_an_unauthorized_reporter() {
    let h = setup();
    let stranger = Address::generate(&h.env);
    h.client.report_price(&stranger, &Symbol::new(&h.env, "XLM"), &1_200_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // InvalidPrice
fn report_price_rejects_a_non_positive_price() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    h.client.report_price(&feeder, &Symbol::new(&h.env, "XLM"), &0);
}

#[test]
fn report_price_rejects_a_revoked_feeder() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    h.client.remove_feeder(&feeder);

    let result = h.client.try_report_price(&feeder, &Symbol::new(&h.env, "XLM"), &1_200_000);
    assert!(result.is_err());
}

#[test]
fn get_latest_report_is_none_before_any_report() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    assert!(h.client.get_latest_report(&Symbol::new(&h.env, "XLM"), &feeder).is_none());
}
