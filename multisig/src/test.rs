#![cfg(test)]

use crate::{AccountConfig, AccountSignature, ActionClass, Delays, Multisig, MultisigClient};
use ed25519_dalek::{Signer, SigningKey};
use soroban_sdk::{
    auth::{Context, ContractContext},
    testutils::{Address as _, BytesN as _, Events as _, Ledger as _},
    vec, Address, BytesN, Env, IntoVal, Symbol, TryFromVal, Vec,
};

const NO_DELAYS: Delays = Delays {
    standard: 0,
    critical: 0,
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
        &NO_DELAYS,
        &None,
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
    h.client.initialize(
        &vec![&h.env, h.signers[0].clone()],
        &1,
        &0,
        &NO_DELAYS,
        &None,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidThreshold
fn initialize_rejects_a_zero_threshold() {
    let env = Env::default();
    env.mock_all_auths();
    let signers = vec![&env, Address::generate(&env)];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    client.initialize(&signers, &0, &0, &NO_DELAYS, &None);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidThreshold
fn initialize_rejects_a_threshold_above_the_signer_count() {
    let env = Env::default();
    env.mock_all_auths();
    let signers = vec![&env, Address::generate(&env), Address::generate(&env)];
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    client.initialize(&signers, &3, &0, &NO_DELAYS, &None);
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
    client.initialize(&signers, &1, &0, &NO_DELAYS, &None);
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
        &NO_DELAYS,
        &None,
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
    h.client.approve(&h.signers[0], &42);

    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 1_000_000_000);
    assert!(h.client.has_approved(&42, &h.signers[0]));
    assert_eq!(h.client.get_approval_count(&42), 1);
}

#[test]
fn approval_older_than_ttl_stops_counting() {
    let h = setup_with_ttl(3600);
    h.client.approve(&h.signers[0], &42);
    assert!(h.client.has_approved(&42, &h.signers[0]));

    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 3601);
    assert!(!h.client.has_approved(&42, &h.signers[0]));
    assert_eq!(h.client.get_approval_count(&42), 0);
}

#[test]
fn is_approved_drops_once_enough_approvals_expire() {
    let h = setup_with_ttl(3600);
    h.client.approve(&h.signers[0], &42);
    h.client.approve(&h.signers[1], &42);
    assert!(h.client.is_approved(&42));

    // signers[1] approves again later, refreshing only its own timestamp.
    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 1800);
    h.client.revoke(&h.signers[1], &42);
    h.client.approve(&h.signers[1], &42);

    // Advance past signers[0]'s original approval but not signers[1]'s
    // refreshed one.
    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 1900);
    assert!(!h.client.has_approved(&42, &h.signers[0]));
    assert!(h.client.has_approved(&42, &h.signers[1]));
    assert_eq!(h.client.get_approval_count(&42), 1);
    assert!(!h.client.is_approved(&42));
}

#[test]
fn a_signer_can_reapprove_after_their_own_approval_expires() {
    let h = setup_with_ttl(3600);
    h.client.approve(&h.signers[0], &42);

    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 3601);
    // Expired, not merely revoked — re-approving must not panic with
    // AlreadyApproved.
    h.client.approve(&h.signers[0], &42);
    assert!(h.client.has_approved(&42, &h.signers[0]));
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // NotYetApproved
fn revoke_rejects_an_already_expired_approval() {
    let h = setup_with_ttl(3600);
    h.client.approve(&h.signers[0], &42);

    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + 3601);
    h.client.revoke(&h.signers[0], &42);
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
    h.client.approve(&h.signers[0], &42);
    h.client.approve(&h.signers[1], &42);
    assert!(h.client.is_approved(&42));

    h.client.reset(&42);

    assert!(!h.client.has_approved(&42, &h.signers[0]));
    assert!(!h.client.has_approved(&42, &h.signers[1]));
    assert_eq!(h.client.get_approval_count(&42), 0);
    assert!(!h.client.is_approved(&42));
}

#[test]
fn action_id_can_be_approved_fresh_after_a_reset() {
    let h = setup();
    h.client.approve(&h.signers[0], &42);
    h.client.approve(&h.signers[1], &42);
    h.client.reset(&42);

    // The same id, reused for a later action, starts from a clean slate —
    // a single signer's leftover vote can't carry over.
    h.client.approve(&h.signers[2], &42);
    assert_eq!(h.client.get_approval_count(&42), 1);
    assert!(!h.client.is_approved(&42));
}

