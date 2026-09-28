#![cfg(test)]

use crate::{Vault, VaultClient};
use multisig::{Multisig, MultisigClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _},
    token, Address, Env, TryFromVal,
};

struct Harness<'a> {
    env: Env,
    client: VaultClient<'a>,
    token: Address,
    admin: Address,
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_contract = env.register_stellar_asset_contract_v2(token_admin);
    let token_address = token_contract.address();

    let contract_id = env.register_contract(None, Vault);
    let client = VaultClient::new(&env, &contract_id);
    client.initialize(&admin, &token_address);

    Harness {
        env,
        client,
        token: token_address,
        admin,
    }
}

fn mint(h: &Harness, to: &Address, amount: i128) {
    token::StellarAssetClient::new(&h.env, &h.token).mint(to, &amount);
}

fn balance(h: &Harness, of: &Address) -> i128 {
    token::Client::new(&h.env, &h.token).balance(of)
}

#[test]
fn initialize_sets_admin_and_token() {
    let h = setup();
    assert_eq!(h.client.get_admin(), h.admin);
    assert_eq!(h.client.get_token(), h.token);
    assert_eq!(h.client.get_total_escrowed(), 0);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // AlreadyInitialized
fn initialize_twice_panics() {
    let h = setup();
    h.client.initialize(&h.admin, &h.token);
}

#[test]
#[should_panic] // no auth was mocked at all — require_auth() has nothing to accept
fn initialize_without_any_authorization_panics() {
    // Deliberately skips mock_all_auths(): every other test in this file
    // uses it (mock_all_auths() arms for the env's whole lifetime once
    // called, so there's no way to "unmock" partway through a test to
    // check a LATER call specifically) — this test exists just to
    // confirm require_auth() is actually load-bearing on initialize(),
    // not a no-op, by never arming it in the first place. Mirrors
    // options_market's identically-named test.
    let env = Env::default();
    let admin = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token_address = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let contract_id = env.register_contract(None, Vault);
    let client = VaultClient::new(&env, &contract_id);
    client.initialize(&admin, &token_address);
}

// ─── deposit ─────────────────────────────────────────────────────────────────

#[test]
fn deposit_pulls_funds_and_credits_the_tags_ledger() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);

    h.client.deposit(&depositor, &7, &400);

    assert_eq!(h.client.balance_of(&7), 400);
    assert_eq!(h.client.get_total_escrowed(), 400);
    assert_eq!(balance(&h, &depositor), 600);
    assert_eq!(balance(&h, &h.client.address), 400);
}

#[test]
fn deposit_accumulates_across_multiple_calls_for_the_same_tag() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);

    h.client.deposit(&depositor, &7, &100);
    h.client.deposit(&depositor, &7, &250);

    assert_eq!(h.client.balance_of(&7), 350);
    assert_eq!(h.client.get_total_escrowed(), 350);
}

#[test]
fn deposit_keeps_different_tags_independent() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);

    h.client.deposit(&depositor, &1, &100);
    h.client.deposit(&depositor, &2, &250);

    assert_eq!(h.client.balance_of(&1), 100);
    assert_eq!(h.client.balance_of(&2), 250);
    assert_eq!(h.client.get_total_escrowed(), 350);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidAmount
fn deposit_rejects_a_non_positive_amount() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &0);
}

// ─── withdraw ────────────────────────────────────────────────────────────────

#[test]
fn withdraw_pays_out_and_debits_the_tags_ledger() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    let payee = Address::generate(&h.env);
    h.client.withdraw(&7, &payee, &150);

    assert_eq!(h.client.balance_of(&7), 250);
    assert_eq!(h.client.get_total_escrowed(), 250);
    assert_eq!(balance(&h, &payee), 150);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // InsufficientEscrowBalance
fn withdraw_rejects_more_than_the_tags_own_balance() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    // Tag 7 only has 400 earmarked — this is exactly the gap the vault
    // exists to close: a withdrawal can't draw on tag 8's (nonexistent)
    // balance just because the vault's raw token balance happens to be
    // nonzero from OTHER deposits.
    let payee = Address::generate(&h.env);
    h.client.withdraw(&7, &payee, &401);
}

