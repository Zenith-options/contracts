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
fn is_signer_is_o1_membership() {
    let (env, id) = setup();
    env.as_contract(&id, || {
        let signer = Address::generate(&env);
        assert!(!storage::is_signer(&env, &signer));
        storage::add_signer(&env, &signer);
        assert!(storage::is_signer(&env, &signer));
        storage::remove_signer(&env, &signer);
        assert!(!storage::is_signer(&env, &signer));
    });
}

#[test]
fn signer_list_enumerates_added_signers() {
    let (env, id) = setup();
    env.as_contract(&id, || {
        let a = Address::generate(&env);
        let b = Address::generate(&env);
        storage::add_signer(&env, &a);
        storage::add_signer(&env, &b);
        assert_eq!(storage::signer_list(&env).len(), 2);

        storage::remove_signer(&env, &a);
        let remaining = storage::signer_list(&env);
        assert_eq!(remaining.len(), 1);
        assert_eq!(remaining.get(0), Some(b));
    });
}
