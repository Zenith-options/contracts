#![cfg(test)]

use crate::{PriceOracle, PriceOracleClient};
use multisig::{Multisig, MultisigClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger},
    Address, Env, Symbol, TryFromVal,
};

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
    h.client
        .report_price(&stranger, &Symbol::new(&h.env, "XLM"), &1_200_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // InvalidPrice
fn report_price_rejects_a_non_positive_price() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    h.client
        .report_price(&feeder, &Symbol::new(&h.env, "XLM"), &0);
}

#[test]
fn report_price_rejects_a_revoked_feeder() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    h.client.remove_feeder(&feeder);

    let result = h
        .client
        .try_report_price(&feeder, &Symbol::new(&h.env, "XLM"), &1_200_000);
    assert!(result.is_err());
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // TooManyFeeders
fn add_feeder_rejects_beyond_the_cap() {
    let h = setup();
    for _ in 0..16 {
        h.client.add_feeder(&Address::generate(&h.env));
    }
    assert_eq!(h.client.get_feeder_count(), 16);
    h.client.add_feeder(&Address::generate(&h.env));
}

#[test]
fn get_latest_report_is_none_before_any_report() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    assert!(h
        .client
        .get_latest_report(&Symbol::new(&h.env, "XLM"), &feeder)
        .is_none());
}

// ─── get_price aggregation ───────────────────────────────────────────────────

#[test]
fn get_price_is_none_with_no_feeders() {
    let h = setup();
    assert!(h.client.get_price(&Symbol::new(&h.env, "XLM")).is_none());
}

#[test]
fn get_price_matches_the_single_feeders_report() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    let xlm = Symbol::new(&h.env, "XLM");
    h.client.report_price(&feeder, &xlm, &1_200_000);

    assert_eq!(h.client.get_price(&xlm).unwrap(), 1_200_000);
}

#[test]
fn get_price_is_the_median_across_several_feeders() {
    let h = setup();
    let xlm = Symbol::new(&h.env, "XLM");
    let prices = [1_000_000i128, 1_100_000, 1_050_000];
    for price in prices {
        let feeder = Address::generate(&h.env);
        h.client.add_feeder(&feeder);
        h.client.report_price(&feeder, &xlm, &price);
    }

    assert_eq!(h.client.get_price(&xlm).unwrap(), 1_050_000);
}

#[test]
fn get_price_ignores_a_stale_report() {
    let h = setup();
    let xlm = Symbol::new(&h.env, "XLM");

    let stale_feeder = Address::generate(&h.env);
    h.client.add_feeder(&stale_feeder);
    h.client.report_price(&stale_feeder, &xlm, &999_999_999); // way off, but will go stale

    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + h.client.get_max_staleness() + 1);

    let fresh_feeder = Address::generate(&h.env);
    h.client.add_feeder(&fresh_feeder);
    h.client.report_price(&fresh_feeder, &xlm, &1_200_000);

    // Only the fresh feeder's report counts — the stale one is excluded
    // entirely rather than dragging the median toward its outlier value.
    assert_eq!(h.client.get_price(&xlm).unwrap(), 1_200_000);
}

#[test]
fn get_price_ignores_a_revoked_feeders_old_report() {
    let h = setup();
    let xlm = Symbol::new(&h.env, "XLM");

    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    h.client.report_price(&feeder, &xlm, &1_200_000);
    h.client.remove_feeder(&feeder);

    // Their report is still in storage (audit trail), but a revoked feeder
    // no longer counts toward the aggregate.
    assert!(h.client.get_latest_report(&xlm, &feeder).is_some());
    assert!(h.client.get_price(&xlm).is_none());
}

// ─── max_staleness ───────────────────────────────────────────────────────────