#[test]
fn withdraw_cannot_drain_another_tags_deposit() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &100);
    h.client.deposit(&depositor, &2, &900);

    let payee = Address::generate(&h.env);
    h.client.withdraw(&1, &payee, &100);

    // Tag 1 is now fully withdrawn; tag 2's much larger balance must be
    // completely unaffected.
    assert_eq!(h.client.balance_of(&1), 0);
    assert_eq!(h.client.balance_of(&2), 900);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidAmount
fn withdraw_rejects_a_non_positive_amount() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);
    h.client.withdraw(&7, &depositor, &0);
}

// ─── transfer_admin ─────────────────────────────────────────────────────────

#[test]
fn transfer_admin_hands_off_control() {
    let h = setup();
    assert_eq!(h.client.get_admin(), h.admin);

    let new_admin = Address::generate(&h.env);
    h.client.transfer_admin(&new_admin);
    assert_eq!(h.client.get_admin(), new_admin);
}

#[test]
fn transfer_admin_emits_an_admin_transferred_event_with_old_and_new_admin() {
    let h = setup();
    let new_admin = Address::generate(&h.env);
    h.client.transfer_admin(&new_admin);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    let (old_admin, event_new_admin) = <(Address, Address)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(old_admin, h.admin);
    assert_eq!(event_new_admin, new_admin);
}

// ─── pause / unpause ────────────────────────────────────────────────────────

#[test]
fn pause_blocks_deposit_and_withdraw_unpause_restores_them() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    assert!(!h.client.is_paused());
    h.client.pause();
    assert!(h.client.is_paused());

    h.client.unpause();
    assert!(!h.client.is_paused());

    // Both sides work again post-unpause.
    h.client.deposit(&depositor, &7, &100);
    h.client.withdraw(&7, &depositor, &50);
    assert_eq!(h.client.balance_of(&7), 450);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // ContractPaused
fn deposit_is_rejected_while_paused() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.pause();
    h.client.deposit(&depositor, &7, &400);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // ContractPaused
fn withdraw_is_rejected_while_paused() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    h.client.pause();
    h.client.withdraw(&7, &depositor, &100);
}

// ─── events ──────────────────────────────────────────────────────────────────

#[test]
fn deposit_emits_a_deposited_event_with_the_amount_as_data() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);

    h.client.deposit(&depositor, &7, &400);

    let events = h.env.events().all();
    let (contract_id, topics, data) = events.last().unwrap();
    assert_eq!(contract_id, h.client.address);
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 400);

    // from and tag live in topics, not data — an indexer filtering
    // "deposits from this wallet" or "deposits for this tag" reads
    // them from here.
    let topic_from = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    let topic_tag = u64::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    assert_eq!(topic_from, depositor);
    assert_eq!(topic_tag, 7);
}

#[test]
fn withdraw_emits_a_withdrawn_event_with_the_amount_as_data() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    h.client.withdraw(&7, &depositor, &150);

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 150);

    // to and tag live in topics, not data.
    let topic_to = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    let topic_tag = u64::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    assert_eq!(topic_to, depositor);
    assert_eq!(topic_tag, 7);
}

// ─── sweep_untagged ─────────────────────────────────────────────────────────

#[test]
fn sweep_untagged_recovers_a_direct_transfer_that_bypassed_deposit() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    // Simulate a stray direct transfer straight to the vault's own
    // address, bypassing deposit() entirely — those funds aren't credited
    // to any tag.
    mint(&h, &h.client.address, 250);

    let rescuer = Address::generate(&h.env);
    let swept = h.client.sweep_untagged(&rescuer);

    assert_eq!(swept, 250);
    assert_eq!(balance(&h, &rescuer), 250);
    // Tag 7's own escrowed balance must be completely untouched.
    assert_eq!(h.client.balance_of(&7), 400);
    assert_eq!(h.client.get_total_escrowed(), 400);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // NoUntaggedFunds
