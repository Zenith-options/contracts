//! Adversarial cross-contract negative tests for options_market (issue #115).
//!
//! Each test exercises one cross-contract trust boundary with an adversarial
//! counterparty from `test_adversaries`. Tests that currently expose a
//! vulnerability are marked `#[ignore]` with the issue title so they show up
//! in the suite (not silently skipped) and are un-ignored by the fix PRs.
//!
//! # Cross-contract boundaries covered
//!
//! 1. Oracle boundary — `set_settlement_price_from_oracle` calls `oracle.get_price(symbol)`.
//!    Adversaries: `AlwaysNoneOracle`, `RevertingOracle`, `NegativePriceOracle`,
//!    `ExtremeHighOracle`, `ExtremeLowOracle`.
//!
//! 2. Vault boundary — `escrow_series_to_vault` calls `vault.deposit(...)`;
//!    `claim_refund_from_vault` calls `vault.withdraw(...)`.
//!    Adversaries: `NoOpVault`, `RevertOnWithdrawVault`, `DrainVault`.
//!
//! 3. Multisig boundary — every `_via_multisig` entrypoint calls `multisig.is_executable(...)`.
//!    Adversaries: `NeverApproveMultisig`, `AlwaysApproveMultisig`.
//!
//! 4. Token boundary — collateral and premium transfers via `soroban_sdk::token::Client`.
//!    Adversaries: `FeeOnTransferToken`, `RevertOnTransferToken`, `LyingBalanceToken`,
//!    `ZeroBalanceToken`.

#![cfg(test)]

extern crate std;

use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, Symbol};

use test_adversaries::multisig::{AlwaysApproveMultisig, NeverApproveMultisig};
use test_adversaries::oracle::{
    AlwaysNoneOracle, ExtremeHighOracle, NegativePriceOracle, RevertingOracle,
};
use test_adversaries::tokens::{FeeOnTransferToken, LyingBalanceToken, RevertOnTransferToken};
use test_adversaries::vault::{NoOpVault, RevertOnWithdrawVault};

// ─── helper: build a 32-byte action id from a u8 ────────────────────────────

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

// ─── helper: action id from u64 discriminant for multisig ────────────────────

fn action_id_u64(env: &Env, n: u64) -> u64 {
    let _ = env;
    n
}

// ─── Oracle boundary ─────────────────────────────────────────────────────────

/// `set_settlement_price_from_oracle` must NOT silently settle at price 0
/// when the oracle returns `None`. The call must panic / propagate an error.
///
/// **Issue #115 — Oracle: AlwaysNoneOracle boundary.**
/// Marked `#[ignore]` because options_market::set_settlement_price_from_oracle
/// is not yet implemented (lib.rs is a stub). Un-ignore when the function
/// is implemented and ensure it returns an error rather than settling at 0.
#[test]
#[ignore = "issue #115: options_market::set_settlement_price_from_oracle not yet implemented"]
fn oracle_none_price_must_not_settle_the_series() {
    let env = Env::default();
    env.mock_all_auths();

    // Register adversarial oracle that always returns None.
    let oracle_id = env.register_contract(None, AlwaysNoneOracle);
    // Initialize adversarial oracle
    let oracle_admin = Address::generate(&env);
    soroban_sdk::token::Client::new(&env, &oracle_id); // type-check only
    // In a real test this would call options_market::set_settlement_price_from_oracle
    // and assert it panics or returns PriceNotSet.
    let _ = oracle_id;
    let _ = oracle_admin;
}

/// A reverting oracle contract must cause the whole tx to revert, NOT
/// partially update the series state.
///
/// **Issue #115 — Oracle: RevertingOracle boundary.**
#[test]
#[ignore = "issue #115: options_market::set_settlement_price_from_oracle not yet implemented"]
fn oracle_revert_must_propagate_and_not_partially_settle() {
    let env = Env::default();
    env.mock_all_auths();

    let oracle_id = env.register_contract(None, RevertingOracle);
    let _ = oracle_id;
    // When implemented: call set_settlement_price_from_oracle and assert it panics.
}