#[test]
fn max_staleness_defaults_to_one_hour_and_is_configurable() {
    let h = setup();
    assert_eq!(h.client.get_max_staleness(), 3600);

    h.client.set_max_staleness(&7200);
    assert_eq!(h.client.get_max_staleness(), 7200);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")] // InvalidStaleness
fn set_max_staleness_rejects_zero() {
    let h = setup();
    h.client.set_max_staleness(&0);
}

// ─── transfer_admin ─────────────────────────────────────────────────────────

#[test]
fn transfer_admin_hands_off_control() {
    let h = setup();
    assert_eq!(h.client.get_admin(), h.admin);

    let new_admin = Address::generate(&h.env);
    h.client.transfer_admin(&new_admin);
    assert_eq!(h.client.get_admin(), new_admin);

    // Admin-gated calls still work, now authorized against the new admin.
    h.client.add_feeder(&Address::generate(&h.env));
}

// ─── pause / unpause ────────────────────────────────────────────────────────

#[test]
fn pause_blocks_mutations_unpause_restores_them() {
    let h = setup();
    assert!(!h.client.is_paused());

    h.client.pause();
    assert!(h.client.is_paused());

    h.client.unpause();
    assert!(!h.client.is_paused());
    h.client.add_feeder(&Address::generate(&h.env));
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // ContractPaused
fn add_feeder_is_rejected_while_paused() {
    let h = setup();
    h.client.pause();
    h.client.add_feeder(&Address::generate(&h.env));
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // ContractPaused
fn report_price_is_rejected_while_paused() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    h.client.pause();
    h.client
        .report_price(&feeder, &Symbol::new(&h.env, "XLM"), &1_200_000);
}

/// A pause must not hide the last-known price from callers still reading
/// it — get_price stays live even while mutations are frozen.
#[test]
fn pause_does_not_block_get_price() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    let xlm = Symbol::new(&h.env, "XLM");
    h.client.report_price(&feeder, &xlm, &1_200_000);

    h.client.pause();
    assert_eq!(h.client.get_price(&xlm).unwrap(), 1_200_000);
}

// ─── events ──────────────────────────────────────────────────────────────────

#[test]
fn add_feeder_emits_a_feeder_added_event() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);

    let events = h.env.events().all();
    let (contract_id, _topics, data) = events.last().unwrap();
    assert_eq!(contract_id, h.client.address);
    assert_eq!(Address::try_from_val(&h.env, &data).unwrap(), feeder);
}

#[test]
fn report_price_emits_a_price_reported_event() {
    let h = setup();
    let feeder = Address::generate(&h.env);
    h.client.add_feeder(&feeder);
    let xlm = Symbol::new(&h.env, "XLM");

    h.client.report_price(&feeder, &xlm, &1_200_000);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 1_200_000);
}

#[test]
fn pause_and_unpause_each_emit_their_own_event() {
    let h = setup();
    let events_before = h.env.events().all().len();

    h.client.pause();
    h.client.unpause();

    let events_after = h.env.events().all().len();
    assert_eq!(events_after - events_before, 2);
}

// ─── cross-contract: pause_via_multisig / transfer_admin_via_multisig ─────

/// Deploys a real multisig contract in the SAME Env as the price
/// oracle under test, with a 2-of-3 threshold.
fn setup_multisig(h: &Harness) -> (Address, [Address; 3]) {
    let signers = [
        Address::generate(&h.env),
        Address::generate(&h.env),
        Address::generate(&h.env),
    ];
    let contract_id = h.env.register_contract(None, Multisig);
    let client = MultisigClient::new(&h.env, &contract_id);
    client.initialize(
        &soroban_sdk::vec![
            &h.env,
            signers[0].clone(),
            signers[1].clone(),
            signers[2].clone()
        ],
        &2,
    );
    (contract_id, signers)
}

#[test]
fn pause_via_multisig_pauses_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = 1u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    assert!(!h.client.is_paused());
    h.client.pause_via_multisig(&multisig_id, &action_id);
    assert!(h.client.is_paused());
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // Unauthorized
fn pause_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    multisig_client.approve(&signers[0], &1u64); // only 1 of 3

    h.client.pause_via_multisig(&multisig_id, &1u64);
}

#[test]
fn unpause_via_multisig_unpauses_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    h.client.pause();
    assert!(h.client.is_paused());

    let action_id = 3u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client.unpause_via_multisig(&multisig_id, &action_id);
    assert!(!h.client.is_paused());
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // Unauthorized
fn unpause_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client.pause();
    h.client.unpause_via_multisig(&multisig_id, &99u64);
}

#[test]
fn transfer_admin_via_multisig_hands_off_control_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = 2u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let new_admin = Address::generate(&h.env);
    h.client
        .transfer_admin_via_multisig(&multisig_id, &action_id, &new_admin);
    assert_eq!(h.client.get_admin(), new_admin);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // Unauthorized
fn transfer_admin_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);
    let new_admin = Address::generate(&h.env);

    h.client
        .transfer_admin_via_multisig(&multisig_id, &99u64, &new_admin);
}

#[test]
fn set_max_staleness_via_multisig_applies_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = 4u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client
        .set_max_staleness_via_multisig(&multisig_id, &action_id, &7200);
    assert_eq!(h.client.get_max_staleness(), 7200);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // Unauthorized
fn set_max_staleness_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client
        .set_max_staleness_via_multisig(&multisig_id, &99u64, &7200);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")] // InvalidStaleness
fn set_max_staleness_via_multisig_still_rejects_zero_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = 5u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client
        .set_max_staleness_via_multisig(&multisig_id, &action_id, &0);
}