#[test]
fn reset_does_not_affect_other_action_ids() {
    let h = setup();
    h.client.approve(&h.signers[0], &1);
    h.client.approve(&h.signers[1], &1);
    h.client.approve(&h.signers[0], &2);
    h.client.approve(&h.signers[1], &2);

    h.client.reset(&1);

    assert!(!h.client.is_approved(&1));
    assert!(h.client.is_approved(&2));
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // NotYetApproved
fn reset_rejects_an_action_below_threshold() {
    let h = setup();
    h.client.approve(&h.signers[0], &42); // only 1 of 3, threshold is 2
    h.client.reset(&42);
}

// ─── events ──────────────────────────────────────────────────────────────────

#[test]
fn approve_emits_an_approved_event() {
    let h = setup();
    h.client.approve(&h.signers[0], &42);

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
    h.client.approve(&h.signers[0], &42);

    let events = h.env.events().all();
    let (_, topics, _data) = events.last().unwrap();
    let signer = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    let action_id = u64::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    assert_eq!(signer, h.signers[0]);
    assert_eq!(action_id, 42);
}

#[test]
fn revoke_emits_a_revoked_event() {
    let h = setup();
    h.client.approve(&h.signers[0], &42);
    h.client.revoke(&h.signers[0], &42);

    let events = h.env.events().all();
    let (contract_id, _topics, _data) = events.last().unwrap();
    assert_eq!(contract_id, h.client.address);
}

#[test]
fn revoked_event_topics_carry_the_signer_and_action_id() {
    let h = setup();
    h.client.approve(&h.signers[1], &7);
    h.client.revoke(&h.signers[1], &7);

    let events = h.env.events().all();
    let (_, topics, _data) = events.last().unwrap();
    let signer = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    let action_id = u64::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    assert_eq!(signer, h.signers[1]);
    assert_eq!(action_id, 7);
}

#[test]
fn reset_emits_a_reset_event_with_the_action_id_as_data() {
    let h = setup();
    h.client.approve(&h.signers[0], &42);
    h.client.approve(&h.signers[1], &42);
    h.client.reset(&42);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    assert_eq!(u64::try_from_val(&h.env, &data).unwrap(), 42);
}

// ─── require_auth is load-bearing where it exists, absent where it doesn't ─

/// Confirms initialize(, &NO_DELAYS, &None)'s lack of require_auth() is an intentional
/// design choice, not an untested oversight: it succeeds even with NO
/// auths mocked at all, unlike every other contract's initialize(, &NO_DELAYS, &None)
/// here (which all require the incoming admin's own signature). See
/// the doc comment on initialize(, &NO_DELAYS, &None) for why this is safe — approve()'s
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
        &NO_DELAYS,
        &None,
    );
    assert_eq!(client.get_signer_count(), 3);
}

#[test]
#[should_panic] // no auth was mocked at all — require_auth() has nothing to accept
fn approve_without_any_authorization_panics() {
    // Unlike initialize(, &NO_DELAYS, &None), approve() DOES require the signer's own
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
        &NO_DELAYS,
        &None,
    );

    client.approve(&signers[0], &42);
}

// ─── timelock tiers ──────────────────────────────────────────────────────────

const STANDARD: u64 = 24 * 60 * 60;
const CRITICAL: u64 = 72 * 60 * 60;

fn setup_timelocked<'a>(ttl: u64) -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().set_timestamp(1_000);
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
        &Delays {
            standard: STANDARD,
            critical: CRITICAL,
        },
        &None,
    );
    Harness {
        env,
        client,
        signers,
    }
}

fn advance(h: &Harness, seconds: u64) {
    h.env
        .ledger()
        .set_timestamp(h.env.ledger().timestamp() + seconds);
}

#[test]
fn emergency_class_is_executable_as_soon_as_threshold_is_reached() {
    let h = setup_timelocked(0);
    h.client.approve(&h.signers[0], &1);
    assert!(!h.client.is_executable(&1, &ActionClass::Emergency));
    h.client.approve(&h.signers[1], &1);
    assert!(h.client.is_executable(&1, &ActionClass::Emergency));
    assert!(!h.client.is_executable(&1, &ActionClass::Standard));
    assert!(!h.client.is_executable(&1, &ActionClass::Critical));
}

#[test]
fn each_class_becomes_executable_exactly_at_its_delay() {
    let h = setup_timelocked(0);
    h.client.approve(&h.signers[0], &1);
    advance(&h, 50);
    h.client.approve(&h.signers[1], &1);
    let reached = h.env.ledger().timestamp();
    assert_eq!(h.client.get_threshold_reached_at(&1), Some(reached));

    advance(&h, STANDARD - 1);
    assert!(!h.client.is_executable(&1, &ActionClass::Standard));
    advance(&h, 1);
    assert!(h.client.is_executable(&1, &ActionClass::Standard));
    assert!(!h.client.is_executable(&1, &ActionClass::Critical));

    h.env.ledger().set_timestamp(reached + CRITICAL - 1);
    assert!(!h.client.is_executable(&1, &ActionClass::Critical));
    advance(&h, 1);
    assert!(h.client.is_executable(&1, &ActionClass::Critical));
}

