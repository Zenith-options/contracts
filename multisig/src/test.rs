#![cfg(test)]

use crate::types::ActionStatus;
use crate::{Multisig, MultisigClient, MAX_PAGE_LIMIT, MAX_PENDING_PER_SIGNER};
use soroban_sdk::{
    contract, contractimpl, symbol_short,
    testutils::{Address as _, Events as _, Ledger as _},
    vec, Address, BytesN, Env, IntoVal, Symbol, TryFromVal, Val, Vec,
};

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
        &0, // no approval expiry
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
    h.client
        .initialize(&vec![&h.env, h.signers[0].clone()], &1, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidThreshold
fn initialize_rejects_a_zero_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let signers = vec![&env, Address::generate(&env)];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    client.initialize(&signers, &0, &0);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidThreshold
fn initialize_rejects_a_threshold_above_the_signer_count() {
    let env = Env::default();
    env.mock_all_auths();
    let signers = vec![&env, Address::generate(&env), Address::generate(&env)];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    client.initialize(&signers, &3, &0);
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
    client.initialize(&signers, &1, &0);
}

// ─── approve / revoke / is_approved ────────────────────────────────────────

#[test]
fn approve_records_the_signers_own_vote() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));

    assert!(h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));
    assert!(!h.client.has_approved(&aid(&h.env, 42), &h.signers[1]));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 1);
}

#[test]
fn is_approved_flips_true_once_the_threshold_is_reached() {
    let h = setup();
    // Threshold is 2 of 3.
    assert!(!h.client.is_approved(&aid(&h.env, 42)));

    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    assert!(!h.client.is_approved(&aid(&h.env, 42)));

    h.client.approve(&h.signers[1], &aid(&h.env, 42));
    assert!(h.client.is_approved(&aid(&h.env, 42)));
}

#[test]
fn different_action_ids_track_approvals_independently() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 1));
    h.client.approve(&h.signers[0], &aid(&h.env, 2));
    h.client.approve(&h.signers[1], &aid(&h.env, 2));

    assert!(!h.client.is_approved(&aid(&h.env, 1))); // only 1 of 3
    assert!(h.client.is_approved(&aid(&h.env, 2))); // 2 of 3
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // NotASigner
fn approve_rejects_a_non_signer() {
    let h = setup();
    let stranger = Address::generate(&h.env);
    h.client.approve(&stranger, &aid(&h.env, 42));
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // AlreadyApproved
fn approve_rejects_the_same_signer_voting_twice() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
}

#[test]
fn revoke_withdraws_the_signers_vote() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.approve(&h.signers[1], &aid(&h.env, 42));
    assert!(h.client.is_approved(&aid(&h.env, 42)));

    h.client.revoke(&h.signers[1], &aid(&h.env, 42));
    assert!(!h.client.has_approved(&aid(&h.env, 42), &h.signers[1]));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 1);
    // Dropping below threshold un-approves the action.
    assert!(!h.client.is_approved(&aid(&h.env, 42)));
}

#[test]
fn a_revoked_signer_can_approve_again() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.revoke(&h.signers[0], &aid(&h.env, 42));
    h.client.approve(&h.signers[0], &aid(&h.env, 42));

    assert!(h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 1);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // NotYetApproved
fn revoke_rejects_a_signer_who_never_approved() {
    let h = setup();
    h.client.revoke(&h.signers[0], &aid(&h.env, 42));
}

// ─── approval expiry ────────────────────────────────────────────────────────

fn setup_with_ttl<'a>(ttl: u64) -> Harness<'a> {
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
        &ttl,
    );

    Harness {
        env,
        client,
        signers,
    }
}

#[test]
fn zero_ttl_means_approvals_never_expire() {
    let h = setup_with_ttl(0);
    h.client.approve(&h.signers[0], &aid(&h.env, 42));

    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 1_000_000_000);
    assert!(h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 1);
}

#[test]
fn approval_older_than_ttl_stops_counting() {
    let h = setup_with_ttl(3600);
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    assert!(h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));

    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 3601);
    assert!(!h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 0);
}

