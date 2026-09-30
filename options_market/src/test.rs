use soroban_sdk::{contract, testutils::Address as _, Address, Env};

use crate::storage;
use crate::types::{Config, OptionPosition, PositionStatus, Stats};

// A dummy contract purely so these storage-layer unit tests have a contract
// context to run `env.storage()` calls against (issues #100/#102/#103 only
// touch storage/types — there is no `#[contract]` in this crate to attach
// entrypoints to yet).
#[contract]
struct Dummy;

fn setup() -> (Env, Address) {
    let env = Env::default();
    let id = env.register_contract(None, Dummy);
    (env, id)
}

#[test]
fn config_round_trips() {
    let (env, id) = setup();
    env.as_contract(&id, || {
        let config = Config {
            admin: Address::generate(&env),
            oracle: Address::generate(&env),
            token: Address::generate(&env),
            fee_recipient: Address::generate(&env),
            fee_bps: 50,
            paused: false,
        };
        storage::save_config(&env, &config);
        let loaded = storage::load_config(&env);
        assert_eq!(loaded.fee_bps, 50);
        assert!(!loaded.paused);
    });
}

#[test]
fn stats_defaults_to_zero_then_round_trips() {
    let (env, id) = setup();
    env.as_contract(&id, || {
        let stats = storage::load_stats(&env);
        assert_eq!(stats.position_counter, 0);

        let updated = Stats {
            premiums_collected: 100,
            open_interest: 5,
            premium_pool: 100,
            series_count: 1,
            position_counter: 1,
        };
        storage::save_stats(&env, &updated);
        assert_eq!(storage::load_stats(&env).position_counter, 1);
    });
}

#[test]
fn position_status_replaces_the_two_bool_flags() {
    let (env, id) = setup();
    env.as_contract(&id, || {
        let owner = Address::generate(&env);
        let position = OptionPosition {
            series_id: 1,
            owner: owner.clone(),
            side_is_buyer: true,
            amount: 10,
            collateral_locked: 0,
            status: PositionStatus::Open,
            opened_at: env.ledger().timestamp(),
        };
        storage::save_position(&env, 1, &position);
        let loaded = storage::load_position(&env, 1).unwrap();
        assert_eq!(loaded.status, PositionStatus::Open);

        let exercised = OptionPosition {
            status: PositionStatus::Exercised,
            ..loaded
        };
        storage::save_position(&env, 1, &exercised);
        assert_eq!(
            storage::load_position(&env, 1).unwrap().status,
            PositionStatus::Exercised
        );
    });
}

#[test]
fn user_positions_append_in_o1_and_paginate() {
    let (env, id) = setup();
    env.as_contract(&id, || {
        let user = Address::generate(&env);
        for i in 0..150u64 {
            storage::add_user_position(&env, &user, i);
        }
        assert_eq!(storage::user_position_count(&env, &user), 150);

        let page = storage::get_user_positions_page(&env, &user, 0, 10);
        assert_eq!(page.len(), 10);
        assert_eq!(page.get(0), Some(0));

        // Crosses a page boundary (POSITIONS_PER_PAGE == 64).
        let page = storage::get_user_positions_page(&env, &user, 60, 10);
        assert_eq!(page.len(), 10);
        assert_eq!(page.get(0), Some(60));
        assert_eq!(page.get(9), Some(69));

        // Past the end.
        let page = storage::get_user_positions_page(&env, &user, 145, 10);
        assert_eq!(page.len(), 5);
    });
}