#[test]
fn a_third_approval_does_not_restart_the_timer() {
    let h = setup_timelocked(0);
    h.client.approve(&h.signers[0], &1);
    h.client.approve(&h.signers[1], &1);
    advance(&h, STANDARD - 10);
    h.client.approve(&h.signers[2], &1);
    advance(&h, 10);
    assert!(h.client.is_executable(&1, &ActionClass::Standard));
}

#[test]
fn revoke_during_the_delay_resets_the_timer() {
    let h = setup_timelocked(0);
    h.client.approve(&h.signers[0], &1);
    h.client.approve(&h.signers[1], &1);
    advance(&h, STANDARD / 2);
    h.client.revoke(&h.signers[1], &1);
    assert_eq!(h.client.get_threshold_reached_at(&1), None);

    h.client.approve(&h.signers[2], &1);
    let reached = h.env.ledger().timestamp();
    assert_eq!(h.client.get_threshold_reached_at(&1), Some(reached));
    advance(&h, STANDARD / 2);
    assert!(!h.client.is_executable(&1, &ActionClass::Standard));
    h.env.ledger().set_timestamp(reached + STANDARD);
    assert!(h.client.is_executable(&1, &ActionClass::Standard));
}

#[test]
fn an_approval_expiring_during_the_delay_resets_the_timer() {
    let ttl = STANDARD / 2;
    let h = setup_timelocked(ttl);
    h.client.approve(&h.signers[0], &1);
    h.client.approve(&h.signers[1], &1);
    advance(&h, ttl + 1); // both approvals expire: below threshold
    assert!(!h.client.is_executable(&1, &ActionClass::Emergency));

    h.client.approve(&h.signers[0], &1);
    h.client.approve(&h.signers[2], &1);
    let reached = h.env.ledger().timestamp();
    assert_eq!(h.client.get_threshold_reached_at(&1), Some(reached));
}

#[test]
fn reset_clears_the_threshold_timestamp() {
    let h = setup_timelocked(0);
    h.client.approve(&h.signers[0], &1);
    h.client.approve(&h.signers[1], &1);
    h.client.reset(&1);
    assert_eq!(h.client.get_threshold_reached_at(&1), None);
    assert!(!h.client.is_executable(&1, &ActionClass::Emergency));
}

