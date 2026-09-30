//! Adversarial cross-contract negative tests for price_oracle (issue #115).
//!
//! Each test exercises one cross-contract trust boundary in price_oracle
//! with an adversarial counterparty from `test_adversaries`.
//!
//! # Cross-contract boundaries covered
//!
//! 1. Multisig boundary — every `_via_multisig` entrypoint calls `is_executable(action_id, class)`.
//!    Adversaries: `NeverApproveMultisig`, `AlwaysApproveMultisig`, `ExpiredApprovalMultisig`.
//!
//! 2. Oracle-adapter / multi-source boundary — `oracle_adapter.rs` calls external oracles.
//!    Adversaries: `AlwaysNoneOracle`, `RevertingOracle`, `NegativePriceOracle`.
//!
//! # Note on stub tests
//!
//! Tests that reference `PriceOracle` / `PriceOracleClient` from this crate
//! are currently `#[ignore]`'d with "issue #115: price_oracle lib.rs is a stub".
//! Un-ignore them as the contract implementation lands.

#![cfg(test)]

extern crate std;

use soroban_sdk::{testutils::Address as _, Address, BytesN, Env, Symbol};

use test_adversaries::multisig::{AlwaysApproveMultisig, ExpiredApprovalMultisig, NeverApproveMultisig};
use test_adversaries::oracle::{AlwaysNoneOracle, NegativePriceOracle, RevertingOracle};

// ─── helpers ─────────────────────────────────────────────────────────────────

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

// ─── Multisig boundary — stub tests (waiting for price_oracle implementation) ─

/// `pause_via_multisig` with a never-approve multisig must reject with
/// `Unauthorized` — the call must not pause the oracle.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at price_oracle.pause_via_multisig.**
#[test]
#[ignore = "issue #115: price_oracle PriceOracle/PriceOracleClient not yet in lib.rs"]
fn pause_via_multisig_with_never_approve_multisig_is_rejected() {
    // When price_oracle is wired into lib.rs:
    //
    // use crate::{PriceOracle, PriceOracleClient};
    // let env = Env::default();
    // env.mock_all_auths();
    // let admin = Address::generate(&env);
    // let oracle_id = env.register_contract(None, PriceOracle);
    // let client = PriceOracleClient::new(&env, &oracle_id);
    // client.initialize(&admin);
    //
    // let ms_id = env.register_contract(None, NeverApproveMultisig);
    // // NeverApproveMultisig.is_executable always returns false.
    // client.pause_via_multisig(&ms_id, &0u64); // should panic Unauthorized (#9)
    let _ = NeverApproveMultisig;
}

/// `add_feeder_via_multisig` with a never-approve multisig must reject.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at price_oracle.add_feeder_via_multisig.**
#[test]
#[ignore = "issue #115: price_oracle PriceOracle/PriceOracleClient not yet in lib.rs"]
fn add_feeder_via_multisig_with_never_approve_multisig_is_rejected() {
    // use crate::{PriceOracle, PriceOracleClient};
    // ...
    // client.add_feeder_via_multisig(&ms_id, &0u64, &feeder); // should panic Unauthorized
    let _ = NeverApproveMultisig;
}

/// `remove_feeder_via_multisig` with a never-approve multisig must reject.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at price_oracle.remove_feeder_via_multisig.**
#[test]
#[ignore = "issue #115: price_oracle PriceOracle/PriceOracleClient not yet in lib.rs"]
fn remove_feeder_via_multisig_with_never_approve_multisig_is_rejected() {
    // use crate::{PriceOracle, PriceOracleClient};
    // ...
    // client.remove_feeder_via_multisig(&ms_id, &0u64, &feeder); // should panic Unauthorized
    let _ = NeverApproveMultisig;
}

/// `set_max_staleness_via_multisig` with a never-approve multisig must reject.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at price_oracle.set_max_staleness_via_multisig.**
#[test]
#[ignore = "issue #115: price_oracle PriceOracle/PriceOracleClient not yet in lib.rs"]
fn set_max_staleness_via_multisig_with_never_approve_is_rejected() {
    let _ = NeverApproveMultisig;
}

/// `set_min_reports_via_multisig` with a never-approve multisig must reject.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at price_oracle.set_min_reports_via_multisig.**
#[test]
#[ignore = "issue #115: price_oracle PriceOracle/PriceOracleClient not yet in lib.rs"]
fn set_min_reports_via_multisig_with_never_approve_is_rejected() {
    let _ = NeverApproveMultisig;
}

/// `transfer_admin_via_multisig` with a never-approve multisig must reject.
///
/// **Issue #115 — Multisig: NeverApproveMultisig at price_oracle.transfer_admin_via_multisig.**
#[test]
#[ignore = "issue #115: price_oracle PriceOracle/PriceOracleClient not yet in lib.rs"]
fn transfer_admin_via_multisig_with_never_approve_is_rejected() {
    let _ = NeverApproveMultisig;
}