fn sweep_untagged_rejects_when_the_balance_exactly_matches_the_ledger() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    // No stray transfer this time — the vault's actual balance (400)
    // exactly matches get_total_escrowed (400), so there's nothing to
    // sweep.
    let rescuer = Address::generate(&h.env);
    h.client.sweep_untagged(&rescuer);
}

#[test]
fn sweep_untagged_emits_a_swept_untagged_event() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);
    mint(&h, &h.client.address, 250);

    let rescuer = Address::generate(&h.env);
    h.client.sweep_untagged(&rescuer);

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 250);

    // to lives in topics, not data.
    let topic_to = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    assert_eq!(topic_to, rescuer);
}

// ─── transfer_tag ───────────────────────────────────────────────────────────

#[test]
fn transfer_tag_moves_escrow_without_any_token_movement() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &400);

    let vault_balance_before = balance(&h, &h.client.address);
    h.client.transfer_tag(&1, &2, &150);

    assert_eq!(h.client.balance_of(&1), 250);
    assert_eq!(h.client.balance_of(&2), 150);
    // TotalEscrowed unchanged — nothing entered or left the vault.
    assert_eq!(h.client.get_total_escrowed(), 400);
    assert_eq!(balance(&h, &h.client.address), vault_balance_before);
}

#[test]
fn transfer_tag_accumulates_into_an_already_funded_destination() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &400);
    h.client.deposit(&depositor, &2, &100);

    h.client.transfer_tag(&1, &2, &400);

    assert_eq!(h.client.balance_of(&1), 0);
    assert_eq!(h.client.balance_of(&2), 500);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // InsufficientEscrowBalance
fn transfer_tag_rejects_more_than_the_source_tags_balance() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &100);

    h.client.transfer_tag(&1, &2, &101);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // InvalidAmount
fn transfer_tag_rejects_a_non_positive_amount() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &100);

    h.client.transfer_tag(&1, &2, &0);
}

#[test]
fn transfer_tag_emits_a_tag_transferred_event() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &400);

    h.client.transfer_tag(&1, &2, &150);

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 150);

    // from_tag and to_tag live in topics, not data.
    let topic_from_tag = u64::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    let topic_to_tag = u64::try_from_val(&h.env, &topics.get(2).unwrap()).unwrap();
    assert_eq!(topic_from_tag, 1);
    assert_eq!(topic_to_tag, 2);
}

// ─── cross-contract: pause_via_multisig / transfer_admin_via_multisig ─────

/// Deploys a real multisig contract in the SAME Env as the vault under
/// test, with a 2-of-3 threshold.
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
        &0,
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
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
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
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
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
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
fn transfer_admin_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);
    let new_admin = Address::generate(&h.env);

    h.client
        .transfer_admin_via_multisig(&multisig_id, &99u64, &new_admin);
}

// ─── cross-contract: withdraw_via_multisig ─────────────────────────────────

#[test]
fn withdraw_via_multisig_pays_out_once_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = 4u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let payee = Address::generate(&h.env);
    h.client
        .withdraw_via_multisig(&multisig_id, &action_id, &7, &payee, &150);

    assert_eq!(h.client.balance_of(&7), 250);
    assert_eq!(balance(&h, &payee), 150);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
fn withdraw_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);
    let (multisig_id, _signers) = setup_multisig(&h);

    let payee = Address::generate(&h.env);
    h.client
        .withdraw_via_multisig(&multisig_id, &99u64, &7, &payee, &150);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // InsufficientEscrowBalance
fn withdraw_via_multisig_still_enforces_the_tags_own_balance_once_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &7, &400);

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = 5u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    // Approval authorizes WHO can call this, not draining more than
    // tag 7 actually has earmarked.
    let payee = Address::generate(&h.env);
    h.client
        .withdraw_via_multisig(&multisig_id, &action_id, &7, &payee, &401);
}

// ─── cross-contract: transfer_tag_via_multisig ─────────────────────────────