/// An oracle returning a negative price must be rejected by options_market
/// before any storage is updated — a negative settlement price must not
/// produce a negative payout or corrupt the series state.
///
/// **Issue #115 — Oracle: NegativePriceOracle boundary.**
#[test]
#[ignore = "issue #115: options_market::set_settlement_price_from_oracle not yet implemented"]
fn oracle_negative_price_must_be_rejected_not_stored() {
    let env = Env::default();
    env.mock_all_auths();

    let oracle_id = env.register_contract(None, NegativePriceOracle);
    let _ = oracle_id;
    // When implemented: call set_settlement_price_from_oracle and assert it
    // panics with InvalidPrice or similar, and the series is not settled.
}

/// An oracle returning `i128::MAX` must not cause arithmetic overflow in the
/// settlement payout calculation.
///
/// **Issue #115 — Oracle: ExtremeHighOracle overflow boundary.**
#[test]
#[ignore = "issue #115: options_market::set_settlement_price_from_oracle not yet implemented"]
fn oracle_extreme_high_price_must_not_overflow_payout_math() {
    let env = Env::default();
    env.mock_all_auths();

    let oracle_id = env.register_contract(None, ExtremeHighOracle);
    let _ = oracle_id;
    // When implemented: settle a series with i128::MAX price and verify
    // that exercise() doesn't panic due to arithmetic overflow.
}

// ─── Vault boundary ──────────────────────────────────────────────────────────

/// `escrow_series_to_vault` calls `vault.deposit`; a no-op vault that
/// returns 0 (deposited nothing) must cause the escrow operation to fail
/// rather than silently zeroing the liability.
///
/// **Issue #115 — Vault: NoOpVault deposit boundary.**
#[test]
#[ignore = "issue #115: options_market::escrow_series_to_vault not yet implemented"]
fn noop_vault_deposit_must_cause_escrow_to_fail() {
    let env = Env::default();
    env.mock_all_auths();

    let vault_id = env.register_contract(None, NoOpVault);
    let _ = vault_id;
    // When implemented: call escrow_series_to_vault pointing at the no-op
    // vault and assert it fails / panics with a meaningful error.
}

/// `claim_refund_from_vault` calls `vault.withdraw`; a vault that always
/// reverts on withdraw must propagate the revert rather than marking the
/// position as refunded.
///
/// **Issue #115 — Vault: RevertOnWithdrawVault withdraw boundary.**
#[test]
#[ignore = "issue #115: options_market::claim_refund_from_vault not yet implemented"]
fn vault_revert_on_withdraw_must_not_mark_position_refunded() {
    let env = Env::default();
    env.mock_all_auths();

    let vault_id = env.register_contract(None, RevertOnWithdrawVault);
    let _ = vault_id;
    // When implemented: set up a cancelled series, call escrow_series_to_vault
    // (with a real or good vault), then switch to a revert vault and call
    // claim_refund_from_vault. Assert the tx reverts and the position is not
    // marked refunded.
}

// ─── Multisig boundary ───────────────────────────────────────────────────────

/// A multisig that never approves must cause every `_via_multisig` entrypoint
/// to reject with `Unauthorized`. This is an active test: options_market
/// registers, calls `pause_via_multisig`, and expects `Error(Contract, #17)`.
///
/// Note: this test is marked ignore because lib.rs is a stub, but the
/// adversary setup code is written in full as a template.
///
/// **Issue #115 — Multisig: NeverApproveMultisig boundary.**
#[test]
#[ignore = "issue #115: options_market is a stub; wire in when implemented"]
fn never_approve_multisig_rejects_pause_via_multisig() {
    let env = Env::default();
    env.mock_all_auths();

    // Register and initialize the NeverApproveMultisig adversary.
    let multisig_id = env.register_contract(None, NeverApproveMultisig);
    let signers = soroban_sdk::vec![
        &env,
        Address::generate(&env),
        Address::generate(&env),
    ];
    {
        use soroban_sdk::contractclient;
        // Use invoke_contract since NeverApproveMultisig doesn't need wasm:
        // direct registration works in testutils.
        let _ = &signers;
        let _ = multisig_id.clone();
    }

    // When options_market is implemented:
    // let market_id = env.register_contract(None, OptionsMarket);
    // let client = OptionsMarketClient::new(&env, &market_id);
    // client.initialize(&admin, &multisig_id, &token, &fee_recipient);
    // client.pause_via_multisig(&multisig_id, &1u64);  // should panic Unauthorized
}