#[test]
fn is_approved_drops_once_enough_approvals_expire() {
    let h = setup_with_ttl(3600);
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.approve(&h.signers[1], &aid(&h.env, 42));
    assert!(h.client.is_approved(&aid(&h.env, 42)));

    // signers[1] approves again later, refreshing only its own timestamp.
    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 1800);
    h.client.revoke(&h.signers[1], &aid(&h.env, 42));
    h.client.approve(&h.signers[1], &aid(&h.env, 42));

    // Advance past signers[0]'s original approval but not signers[1]'s
    // refreshed one.
    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 1900);
    assert!(!h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));
    assert!(h.client.has_approved(&aid(&h.env, 42), &h.signers[1]));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 1);
    assert!(!h.client.is_approved(&aid(&h.env, 42)));
}

#[test]
fn a_signer_can_reapprove_after_their_own_approval_expires() {
    let h = setup_with_ttl(3600);
    h.client.approve(&h.signers[0], &aid(&h.env, 42));

    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 3601);
    // Expired, not merely revoked — re-approving must not panic with
    // AlreadyApproved.
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    assert!(h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // NotYetApproved
fn revoke_rejects_an_already_expired_approval() {
    let h = setup_with_ttl(3600);
    h.client.approve(&h.signers[0], &aid(&h.env, 42));

    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 3601);
    h.client.revoke(&h.signers[0], &aid(&h.env, 42));
}

#[test]
fn get_approval_ttl_returns_the_configured_value() {
    let h = setup_with_ttl(7200);
    assert_eq!(h.client.get_approval_ttl(), 7200);
}

// ─── reset ──────────────────────────────────────────────────────────────────

#[test]
fn reset_clears_every_signers_approval() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.approve(&h.signers[1], &aid(&h.env, 42));
    assert!(h.client.is_approved(&aid(&h.env, 42)));

    h.client.reset(&aid(&h.env, 42));

    assert!(!h.client.has_approved(&aid(&h.env, 42), &h.signers[0]));
    assert!(!h.client.has_approved(&aid(&h.env, 42), &h.signers[1]));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 0);
    assert!(!h.client.is_approved(&aid(&h.env, 42)));
}

#[test]
fn action_id_can_be_approved_fresh_after_a_reset() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.approve(&h.signers[1], &aid(&h.env, 42));
    h.client.reset(&aid(&h.env, 42));

    // The same id, reused for a later action, starts from a clean slate —
    // a single signer's leftover vote can't carry over.
    h.client.approve(&h.signers[2], &aid(&h.env, 42));
    assert_eq!(h.client.get_approval_count(&aid(&h.env, 42)), 1);
    assert!(!h.client.is_approved(&aid(&h.env, 42)));
}

#[test]
fn reset_does_not_affect_other_action_ids() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 1));
    h.client.approve(&h.signers[1], &aid(&h.env, 1));
    h.client.approve(&h.signers[0], &aid(&h.env, 2));
    h.client.approve(&h.signers[1], &aid(&h.env, 2));

    h.client.reset(&aid(&h.env, 1));

    assert!(!h.client.is_approved(&aid(&h.env, 1)));
    assert!(h.client.is_approved(&aid(&h.env, 2)));
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // NotYetApproved
fn reset_rejects_an_action_below_threshold() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42)); // only 1 of 3, threshold is 2
    h.client.reset(&aid(&h.env, 42));
}

// ─── events ──────────────────────────────────────────────────────────────────

#[test]
fn approve_emits_an_approved_event() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));

    let events = h.env.events().all();
    let (contract_id, _topics, _data) = events.last().unwrap();
    assert_eq!(contract_id, h.client.address);
}

#[test]
fn approved_event_topics_carry_the_signer_and_action_id() {
    // approve()'s entire payload lives in its topics — data is just ()
    // — so unlike most other events here, checking contract_id alone
    // proves nothing about WHICH signer or action_id was actually
    // recorded. Decodes topics[1] (signer) and topics[2] (action_id)
    // directly.
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));

    let events = h.env.events().all();
    let (_, topics, _data) = events.last().unwrap();
    let signer = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    let action_id = BytesN::<32>::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    assert_eq!(signer, h.signers[0]);
    assert_eq!(action_id, aid(&h.env, 42));
}

#[test]
fn revoke_emits_a_revoked_event() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.revoke(&h.signers[0], &aid(&h.env, 42));

    let events = h.env.events().all();
    let (contract_id, _topics, _data) = events.last().unwrap();
    assert_eq!(contract_id, h.client.address);
}