#[test]
fn transfer_tag_via_multisig_moves_escrow_once_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &400);

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = 6u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client
        .transfer_tag_via_multisig(&multisig_id, &action_id, &1, &2, &150);

    assert_eq!(h.client.balance_of(&1), 250);
    assert_eq!(h.client.balance_of(&2), 150);
    assert_eq!(h.client.get_total_escrowed(), 400); // unaffected by the reassignment
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
fn transfer_tag_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &400);
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client
        .transfer_tag_via_multisig(&multisig_id, &99u64, &1, &2, &150);
}

// ─── cross-contract: sweep_untagged_via_multisig ───────────────────────────

#[test]
fn sweep_untagged_via_multisig_recovers_once_approved() {
    let h = setup();
    let depositor = Address::generate(&h.env);
    mint(&h, &depositor, 1_000);
    h.client.deposit(&depositor, &1, &400);
    // A stray direct transfer that bypassed deposit() entirely.
    mint(&h, &h.client.address, 100);

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = 7u64;
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let recovered_to = Address::generate(&h.env);
    let swept = h
        .client
        .sweep_untagged_via_multisig(&multisig_id, &action_id, &recovered_to);

    assert_eq!(swept, 100);
    assert_eq!(balance(&h, &recovered_to), 100);
}

#[test]
#[should_panic(expected = "Error(Contract, #6)")] // Unauthorized
fn sweep_untagged_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    mint(&h, &h.client.address, 100);
    let (multisig_id, _signers) = setup_multisig(&h);

    let recovered_to = Address::generate(&h.env);
    h.client
        .sweep_untagged_via_multisig(&multisig_id, &99u64, &recovered_to);
}

// ── ActiveTags index / verify_ledger (#84) ──────────────────────────────────

/// Full off-chain-style reconciliation: page through verify_ledger and
/// check the partial sums add up to TotalEscrowed, and that get_tags
/// agrees with balance_of for every indexed tag.
fn reconcile(h: &Harness, page: u32) {
    let count = h.client.get_tag_count();
    let (mut cursor, mut sum) = (0u32, 0i128);
    while cursor < count {
        let (partial, next) = h.client.verify_ledger(&cursor, &page);
        for tag in h.client.get_tags(&cursor, &page).iter() {
            assert!(h.client.balance_of(&tag) > 0);
        }
        sum += partial;
        cursor = next;
    }
    assert_eq!(cursor, count);
    assert_eq!(sum, h.client.get_total_escrowed());
    assert!(sum <= balance(h, &h.client.address));
}

#[test]
fn tag_index_tracks_nonzero_balances_with_swap_remove() {
    let h = setup();
    let user = Address::generate(&h.env);
    mint(&h, &user, 1_000);
    h.client.deposit(&user, &1, &100);
    h.client.deposit(&user, &2, &200);
    h.client.deposit(&user, &3, &300);
    assert_eq!(
        h.client.get_tags(&0, &10),
        soroban_sdk::vec![&h.env, 1, 2, 3]
    );

    h.client.withdraw(&1, &user, &100);
    assert_eq!(h.client.get_tags(&0, &10), soroban_sdk::vec![&h.env, 3, 2]);

    h.client.transfer_tag(&2, &4, &200);
    assert_eq!(h.client.get_tags(&0, &10), soroban_sdk::vec![&h.env, 3, 4]);
    assert_eq!(h.client.get_tag_count(), 2);
    assert_eq!(h.client.verify_ledger(&0, &1), (300, 1));
    assert_eq!(h.client.verify_ledger(&1, &1), (200, 2));
    assert_eq!(h.client.verify_ledger(&5, &1), (0, 2));
    reconcile(&h, 1);
}

