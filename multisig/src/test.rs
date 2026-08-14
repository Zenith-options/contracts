#![cfg(test)]

use crate::{Multisig, MultisigClient};
use soroban_sdk::{testutils::Address as _, vec, Address, Env};

struct Harness<'a> {
    env: Env,
    client: MultisigClient<'a>,
    signers: [Address; 3],
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let signers = [
        Address::generate(&env),
        Address::generate(&env),
        Address::generate(&env),
    ];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    client.initialize(
        &vec![
            &env,
            signers[0].clone(),
            signers[1].clone(),
            signers[2].clone(),
        ],
        &2,
    );

    Harness {
        env,
        client,
        signers,
    }
}

#[test]
fn initialize_sets_signers_and_threshold() {
    let h = setup();
    assert_eq!(h.client.get_signer_count(), 3);
    assert_eq!(h.client.get_threshold(), 2);
    for signer in &h.signers {
        assert!(h.client.is_signer(signer));
    }
    let stranger = Address::generate(&h.env);
    assert!(!h.client.is_signer(&stranger));
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // AlreadyInitialized
fn initialize_twice_panics() {
    let h = setup();
    h.client.initialize(&vec![&h.env, h.signers[0].clone()], &1);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidThreshold
fn initialize_rejects_a_zero_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let signers = vec![&env, Address::generate(&env)];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    client.initialize(&signers, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidThreshold
fn initialize_rejects_a_threshold_above_the_signer_count() {
    let env = Env::default();
    env.mock_all_auths();
    let signers = vec![&env, Address::generate(&env), Address::generate(&env)];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    client.initialize(&signers, &3);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // DuplicateSigner
fn initialize_rejects_a_duplicate_signer() {
    let env = Env::default();
    env.mock_all_auths();
    let signer = Address::generate(&env);
    let signers = vec![&env, signer.clone(), signer];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    client.initialize(&signers, &1);
}