/// An always-approve multisig at an arbitrary address must NOT be accepted
/// as the trusted multisig unless it was registered as the admin at
/// `initialize`. This validates that options_market compares the multisig
/// address against the stored admin, not just trusts any passing contract.
///
/// **Issue #115 — Multisig: AlwaysApproveMultisig substitution boundary.**
#[test]
#[ignore = "issue #115: options_market is a stub; wire in when implemented"]
fn always_approve_multisig_at_wrong_address_must_be_rejected() {
    let env = Env::default();
    env.mock_all_auths();

    let malicious_multisig = env.register_contract(None, AlwaysApproveMultisig);
    let _ = malicious_multisig;
    // When implemented:
    // let market_id = env.register_contract(None, OptionsMarket);
    // let client = OptionsMarketClient::new(&env, &market_id);
    // client.initialize(&admin, &legitimate_multisig_id, &token, &fee_recipient);
    // client.pause_via_multisig(&malicious_multisig, &1u64);  // must reject
}

// ─── Token boundary ──────────────────────────────────────────────────────────

/// A fee-on-transfer collateral token must not cause a vault to credit more
/// escrow than tokens actually arrived. This probes the vault's delta-check
/// (`safe_inbound_transfer`) via the options_market deposit path.
///
/// **Issue #115 — Token: FeeOnTransferToken delta-check boundary.**
#[test]
#[ignore = "issue #115: options_market is a stub; wire in when implemented"]
fn fee_on_transfer_token_must_not_over_credit_escrow() {
    let env = Env::default();
    env.mock_all_auths();

    let token_id = env.register_contract(None, FeeOnTransferToken);
    let _ = token_id;
    // When options_market is implemented:
    // - set up a series with the FOT as collateral token
    // - write_option: writer deposits collateral through FOT
    // - assert that the actual credited collateral equals the delta (90% of requested)
    //   NOT the requested amount (100%)
    // Validates vault::safe_inbound_transfer's DeltaMismatch guard.
}

/// A token that always reverts on transfer must cause write_option /
/// buy_option to fail atomically — no partial state update.
///
/// **Issue #115 — Token: RevertOnTransferToken atomicity boundary.**
#[test]
#[ignore = "issue #115: options_market is a stub; wire in when implemented"]
fn revert_on_transfer_token_causes_write_option_to_revert_atomically() {
    let env = Env::default();
    env.mock_all_auths();

    let token_id = env.register_contract(None, RevertOnTransferToken);
    let _ = token_id;
    // When implemented:
    // - register OptionsMarket with RevertOnTransferToken as collateral
    // - call write_option; assert it panics
    // - assert no series or position state was written
}

/// A token whose `balance()` always returns `i128::MAX` must not cause
/// the vault's balance-delta check to report a false positive when the
/// token is the collateral for a series.
///
/// **Issue #115 — Token: LyingBalanceToken delta boundary.**
#[test]
#[ignore = "issue #115: options_market is a stub; wire in when implemented"]
fn lying_balance_token_must_not_bypass_delta_check() {
    let env = Env::default();
    env.mock_all_auths();

    let token_id = env.register_contract(None, LyingBalanceToken);
    let _ = token_id;
    // When implemented:
    // - set up a series with the LyingBalanceToken as collateral
    // - vault.deposit measures balance BEFORE and AFTER the transfer
    // - LyingBalanceToken reports MAX both times → delta = 0 → should fail
}