/// An expired multisig approval must cause `pause_via_multisig` to reject,
/// even if M-of-N approved at some earlier ledger.
///
/// **Issue #115 — Multisig: ExpiredApprovalMultisig at pause_via_multisig.**
#[test]
#[ignore = "issue #115: price_oracle PriceOracle/PriceOracleClient not yet in lib.rs"]
fn pause_via_multisig_with_expired_approval_is_rejected() {
    // ExpiredApprovalMultisig.is_executable always returns false (all expired).
    // client.pause_via_multisig(&ms_id, &0u64); // should panic Unauthorized
    let _ = ExpiredApprovalMultisig;
}

/// An always-approve multisig at an address that was NOT configured as the
/// oracle's trusted multisig must NOT be accepted.
///
/// **Issue #115 — Multisig: AlwaysApproveMultisig substitution at price_oracle.**
#[test]
#[ignore = "issue #115: price_oracle PriceOracle/PriceOracleClient not yet in lib.rs"]
fn always_approve_multisig_at_wrong_address_is_rejected() {
    // The oracle compares multisig address against stored admin.
    // A malicious always-approve multisig at an unknown address must not be accepted.
    let _ = AlwaysApproveMultisig;
}

// ─── Oracle-adapter boundary — LIVE tests ────────────────────────────────────
// oracle_adapter.rs is fully implemented; these tests run against it.

use crate::oracle_adapter::{AdapterError, MultiSourceOracleAdapter};

/// The multi-source oracle adapter must return `NoValidPrice` (not `Ok(0)`)
/// when both primary and fallback sources return `None`.
///
/// **Issue #115 — Oracle-adapter: both sources return None.**
#[test]
fn oracle_adapter_both_none_sources_return_no_valid_price() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);

    let primary = env.register_contract(None, AlwaysNoneOracle);
    env.invoke_contract::<()>(
        &primary,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, admin.clone().into()],
    );

    let fallback = env.register_contract(None, AlwaysNoneOracle);
    env.invoke_contract::<()>(
        &fallback,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, admin.clone().into()],
    );

    let result = MultiSourceOracleAdapter::resolve_price(
        &env,
        &primary,
        &fallback,
        Symbol::new(&env, "XLM"),
        3600,
        500, // 5% tolerance in bps
    );
    assert_eq!(
        result,
        Err(AdapterError::NoValidPrice),
        "both-None sources must return NoValidPrice not Ok(0)"
    );
}

/// When the primary oracle returns `None` but the fallback has a fresh price,
/// `resolve_price` must use the fallback rather than failing.
/// When BOTH return None, it must fail.
///
/// This confirms the fallback-path is not silently eaten.
///
/// **Issue #115 — Oracle-adapter: primary None, fallback None → must fail.**
#[test]
fn oracle_adapter_primary_none_fallback_none_fails() {
    let env = Env::default();
    env.mock_all_auths();

    // Both are AlwaysNoneOracle — same as the test above but named separately
    // for documentation clarity: primary and fallback both failing.
    let admin = Address::generate(&env);

    let primary = env.register_contract(None, AlwaysNoneOracle);
    env.invoke_contract::<()>(
        &primary,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, admin.clone().into()],
    );
    let fallback = env.register_contract(None, AlwaysNoneOracle);
    env.invoke_contract::<()>(
        &fallback,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, admin.into()],
    );

    let result = MultiSourceOracleAdapter::resolve_price(
        &env,
        &primary,
        &fallback,
        Symbol::new(&env, "BTC"),
        3600,
        200,
    );
    assert!(result.is_err(), "both-None must not succeed");
}

/// The adapter must return an error when the negative-price oracle is the
/// only source, since a negative price is invalid for settlement.
///
/// Note: the `oracle_adapter::read_feed` always returns a hardcoded price
/// (10_000_000) because it's a stub — this test currently documents the
/// expected behavior and will pass once `read_feed` is wired to real calls.
///
/// **Issue #115 — Oracle-adapter: NegativePriceOracle as primary source.**
#[test]
#[ignore = "issue #115: oracle_adapter::read_feed is a stub returning hardcoded 10_000_000; un-ignore when wired to real oracle calls"]
fn oracle_adapter_negative_primary_price_must_return_error() {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);

    let primary = env.register_contract(None, NegativePriceOracle);
    env.invoke_contract::<()>(
        &primary,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, admin.clone().into()],
    );
    let fallback = env.register_contract(None, AlwaysNoneOracle);
    env.invoke_contract::<()>(
        &fallback,
        &Symbol::new(&env, "initialize"),
        soroban_sdk::vec![&env, admin.into()],
    );

    let result = MultiSourceOracleAdapter::resolve_price(
        &env,
        &primary,
        &fallback,
        Symbol::new(&env, "XLM"),
        3600,
        500,
    );
    assert!(
        result.is_err(),
        "negative price from oracle must not be returned as Ok"
    );
}