#[test]
fn revoked_event_topics_carry_the_signer_and_action_id() {
    let h = setup();
    h.client.approve(&h.signers[1], &aid(&h.env, 7));
    h.client.revoke(&h.signers[1], &aid(&h.env, 7));

    let events = h.env.events().all();
    let (_, topics, _data) = events.last().unwrap();
    let signer = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    let action_id = BytesN::<32>::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    assert_eq!(signer, h.signers[1]);
    assert_eq!(action_id, aid(&h.env, 7));
}

#[test]
fn reset_emits_a_reset_event_with_the_action_id_and_description_hash() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 42));
    h.client.approve(&h.signers[1], &aid(&h.env, 42));
    h.client.reset(&aid(&h.env, 42));

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    assert_eq!(
        BytesN::<32>::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap(),
        aid(&h.env, 42)
    );
    assert_eq!(
        BytesN::<32>::try_from_val(&h.env, &data).unwrap(),
        BytesN::from_array(&h.env, &[0; 32])
    );
}

// ─── require_auth is load-bearing where it exists, absent where it doesn't ─

/// Confirms initialize()'s lack of require_auth() is an intentional
/// design choice, not an untested oversight: it succeeds even with NO
/// auths mocked at all, unlike every other contract's initialize()
/// here (which all require the incoming admin's own signature). See
/// the doc comment on initialize() for why this is safe — approve()'s
/// own require_auth() is what actually gates anything.
#[test]
fn initialize_does_not_require_any_signers_authorization() {
    let env = Env::default(); // deliberately no mock_all_auths()
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
        &0,
    );
    assert_eq!(client.get_signer_count(), 3);
}

#[test]
#[should_panic] // no auth was mocked at all — require_auth() has nothing to accept
fn approve_without_any_authorization_panics() {
    // Unlike initialize(), approve() DOES require the signer's own
    // signature — this confirms that check is load-bearing, not a
    // no-op, by never arming mock_all_auths() in the first place.
    let env = Env::default();
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
        &0,
    );

    client.approve(&signers[0], &aid(&env, 42));
}

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

// ─── registry ───────────────────────────────────────────────────────────────

#[test]
fn first_approval_registers_the_action() {
    let h = setup();
    assert!(h.client.get_action(&aid(&h.env, 1)).is_none());
    h.env.ledger().with_mut(|l| l.timestamp = 500);
    h.client.approve(&h.signers[0], &aid(&h.env, 1));

    let meta = h.client.get_action(&aid(&h.env, 1)).unwrap();
    assert_eq!(meta.proposer, h.signers[0]);
    assert_eq!(meta.created_at, 500);
    assert_eq!(meta.status, ActionStatus::Pending);
    assert_eq!(h.client.get_pending_count(), 1);
}

#[test]
fn register_action_stores_the_description_hash_and_approvals_carry_it() {
    let h = setup();
    let desc = aid(&h.env, 0xAB);
    h.client
        .register_action(&h.signers[2], &aid(&h.env, 1), &desc);
    h.client.approve(&h.signers[0], &aid(&h.env, 1));

    let meta = h.client.get_action(&aid(&h.env, 1)).unwrap();
    assert_eq!(meta.proposer, h.signers[2]);
    assert_eq!(meta.description_hash, desc);
    let events = h.env.events().all();
    let (_, _, data) = events.last().unwrap();
    assert_eq!(BytesN::<32>::try_from_val(&h.env, &data).unwrap(), desc);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")] // ActionAlreadyRegistered
fn register_action_rejects_a_pending_id() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 1));
    h.client
        .register_action(&h.signers[1], &aid(&h.env, 1), &aid(&h.env, 2));
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // NotASigner
fn register_action_rejects_a_non_signer() {
    let h = setup();
    let stranger = Address::generate(&h.env);
    h.client
        .register_action(&stranger, &aid(&h.env, 1), &aid(&h.env, 2));
}

#[test]
fn reset_removes_the_action_from_the_pending_index() {
    let h = setup();
    for n in 1..=3u8 {
        h.client.approve(&h.signers[0], &aid(&h.env, n));
    }
    h.client.approve(&h.signers[1], &aid(&h.env, 1));
    h.client.reset(&aid(&h.env, 1));

    assert_eq!(h.client.get_pending_count(), 2);
    let page = h.client.get_pending_actions(&0, &MAX_PAGE_LIMIT);
    assert!(!page.contains(aid(&h.env, 1)));
    assert!(page.contains(aid(&h.env, 2)) && page.contains(aid(&h.env, 3)));
    assert_eq!(
        h.client.get_action(&aid(&h.env, 1)).unwrap().status,
        ActionStatus::Reset
    );

    // A reset id can be reused, starting over as a fresh pending action.
    h.client.approve(&h.signers[0], &aid(&h.env, 1));
    assert_eq!(
        h.client.get_action(&aid(&h.env, 1)).unwrap().status,
        ActionStatus::Pending
    );
}

