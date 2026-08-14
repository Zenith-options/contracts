#![cfg(test)]

use crate::{Vault, VaultClient};
use soroban_sdk::{testutils::Address as _, token, Address, Env};

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
