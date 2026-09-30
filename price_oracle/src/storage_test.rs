use soroban_sdk::{contract, testutils::Address as _, Address, Env};

use crate::storage;

// Dummy contract purely to give these storage-layer unit tests a contract
// context (issue #101 only touches storage/types here).
#[contract]
struct Dummy;

fn setup() -> (Env, Address) {
    let env = Env::default();
    let id = env.register_contract(None, Dummy);
    (env, id)
}

#[test]
fn is_feeder_is_o1_membership() {
    let (env, id) = setup();
    env.as_contract(&id, || {
        let feeder = Address::generate(&env);
        assert!(!storage::is_feeder(&env, &feeder));
        storage::add_feeder(&env, &feeder);
        assert!(storage::is_feeder(&env, &feeder));
        storage::remove_feeder(&env, &feeder);
        assert!(!storage::is_feeder(&env, &feeder));
    });
}

#[test]
fn feeder_list_enumerates_added_feeders() {
    let (env, id) = setup();
    env.as_contract(&id, || {
        let a = Address::generate(&env);
        let b = Address::generate(&env);
        storage::add_feeder(&env, &a);
        storage::add_feeder(&env, &b);
        assert_eq!(storage::feeder_list(&env).len(), 2);

        storage::remove_feeder(&env, &a);
        let remaining = storage::feeder_list(&env);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining.get(0), Some(b));
    });
}