#[test]
fn delays_are_exposed_and_emergency_is_always_zero() {
    let h = setup_timelocked(0);
    assert_eq!(h.client.get_delay(&ActionClass::Emergency), 0);
    assert_eq!(h.client.get_delay(&ActionClass::Standard), STANDARD);
    assert_eq!(h.client.get_delay(&ActionClass::Critical), CRITICAL);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")]
fn initialize_rejects_a_standard_delay_above_critical() {
    let env = Env::default();
    let contract_id = env.register_contract(None, Multisig);
    let client = MultisigClient::new(&env, &contract_id);
    client.initialize(
        &vec![&env, Address::generate(&env)],
        &1,
        &0,
        &Delays {
            standard: 2,
            critical: 1,
        },
        &None,
    );
}

// ─── custom account (__check_auth) ───────────────────────────────────────────

fn keys() -> [SigningKey; 3] {
    let mut k = [
        SigningKey::from_bytes(&[1; 32]),
        SigningKey::from_bytes(&[2; 32]),
        SigningKey::from_bytes(&[3; 32]),
    ];
    k.sort_by_key(|k| k.verifying_key().to_bytes());
    k
}

fn setup_account(env: &Env, allowed: Vec<Address>) -> Address {
    let contract_id = env.register_contract(None, Multisig);
    let pubkeys = keys().map(|k| BytesN::from_array(env, &k.verifying_key().to_bytes()));
    MultisigClient::new(env, &contract_id).initialize(
        &vec![env, Address::generate(env)],
        &1,
        &0,
        &NO_DELAYS,
        &Some(AccountConfig {
            signers: vec![
                env,
                pubkeys[0].clone(),
                pubkeys[1].clone(),
                pubkeys[2].clone(),
            ],
            threshold: 2,
            allowed_contracts: allowed,
        }),
    );
    contract_id
}

fn sign(env: &Env, key: &SigningKey, payload: &BytesN<32>) -> AccountSignature {
    AccountSignature {
        public_key: BytesN::from_array(env, &key.verifying_key().to_bytes()),
        signature: BytesN::from_array(env, &key.sign(&payload.to_array()).to_bytes()),
    }
}

fn contract_context(env: &Env, contract: &Address) -> Vec<Context> {
    vec![
        env,
        Context::Contract(ContractContext {
            contract: contract.clone(),
            fn_name: Symbol::new(env, "pause"),
            args: vec![env],
        }),
    ]
}

fn check(
    env: &Env,
    account: &Address,
    payload: &BytesN<32>,
    sigs: Vec<AccountSignature>,
    ctx: &Vec<Context>,
) -> Result<(), Result<crate::Error, soroban_sdk::InvokeError>> {
    env.try_invoke_contract_check_auth::<crate::Error>(account, payload, sigs.into_val(env), ctx)
}

#[test]
fn check_auth_accepts_m_distinct_sorted_signer_signatures() {
    let env = Env::default();
    let account = setup_account(&env, vec![&env]);
    let payload = BytesN::random(&env);
    let k = keys();
    let ctx = contract_context(&env, &Address::generate(&env));
    let sigs = vec![
        &env,
        sign(&env, &k[0], &payload),
        sign(&env, &k[2], &payload),
    ];
    assert_eq!(check(&env, &account, &payload, sigs, &ctx), Ok(()));
}

#[test]
fn check_auth_rejects_too_few_signatures() {
    let env = Env::default();
    let account = setup_account(&env, vec![&env]);
    let payload = BytesN::random(&env);
    let ctx = contract_context(&env, &Address::generate(&env));
    let sigs = vec![&env, sign(&env, &keys()[0], &payload)];
    assert_eq!(
        check(&env, &account, &payload, sigs, &ctx),
        Err(Ok(crate::Error::InsufficientSignatures))
    );
}

#[test]
fn check_auth_rejects_duplicate_and_unsorted_signatures() {
    let env = Env::default();
    let account = setup_account(&env, vec![&env]);
    let payload = BytesN::random(&env);
    let k = keys();
    let ctx = contract_context(&env, &Address::generate(&env));
    let dup = vec![
        &env,
        sign(&env, &k[0], &payload),
        sign(&env, &k[0], &payload),
    ];
    assert_eq!(
        check(&env, &account, &payload, dup, &ctx),
        Err(Ok(crate::Error::UnsortedSignatures))
    );
    let unsorted = vec![
        &env,
        sign(&env, &k[1], &payload),
        sign(&env, &k[0], &payload),
    ];
    assert_eq!(
        check(&env, &account, &payload, unsorted, &ctx),
        Err(Ok(crate::Error::UnsortedSignatures))
    );
}

#[test]
fn check_auth_rejects_a_non_signer_key() {
    let env = Env::default();
    let account = setup_account(&env, vec![&env]);
    let payload = BytesN::random(&env);
    let mut k = [keys()[0].clone(), SigningKey::from_bytes(&[9; 32])];
    k.sort_by_key(|k| k.verifying_key().to_bytes());
    let ctx = contract_context(&env, &Address::generate(&env));
    let sigs = vec![
        &env,
        sign(&env, &k[0], &payload),
        sign(&env, &k[1], &payload),
    ];
    assert_eq!(
        check(&env, &account, &payload, sigs, &ctx),
        Err(Ok(crate::Error::NotASigner))
    );
}

#[test]
fn check_auth_rejects_a_signature_over_a_different_payload() {
    let env = Env::default();
    let account = setup_account(&env, vec![&env]);
    let payload = BytesN::random(&env);
    let other = BytesN::random(&env);
    let k = keys();
    let ctx = contract_context(&env, &Address::generate(&env));
    let sigs = vec![&env, sign(&env, &k[0], &payload), sign(&env, &k[1], &other)];
    assert!(check(&env, &account, &payload, sigs, &ctx).is_err());
}

#[test]
fn check_auth_enforces_the_allowed_contracts_policy() {
    let env = Env::default();
    let allowed = Address::generate(&env);
    let account = setup_account(&env, vec![&env, allowed.clone()]);
    let payload = BytesN::random(&env);
    let k = keys();
    let sigs = vec![
        &env,
        sign(&env, &k[0], &payload),
        sign(&env, &k[1], &payload),
    ];
    assert_eq!(
        check(
            &env,
            &account,
            &payload,
            sigs.clone(),
            &contract_context(&env, &allowed)
        ),
        Ok(())
    );
    assert_eq!(
        check(
            &env,
            &account,
            &payload,
            sigs,
            &contract_context(&env, &Address::generate(&env))
        ),
        Err(Ok(crate::Error::ContextNotAllowed))
    );
}

#[test]
fn check_auth_fails_when_no_account_is_configured() {
    let h = setup();
    let payload = BytesN::random(&h.env);
    let ctx = contract_context(&h.env, &Address::generate(&h.env));
    assert_eq!(
        check(&h.env, &h.client.address, &payload, vec![&h.env], &ctx),
        Err(Ok(crate::Error::AccountNotConfigured))
    );
}