/// Deterministic fuzz: 10,000 random deposit/withdraw/transfer_tag ops over a
/// small tag space, reconciling the ledger after each batch of ops. Run with
/// `--features invariants` to also check the post-condition inside every call.
/// Slow (minutes), so ignored by default; CI runs it with
/// `cargo test --features invariants -- --include-ignored`.
#[test]
#[ignore]
fn fuzz_random_ops_preserve_ledger_invariant() {
    let h = setup();
    h.env.budget().reset_unlimited();
    let user = Address::generate(&h.env);
    mint(&h, &user, i128::MAX / 4);

    let mut seed: u64 = 0x2545_f491_4f6c_dd1d;
    let mut next = |m: u64| {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        seed % m
    };
    for i in 0..10_000u32 {
        let tag = next(16);
        let amount = next(1_000) as i128 + 1;
        match next(3) {
            0 => h.client.deposit(&user, &tag, &amount),
            1 => {
                let bal = h.client.balance_of(&tag);
                if bal > 0 {
                    h.client.withdraw(&tag, &user, &(amount.min(bal)));
                }
            }
            _ => {
                let bal = h.client.balance_of(&tag);
                if bal > 0 {
                    h.client.transfer_tag(&tag, &next(16), &(amount.min(bal)));
                }
            }
        }
        if i % 500 == 0 {
            h.env.budget().reset_unlimited();
            reconcile(&h, 7);
        }
    }
    reconcile(&h, 7);
}

// ── Batch withdraw / transfer_tag (#85) ─────────────────────────────────────

#[test]
fn withdraw_batch_matches_sequential_and_aggregates_recipients() {
    let h = setup();
    let user = Address::generate(&h.env);
    let a = Address::generate(&h.env);
    let b = Address::generate(&h.env);
    mint(&h, &user, 1_000);
    h.client.deposit(&user, &1, &500);
    h.client.deposit(&user, &2, &500);

    let ops = soroban_sdk::vec![
        &h.env,
        (1u64, a.clone(), 100i128),
        (1u64, b.clone(), 150i128),
        (2u64, a.clone(), 500i128),
    ];
    h.client.withdraw_batch(&ops);

    assert_eq!(balance(&h, &a), 600);
    assert_eq!(balance(&h, &b), 150);
    assert_eq!(h.client.balance_of(&1), 250);
    assert_eq!(h.client.balance_of(&2), 0);
    assert_eq!(h.client.get_total_escrowed(), 250);
    assert_eq!(h.client.get_tags(&0, &10), soroban_sdk::vec![&h.env, 1]);
    // One `withdrawn` event per ledger entry, not per recipient.
    let withdrawn = h
        .env
        .events()
        .all()
        .iter()
        .filter(|(c, topics, _)| {
            *c == h.client.address
                && soroban_sdk::Symbol::try_from_val(&h.env, &topics.get(0).unwrap())
                    == Ok(soroban_sdk::Symbol::new(&h.env, "withdrawn"))
        })
        .count();
    assert_eq!(withdrawn, 3);
    reconcile(&h, 10);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // InsufficientEscrowBalance
fn withdraw_batch_detects_same_tag_overdraw_midway() {
    let h = setup();
    let user = Address::generate(&h.env);
    mint(&h, &user, 100);
    h.client.deposit(&user, &1, &100);
    let ops = soroban_sdk::vec![
        &h.env,
        (1u64, user.clone(), 60i128),
        (1u64, user.clone(), 60i128)
    ];
    h.client.withdraw_batch(&ops);
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")] // InvalidBatchSize
fn withdraw_batch_rejects_empty_batch() {
    let h = setup();
    h.client.withdraw_batch(&soroban_sdk::Vec::new(&h.env));
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")] // InvalidBatchSize
fn transfer_tag_batch_rejects_oversized_batch() {
    let h = setup();
    let mut ops = soroban_sdk::Vec::new(&h.env);
    for i in 0..=crate::MAX_BATCH as u64 {
        ops.push_back((i, i + 1, 1i128));
    }
    h.client.transfer_tag_batch(&ops);
}

#[test]
fn transfer_tag_batch_chains_through_in_flight_balances() {
    let h = setup();
    let user = Address::generate(&h.env);
    mint(&h, &user, 100);
    h.client.deposit(&user, &1, &100);
    let ops = soroban_sdk::vec![&h.env, (1u64, 2u64, 100i128), (2u64, 3u64, 40i128)];
    h.client.transfer_tag_batch(&ops);
    assert_eq!(h.client.balance_of(&1), 0);
    assert_eq!(h.client.balance_of(&2), 60);
    assert_eq!(h.client.balance_of(&3), 40);
    assert_eq!(h.client.get_total_escrowed(), 100);
    reconcile(&h, 10);
}
