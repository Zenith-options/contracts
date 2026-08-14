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

// ─── approve / revoke / is_approved ────────────────────────────────────────

#[test]
fn approve_records_the_signers_own_vote() {
    let h = setup();
    h.client.approve(&h.signers[0], &42);

    assert!(h.client.has_approved(&42, &h.signers[0]));
    assert!(!h.client.has_approved(&42, &h.signers[1]));
    assert_eq!(h.client.get_approval_count(&42), 1);
}

#[test]
fn is_approved_flips_true_once_the_threshold_is_reached() {
    let h = setup();
    // Threshold is 2 of 3.
    assert!(!h.client.is_approved(&42));

    h.client.approve(&h.signers[0], &42);
    assert!(!h.client.is_approved(&42));

    h.client.approve(&h.signers[1], &42);
    assert!(h.client.is_approved(&42));
}

#[test]
fn different_action_ids_track_approvals_independently() {
    let h = setup();
    h.client.approve(&h.signers[0], &1);
    h.client.approve(&h.signers[0], &2);
    h.client.approve(&h.signers[1], &2);

    assert!(!h.client.is_approved(&1)); // only 1 of 3
    assert!(h.client.is_approved(&2)); // 2 of 3
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // NotASigner
fn approve_rejects_a_non_signer() {
    let h = setup();
    let stranger = Address::generate(&h.env);
    h.client.approve(&stranger, &42);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // AlreadyApproved
fn approve_rejects_the_same_signer_voting_twice() {
    let h = setup();
    h.client.approve(&h.signers[0], &42);
    h.client.approve(&h.signers[0], &42);
}

#[test]
fn revoke_withdraws_the_signers_vote() {
    let h = setup();
    h.client.approve(&h.signers[0], &42);
    h.client.approve(&h.signers[1], &42);
    assert!(h.client.is_approved(&42));

    h.client.revoke(&h.signers[1], &42);
    assert!(!h.client.has_approved(&42, &h.signers[1]));
    assert_eq!(h.client.get_approval_count(&42), 1);
    // Dropping below threshold un-approves the action.
    assert!(!h.client.is_approved(&42));
}

#[test]
fn a_revoked_signer_can_approve_again() {
    let h = setup();
    h.client.approve(&h.signers[0], &42);
    h.client.revoke(&h.signers[0], &42);
    h.client.approve(&h.signers[0], &42);

    assert!(h.client.has_approved(&42, &h.signers[0]));
    assert_eq!(h.client.get_approval_count(&42), 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // NotYetApproved
fn revoke_rejects_a_signer_who_never_approved() {
    let h = setup();
    h.client.revoke(&h.signers[0], &42);
}