#[test]
fn pending_actions_paginate_across_boundaries() {
    let h = setup();
    for n in 1..=5u8 {
        let signer = &h.signers[(n % 3) as usize];
        h.client.approve(signer, &aid(&h.env, n));
    }
    assert_eq!(h.client.get_pending_actions(&0, &2).len(), 2);
    assert_eq!(h.client.get_pending_actions(&2, &2).len(), 2);
    assert_eq!(h.client.get_pending_actions(&4, &2).len(), 1);
    assert_eq!(h.client.get_pending_actions(&5, &2).len(), 0);
    assert_eq!(h.client.get_pending_actions(&50, &2).len(), 0);

    let mut all = Vec::new(&h.env);
    for c in [0u32, 2, 4] {
        all.append(&h.client.get_pending_actions(&c, &2));
    }
    for n in 1..=5u8 {
        assert!(all.contains(aid(&h.env, n)));
    }
}

#[test]
#[should_panic(expected = "Error(Contract, #12)")] // InvalidPageLimit
fn pending_actions_rejects_an_oversized_limit() {
    let h = setup();
    h.client.get_pending_actions(&0, &(MAX_PAGE_LIMIT + 1));
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // TooManyPendingActions
fn a_signer_cannot_register_more_than_the_per_signer_cap() {
    let h = setup();
    for n in 0..=MAX_PENDING_PER_SIGNER {
        h.client.approve(&h.signers[0], &aid(&h.env, n as u8));
    }
}

#[test]
fn stale_actions_can_be_expired_out_of_the_index() {
    let h = setup_with_ttl(100);
    h.env.ledger().with_mut(|l| l.timestamp = 1_000);
    h.client.approve(&h.signers[0], &aid(&h.env, 1));
    h.env.ledger().with_mut(|l| l.timestamp = 1_101);

    h.client.expire(&aid(&h.env, 1));
    assert_eq!(h.client.get_pending_count(), 0);
    assert_eq!(
        h.client.get_action(&aid(&h.env, 1)).unwrap().status,
        ActionStatus::Expired
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")] // NotExpired
fn expire_rejects_an_action_with_a_fresh_approval() {
    let h = setup_with_ttl(100);
    h.env.ledger().with_mut(|l| l.timestamp = 1_000);
    h.client.approve(&h.signers[0], &aid(&h.env, 1));
    h.env.ledger().with_mut(|l| l.timestamp = 1_101);
    h.client.approve(&h.signers[1], &aid(&h.env, 1));
    h.client.expire(&aid(&h.env, 1));
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")] // NotExpired
fn expire_is_unavailable_without_an_approval_ttl() {
    let h = setup();
    h.client.approve(&h.signers[0], &aid(&h.env, 1));
    h.client.revoke(&h.signers[0], &aid(&h.env, 1));
    h.client.expire(&aid(&h.env, 1));
}

// ─── executor ───────────────────────────────────────────────────────────────

/// Minimal target: `admin`-gated setter, plus a function that always fails.
#[contract]
pub struct Target;

#[contractimpl]
impl Target {
    pub fn init(env: Env, admin: Address) {
        env.storage()
            .instance()
            .set(&symbol_short!("admin"), &admin);
    }

    pub fn set(env: Env, value: u32) -> u32 {
        let admin: Address = env
            .storage()
            .instance()
            .get(&symbol_short!("admin"))
            .unwrap();
        admin.require_auth();
        env.storage()
            .instance()
            .set(&symbol_short!("value"), &value);
        value
    }

    pub fn get(env: Env) -> u32 {
        env.storage()
            .instance()
            .get(&symbol_short!("value"))
            .unwrap_or(0)
    }

    pub fn fail(_env: Env) {
        panic!("target failed");
    }
}

fn setup_target(h: &Harness) -> TargetClient<'static> {
    let id = h.env.register_contract(None, Target);
    let target = TargetClient::new(&h.env, &id);
    target.init(&h.client.address);
    target
}

fn set_args(env: &Env, value: u32) -> Vec<Val> {
    vec![env, value.into_val(env)]
}

#[test]
fn execute_invokes_the_target_as_the_multisig() {
    let h = setup();
    let target = setup_target(&h);
    let id = h.client.propose(
        &h.signers[0],
        &target.address,
        &Symbol::new(&h.env, "set"),
        &set_args(&h.env, 7),
    );
    assert_eq!(h.client.get_action(&id).unwrap().description_hash, id);
    h.client.approve(&h.signers[0], &id);
    h.client.approve(&h.signers[1], &id);

    // No mocked auths from here on: the target's admin.require_auth()
    // must be satisfied purely because the multisig is the direct caller.
    h.env.set_auths(&[]);
    let ret = h.client.execute(&id);
    assert_eq!(u32::try_from_val(&h.env, &ret).unwrap(), 7);
    assert_eq!(target.get(), 7);
    assert_eq!(
        h.client.get_action(&id).unwrap().status,
        ActionStatus::Executed
    );
    assert_eq!(h.client.get_pending_count(), 0);
}

#[test]
fn identical_proposals_get_distinct_ids() {
    let h = setup();
    let target = setup_target(&h);
    let f = Symbol::new(&h.env, "set");
    let a = h
        .client
        .propose(&h.signers[0], &target.address, &f, &set_args(&h.env, 1));
    let b = h
        .client
        .propose(&h.signers[0], &target.address, &f, &set_args(&h.env, 1));
    assert_ne!(a, b);
    assert_eq!(h.client.get_proposal(&a).unwrap().nonce, 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // ActionNotPending
fn executed_proposals_cannot_be_replayed() {
    let h = setup();
    let target = setup_target(&h);
    let id = h.client.propose(
        &h.signers[0],
        &target.address,
        &Symbol::new(&h.env, "set"),
        &set_args(&h.env, 7),
    );
    h.client.approve(&h.signers[0], &id);
    h.client.approve(&h.signers[1], &id);
    h.client.execute(&id);
    h.client.execute(&id);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // ActionNotPending
fn executed_proposals_cannot_be_re_approved() {
    let h = setup();
    let target = setup_target(&h);
    let id = h.client.propose(
        &h.signers[0],
        &target.address,
        &Symbol::new(&h.env, "set"),
        &set_args(&h.env, 7),
    );
    h.client.approve(&h.signers[0], &id);
    h.client.approve(&h.signers[1], &id);
    h.client.execute(&id);
    h.client.approve(&h.signers[0], &id);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // NotYetApproved
fn execute_requires_threshold() {
    let h = setup();
    let target = setup_target(&h);
    let id = h.client.propose(
        &h.signers[0],
        &target.address,
        &Symbol::new(&h.env, "set"),
        &set_args(&h.env, 7),
    );
    h.client.approve(&h.signers[0], &id);
    h.client.execute(&id);
}

#[test]
fn a_failing_target_reverts_execute_entirely() {
    let h = setup();
    let target = setup_target(&h);
    let id = h.client.propose(
        &h.signers[0],
        &target.address,
        &Symbol::new(&h.env, "fail"),
        &Vec::new(&h.env),
    );
    h.client.approve(&h.signers[0], &id);
    h.client.approve(&h.signers[1], &id);

    assert!(h.client.try_execute(&id).is_err());
    // Nothing stuck: still pending, still approved, still executable.
    assert_eq!(
        h.client.get_action(&id).unwrap().status,
        ActionStatus::Pending
    );
    assert!(h.client.is_approved(&id));
}

#[test]
#[should_panic(expected = "Error(Contract, #11)")] // ProposalTooLarge
fn propose_rejects_oversized_argument_lists() {
    let h = setup();
    let target = setup_target(&h);
    let mut args = Vec::new(&h.env);
    for i in 0..=crate::MAX_PROPOSAL_ARGS {
        args.push_back(i.into_val(&h.env));
    }
    h.client.propose(
        &h.signers[0],
        &target.address,
        &Symbol::new(&h.env, "set"),
        &args,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // ActionNotPending
fn proposals_cannot_be_reset_out_from_under_execute() {
    let h = setup();
    let target = setup_target(&h);
    let id = h.client.propose(
        &h.signers[0],
        &target.address,
        &Symbol::new(&h.env, "set"),
        &set_args(&h.env, 7),
    );
    h.client.approve(&h.signers[0], &id);
    h.client.approve(&h.signers[1], &id);
    h.client.reset(&id);
}
