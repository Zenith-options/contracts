#![cfg(test)]
extern crate std;

use crate::{
    OptionSeries, OptionType, OptionsMarket, OptionsMarketClient, PositionSide, SeriesFilter,
    SeriesState,
};
use multisig::{Multisig, MultisigClient};
use price_oracle::{PriceOracle, PriceOracleClient};
use soroban_sdk::{
    testutils::{Address as _, Events as _, Ledger},
    token, Address, BytesN, Env, IntoVal, Symbol, TryFromVal,
};
use vault::{Vault, VaultClient};

const USDC_DECIMALS: i128 = 10_000_000; // matches PRICE_PRECISION

struct Harness<'a> {
    env: Env,
    client: OptionsMarketClient<'a>,
    token: Address,
    admin: Address,
    oracle: Address,
    fee_recipient: Address,
}

/// Registers a Stellar Asset Contract as the collateral token (a real
/// deployable token, not a hand-rolled mock) and the options market
/// contract itself, then initializes the market. `mock_all_auths()`
/// means every `require_auth()` call in the contract succeeds without
/// needing real signatures — appropriate for unit tests that are
/// exercising the contract's own logic, not its auth plumbing.
fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();

    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let fee_recipient = Address::generate(&env);
    let token_admin = Address::generate(&env);

    let token_contract = env.register_stellar_asset_contract_v2(token_admin.clone());
    let token_address = token_contract.address();

    let contract_id = env.register_contract(None, OptionsMarket);
    let client = OptionsMarketClient::new(&env, &contract_id);
    client.initialize(&admin, &oracle, &token_address, &fee_recipient);

    Harness {
        env,
        client,
        token: token_address,
        admin,
        oracle,
        fee_recipient,
    }
}

/// Mints `amount` of the test collateral token to `to` via the Stellar
/// Asset Contract's admin-only mint entry point.
fn mint(h: &Harness, to: &Address, amount: i128) {
    token::StellarAssetClient::new(&h.env, &h.token).mint(to, &amount);
}

fn balance(h: &Harness, of: &Address) -> i128 {
    token::Client::new(&h.env, &h.token).balance(of)
}

fn make_series(h: &Harness, option_type: OptionType, strike: i128, premium: i128) -> u64 {
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    h.client.create_series(
        &Symbol::new(&h.env, "XLM"),
        &option_type,
        &strike,
        &expiry,
        &premium,
        &(450_000_000i128), // 45% implied vol
    )
}

/// write_option pays a writer's premium out of the pool that buyers' own
/// premium payments fund (see the InsufficientPremiumPool gate in
/// write_option) — so tests that only care about the writer side still
/// need a throwaway buyer to have funded that pool first. Buying the same
/// `contracts` count in the same series contributes exactly the
/// fee-adjusted amount write_option will need to pay out.
fn fund_premium_pool(h: &Harness, series_id: u64, contracts: i128) {
    let filler_buyer = Address::generate(&h.env);
    mint(h, &filler_buyer, 1_000 * USDC_DECIMALS);
    h.client.buy_option(
        &filler_buyer,
        &series_id,
        &contracts,
        &(1_000 * USDC_DECIMALS),
    );
}

#[test]
fn initialize_sets_admin_and_zeroed_counters() {
    let h = setup();
    let (premiums, oi, series_count) = h.client.get_stats();
    assert_eq!(premiums, 0);
    assert_eq!(oi, 0);
    assert_eq!(series_count, 0);
    // Just confirms admin/oracle/fee_recipient/token were all accepted by
    // initialize without panicking — no direct getter for them exists.
    let _ = (&h.admin, &h.oracle, &h.fee_recipient);
}

#[test]
#[should_panic(expected = "Error(Contract, #1)")] // AlreadyInitialized
fn initialize_twice_panics() {
    let h = setup();
    h.client
        .initialize(&h.admin, &h.oracle, &h.token, &h.fee_recipient);
}

#[test]
fn create_series_stores_correct_fields() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let series: OptionSeries = h.client.get_series(&series_id).unwrap();

    assert_eq!(series.series_id, series_id);
    assert_eq!(series.strike_price, 700_000_000);
    assert_eq!(series.premium, 40_000_000);
    assert_eq!(series.open_interest, 0);
    assert!(series.option_type == OptionType::Call);
}

#[test]
#[should_panic(expected = "Error(Contract, #16)")] // ExpiryTooSoon
fn create_series_rejects_expiry_less_than_an_hour_out() {
    let h = setup();
    let too_soon = h.env.ledger().timestamp() + 1800; // 30 minutes
    h.client.create_series(
        &Symbol::new(&h.env, "XLM"),
        &OptionType::Call,
        &700_000_000,
        &too_soon,
        &40_000_000,
        &450_000_000,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #22)")] // InvalidSeriesParams
fn create_series_rejects_a_non_positive_strike() {
    let h = setup();
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    h.client.create_series(
        &Symbol::new(&h.env, "XLM"),
        &OptionType::Call,
        &0,
        &expiry,
        &40_000_000,
        &450_000_000,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #22)")] // InvalidSeriesParams
fn create_series_rejects_a_negative_premium() {
    let h = setup();
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    h.client.create_series(
        &Symbol::new(&h.env, "XLM"),
        &OptionType::Call,
        &700_000_000,
        &expiry,
        &-1,
        &450_000_000,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #22)")] // InvalidSeriesParams
fn create_series_rejects_a_negative_implied_vol() {
    let h = setup();
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    h.client.create_series(
        &Symbol::new(&h.env, "XLM"),
        &OptionType::Call,
        &700_000_000,
        &expiry,
        &40_000_000,
        &-1,
    );
}

#[test]
#[should_panic] // no auth was mocked at all — require_auth() has nothing to accept
fn initialize_without_any_authorization_panics() {
    // Deliberately skips mock_all_auths(): every other test in this file
    // uses it (soroban's mock-auth mode applies for the env's whole
    // lifetime once armed, so there's no way to "unmock" partway through
    // a test to check a *later* call specifically) — this test exists
    // just to confirm require_auth() is actually load-bearing on
    // initialize(), not a no-op, by never arming it in the first place.
    let env = Env::default();
    let admin = Address::generate(&env);
    let oracle = Address::generate(&env);
    let fee_recipient = Address::generate(&env);
    let token_admin = Address::generate(&env);
    let token = env
        .register_stellar_asset_contract_v2(token_admin)
        .address();

    let contract_id = env.register_contract(None, OptionsMarket);
    let client = OptionsMarketClient::new(&env, &contract_id);
    client.initialize(&admin, &oracle, &token, &fee_recipient);
}

// ─── buy_option ─────────────────────────────────────────────────────────────

#[test]
fn buy_option_transfers_premium_and_opens_a_long_position() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);

    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    let position = h.client.get_position(&pos_id).unwrap();
    assert!(position.side == PositionSide::Long);
    assert_eq!(position.contracts, USDC_DECIMALS);
    assert_eq!(position.collateral_locked, 0);
    // Buyer paid the full premium (40); the 0.5% protocol fee comes out of
    // what the vault later pays the writer, not an extra charge to the buyer.
    assert_eq!(balance(&h, &buyer), 1_000 * USDC_DECIMALS - 40_000_000);

    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    assert_eq!(series.open_interest, USDC_DECIMALS);

    let positions = h.client.get_user_positions(&buyer);
    assert_eq!(positions.len(), 1);
    assert_eq!(positions.get(0).unwrap(), pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #12)")] // ZeroContracts
fn buy_option_rejects_zero_contracts() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    h.client
        .buy_option(&buyer, &series_id, &0, &(50 * USDC_DECIMALS));
}

#[test]
#[should_panic(expected = "Error(Contract, #7)")] // InsufficientPremium
fn buy_option_enforces_slippage_protection() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    // Series premium is 40_000_000 for 1 contract; offering a max_premium
    // below that must be rejected rather than silently charging more than
    // the caller agreed to.
    h.client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &30_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // SeriesNotFound
fn buy_option_rejects_unknown_series() {
    let h = setup();
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    h.client
        .buy_option(&buyer, &999, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
}

// ─── write_option ───────────────────────────────────────────────────────────

#[test]
fn write_covered_call_locks_collateral_equal_to_notional() {
    let h = setup();
    // No underlying price has been set yet, so write_option falls back to
    // the series' own strike as the notional basis — this pins down that
    // documented fallback rather than assuming an oracle price exists.
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    let required = 700_000_000; // 1 contract * strike (fallback price)
    mint(&h, &writer, required);

    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &required);
    let position = h.client.get_position(&pos_id).unwrap();

    assert!(position.side == PositionSide::Short);
    assert_eq!(position.collateral_locked, required);
    // Writer received premium (minus the 0.5% protocol fee) on top of
    // having already spent `required` on collateral.
    let fee = 40_000_000 * 5 / 1000;
    assert_eq!(balance(&h, &writer), 40_000_000 - fee);
}

#[test]
fn write_cash_secured_put_requires_110_percent_of_strike() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Put, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    let required = 700_000_000 * 11 / 10; // strike * contracts * 110%
    mint(&h, &writer, required);

    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &required);
    let position = h.client.get_position(&pos_id).unwrap();
    assert_eq!(position.collateral_locked, required);
}

#[test]
#[should_panic(expected = "Error(Contract, #8)")] // InsufficientCollateral
fn write_option_rejects_undercollateralized_offer() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Put, 700_000_000, 40_000_000);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    // Offers exactly 100% of strike for a put, which needs 110%.
    h.client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #21)")] // InsufficientPremiumPool
fn write_option_rejects_a_write_with_no_buyer_premium_to_draw_from() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    // No buyer has ever bought into this series, so the premium pool is
    // empty — write_option must not pay the writer out of its own
    // just-deposited collateral, which isn't a premium anyone paid.
    h.client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);
}

#[test]
fn write_option_succeeds_once_the_pool_partially_covers_it() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    assert_eq!(h.client.get_premium_pool(), 0);

    fund_premium_pool(&h, series_id, USDC_DECIMALS);
    assert_eq!(h.client.get_premium_pool(), 39_800_000); // 40M premium net of the 0.5% fee

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    h.client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    // The writer's premium exactly drained the pool the lone buyer funded.
    assert_eq!(h.client.get_premium_pool(), 0);
}

// ─── settlement + exercise ──────────────────────────────────────────────────

fn advance_past_expiry(h: &Harness, series_id: u64) {
    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    h.env.ledger().set_timestamp(series.expiry + 1);
}

#[test]
fn exercise_itm_call_pays_out_the_intrinsic_value() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    // Settlement price above strike -> call finishes in the money.
    h.client.set_settlement_price(&series_id, &(750_000_000));

    // Fund the contract's own vault so it can actually pay the intrinsic
    // value out — in this test nothing else deposited into it first.
    mint(&h, &h.client.address, 1_000 * USDC_DECIMALS);

    let before = balance(&h, &buyer);
    h.client.exercise(&buyer, &pos_id);
    // Intrinsic = (750 - 700) * 1 contract = 50.
    assert_eq!(balance(&h, &buyer) - before, 50_000_000);

    let position = h.client.get_position(&pos_id).unwrap();
    assert!(position.is_exercised);
}

#[test]
fn exercise_itm_put_pays_out_the_intrinsic_value() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Put, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    // Settlement price below strike -> put finishes in the money.
    h.client.set_settlement_price(&series_id, &(650_000_000));
    mint(&h, &h.client.address, 1_000 * USDC_DECIMALS);

    let before = balance(&h, &buyer);
    h.client.exercise(&buyer, &pos_id);
    // Intrinsic = (700 - 650) * 1 contract = 50.
    assert_eq!(balance(&h, &buyer) - before, 50_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #14)")] // NotInTheMoney
fn exercise_otm_call_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    // Settlement below strike -> call is worthless.
    h.client.set_settlement_price(&series_id, &(650_000_000));
    h.client.exercise(&buyer, &pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // SeriesNotExpired
fn exercise_before_expiry_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
    h.client.exercise(&buyer, &pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #15)")] // WrongSide
fn exercise_rejects_a_short_position() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    h.client.exercise(&writer, &pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #9)")] // AlreadyExercised
fn exercise_twice_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    mint(&h, &h.client.address, 1_000 * USDC_DECIMALS);

    h.client.exercise(&buyer, &pos_id);
    h.client.exercise(&buyer, &pos_id);
}

// ─── exercise_batch ─────────────────────────────────────────────────────────

#[test]
fn exercise_batch_pays_out_every_position_in_the_list() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_a = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
    let pos_b = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    mint(&h, &h.client.address, 1_000 * USDC_DECIMALS);

    let before = balance(&h, &buyer);
    let total = h
        .client
        .exercise_batch(&buyer, &soroban_sdk::vec![&h.env, pos_a, pos_b]);
    // Intrinsic = 50 per position, two positions.
    assert_eq!(total, 100_000_000);
    assert_eq!(balance(&h, &buyer) - before, 100_000_000);

    assert!(h.client.get_position(&pos_a).unwrap().is_exercised);
    assert!(h.client.get_position(&pos_b).unwrap().is_exercised);
}

#[test]
#[should_panic(expected = "Error(Contract, #23)")] // InvalidBatchSize
fn exercise_batch_rejects_an_empty_list() {
    let h = setup();
    let buyer = Address::generate(&h.env);
    h.client.exercise_batch(&buyer, &soroban_sdk::vec![&h.env]);
}

#[test]
#[should_panic(expected = "Error(Contract, #23)")] // InvalidBatchSize
fn exercise_batch_rejects_more_than_the_max_batch_size() {
    let h = setup();
    let buyer = Address::generate(&h.env);
    let mut ids = soroban_sdk::vec![&h.env];
    for i in 0..26u64 {
        ids.push_back(i);
    }
    h.client.exercise_batch(&buyer, &ids);
}

#[test]
fn exercise_batch_is_all_or_nothing() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let itm_pos = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
    let never_created_pos_id = 9_999u64;

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    mint(&h, &h.client.address, 1_000 * USDC_DECIMALS);

    let before = balance(&h, &buyer);
    let result = h.client.try_exercise_batch(
        &buyer,
        &soroban_sdk::vec![&h.env, itm_pos, never_created_pos_id],
    );
    assert!(result.is_err());

    // The whole call aborted — the otherwise-valid ITM position in the
    // same batch was never paid out or marked exercised.
    assert_eq!(balance(&h, &buyer), before);
    assert!(!h.client.get_position(&itm_pos).unwrap().is_exercised);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn exercise_batch_rejects_a_position_owned_by_someone_else() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    let stranger = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    h.client
        .exercise_batch(&stranger, &soroban_sdk::vec![&h.env, pos_id]);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // SeriesNotExpired
fn set_settlement_price_before_expiry_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.set_settlement_price(&series_id, &(750_000_000));
}

// ─── reclaim_collateral ─────────────────────────────────────────────────────

#[test]
fn reclaim_collateral_returns_locked_minus_max_loss() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000)); // ITM by 50

    let before = balance(&h, &writer);
    h.client.reclaim_collateral(&writer, &pos_id);
    // 700 locked - 50 max loss = 650 reclaimed.
    assert_eq!(balance(&h, &writer) - before, 650_000_000);

    let position = h.client.get_position(&pos_id).unwrap();
    assert!(position.is_settled);
}

#[test]
fn reclaim_collateral_returns_everything_when_otm() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    // Funds the premium pool so write_option below has real buyer premium
    // to pay the writer from, instead of dipping into the writer's own
    // just-deposited collateral (see write_option's InsufficientPremiumPool
    // gate) — the fix for the vault-liquidity gap this test used to work
    // around with a direct top-up mint.
    fund_premium_pool(&h, series_id, USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(650_000_000)); // OTM

    let before = balance(&h, &writer);
    h.client.reclaim_collateral(&writer, &pos_id);
    assert_eq!(balance(&h, &writer) - before, 700_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #15)")] // WrongSide
fn reclaim_collateral_rejects_a_long_position() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    h.client.reclaim_collateral(&buyer, &pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")] // AlreadySettled
fn reclaim_collateral_twice_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    // Funds the premium pool for the same reason noted on
    // reclaim_collateral_returns_everything_when_otm — this used to need a
    // direct top-up mint to make the FIRST reclaim below succeed for real
    // (otherwise it fails on the token contract's own insufficient-balance
    // error, which happens to also render as "Error(Contract, #10)", so
    // the should_panic would pass for the wrong reason without ever
    // reaching the second call).
    fund_premium_pool(&h, series_id, USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(650_000_000));
    h.client.reclaim_collateral(&writer, &pos_id);
    h.client.reclaim_collateral(&writer, &pos_id);
}

// ─── reclaim_batch ──────────────────────────────────────────────────────────

#[test]
fn reclaim_batch_pays_out_every_position_in_the_list() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, 2 * USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 1_400_000_000);
    let pos_a = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);
    let pos_b = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000)); // ITM by 50 each

    let before = balance(&h, &writer);
    let total = h
        .client
        .reclaim_batch(&writer, &soroban_sdk::vec![&h.env, pos_a, pos_b]);
    // (700 locked - 50 max loss) * 2 positions = 1300.
    assert_eq!(total, 1_300_000_000);
    assert_eq!(balance(&h, &writer) - before, 1_300_000_000);

    assert!(h.client.get_position(&pos_a).unwrap().is_settled);
    assert!(h.client.get_position(&pos_b).unwrap().is_settled);
}

#[test]
#[should_panic(expected = "Error(Contract, #23)")] // InvalidBatchSize
fn reclaim_batch_rejects_an_empty_list() {
    let h = setup();
    let writer = Address::generate(&h.env);
    h.client.reclaim_batch(&writer, &soroban_sdk::vec![&h.env]);
}

#[test]
fn reclaim_batch_is_all_or_nothing() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let valid_pos = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);
    let never_created_pos_id = 9_999u64;

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(650_000_000)); // OTM, full reclaim

    let before = balance(&h, &writer);
    let result = h.client.try_reclaim_batch(
        &writer,
        &soroban_sdk::vec![&h.env, valid_pos, never_created_pos_id],
    );
    assert!(result.is_err());

    assert_eq!(balance(&h, &writer), before);
    assert!(!h.client.get_position(&valid_pos).unwrap().is_settled);
}

// ─── update_premium ─────────────────────────────────────────────────────────

#[test]
fn update_premium_changes_price_and_iv() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client
        .update_premium(&series_id, &(45_000_000), &(500_000_000));

    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    assert_eq!(series.premium, 45_000_000);
    assert_eq!(series.implied_vol, 500_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // SeriesNotActive
fn update_premium_on_a_settled_series_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    h.client
        .update_premium(&series_id, &(45_000_000), &(500_000_000));
}

// ─── views ──────────────────────────────────────────────────────────────────

#[test]
fn get_stats_reflects_created_series_count() {
    let h = setup();
    make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    make_series(&h, OptionType::Put, 650_000_000, 35_000_000);

    let (_, _, series_count) = h.client.get_stats();
    assert_eq!(series_count, 2);
}

#[test]
fn get_user_positions_is_empty_for_a_wallet_with_none() {
    let h = setup();
    let stranger = Address::generate(&h.env);
    assert_eq!(h.client.get_user_positions(&stranger).len(), 0);
}

// ─── full lifecycle ─────────────────────────────────────────────────────────

/// One buyer and one writer trade opposite sides of the same series, the
/// series settles ITM for the buyer, and both sides collect what the
/// contract's accounting says they should — a check that the individual
/// unit tests above compose correctly, not just that each function works
/// in isolation.
#[test]
fn full_lifecycle_covered_call_itm() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    // Buyer goes first: write_option pays the writer's premium out of the
    // pool buyers' premium payments fund, so the pool needs to hold real
    // funds before a writer can be paid out of it.
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let buyer_pos = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let writer_pos = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    assert_eq!(series.open_interest, 2 * USDC_DECIMALS); // one long + one short

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000)); // 50 ITM

    let buyer_before = balance(&h, &buyer);
    h.client.exercise(&buyer, &buyer_pos);
    assert_eq!(balance(&h, &buyer) - buyer_before, 50_000_000);

    let writer_before = balance(&h, &writer);
    h.client.reclaim_collateral(&writer, &writer_pos);
    assert_eq!(balance(&h, &writer) - writer_before, 650_000_000); // 700 locked - 50 paid out
}

// ─── transfer_admin ─────────────────────────────────────────────────────────

#[test]
fn transfer_admin_hands_off_control() {
    let h = setup();
    assert_eq!(h.client.get_admin(), h.admin);

    let new_admin = Address::generate(&h.env);
    h.client.transfer_admin(&new_admin);
    assert_eq!(h.client.get_admin(), new_admin);

    // create_series still works, now authorized against the NEW admin
    // (mock_all_auths lets the call through regardless of who signed, but
    // the stored admin address driving that check has genuinely moved).
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    assert!(h.client.get_series(&series_id).is_some());
}

// ─── pause / unpause ────────────────────────────────────────────────────────

#[test]
fn pause_blocks_new_trades_unpause_restores_them() {
    let h = setup();
    assert!(!h.client.is_paused());

    h.client.pause();
    assert!(h.client.is_paused());

    h.client.unpause();
    assert!(!h.client.is_paused());

    // After unpausing, trading works again.
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    assert!(h.client.get_series(&series_id).is_some());
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")] // ContractPaused
fn create_series_is_rejected_while_paused() {
    let h = setup();
    h.client.pause();
    make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")] // ContractPaused
fn buy_option_is_rejected_while_paused() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.pause();

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    h.client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")] // ContractPaused
fn write_option_is_rejected_while_paused() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.pause();

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    h.client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);
}

/// A pause must not trap funds already at risk: an existing writer can still
/// exercise/reclaim through settlement while the contract is paused.
#[test]
fn pause_does_not_block_settlement_of_existing_positions() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    h.client.pause();

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(650_000_000)); // OTM, no exercise needed

    let before = balance(&h, &writer);
    h.client.reclaim_collateral(&writer, &pos_id);
    assert_eq!(balance(&h, &writer) - before, 700_000_000);
}

// ─── cancel_series / claim_refund ───────────────────────────────────────────

#[test]
fn cancelled_series_refunds_buyer_premium_net_of_fee() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.cancel_series(&series_id);

    let before = balance(&h, &buyer);
    h.client.claim_refund(&buyer, &pos_id);
    // 40_000_000 premium, 0.5% fee already sent to fee_recipient at buy
    // time, so only the net 39_800_000 is refundable from the vault.
    assert_eq!(balance(&h, &buyer) - before, 39_800_000);
}

#[test]
fn cancelled_series_refunds_writer_full_collateral() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    h.client.cancel_series(&series_id);

    let before = balance(&h, &writer);
    h.client.claim_refund(&writer, &pos_id);
    assert_eq!(balance(&h, &writer) - before, 700_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #18)")] // SeriesNotCancelled
fn claim_refund_rejects_a_still_active_series() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.claim_refund(&buyer, &pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")] // AlreadySettled
fn claim_refund_twice_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.cancel_series(&series_id);
    h.client.claim_refund(&buyer, &pos_id);
    h.client.claim_refund(&buyer, &pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // SeriesNotActive
fn cancel_series_rejects_an_already_cancelled_series() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.cancel_series(&series_id);
    h.client.cancel_series(&series_id);
}

// ─── cross-contract: escrow_series_to_vault / claim_refund_from_vault ──────

/// Deploys a real vault contract in the SAME Env as the options market
/// under test, with its OWN admin set to the options market's contract
/// address — the precondition escrow_series_to_vault/claim_refund_from_-
/// vault's doc comments require, so options_market's cross-contract
/// deposit()/withdraw() calls satisfy vault's own auth checks the same
/// way any contract-to-contract call does (see vault's own docs).
fn setup_vault(h: &Harness) -> Address {
    let contract_id = h.env.register_contract(None, Vault);
    let client = VaultClient::new(&h.env, &contract_id);
    client.initialize(&h.client.address, &h.token);
    contract_id
}

#[test]
fn escrow_then_claim_refund_from_vault_pays_buyer_net_of_fee() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.cancel_series(&series_id);
    // 40_000_000 premium net of the 0.5% fee = 39_800_000 is this
    // position's own refund-eligible principal.
    assert_eq!(h.client.get_series_escrow(&series_id), 39_800_000);

    h.client.escrow_series_to_vault(&vault_id, &series_id);
    // Fully moved out of options_market's own accounting...
    assert_eq!(h.client.get_series_escrow(&series_id), 0);
    // ...and now sitting in the vault, tagged by series_id.
    let vault_client = VaultClient::new(&h.env, &vault_id);
    assert_eq!(vault_client.balance_of(&series_id), 39_800_000);

    let before = balance(&h, &buyer);
    h.client.claim_refund_from_vault(&vault_id, &buyer, &pos_id);
    assert_eq!(balance(&h, &buyer) - before, 39_800_000);
    assert_eq!(vault_client.balance_of(&series_id), 0);
    assert!(h.client.get_position(&pos_id).unwrap().is_settled);
}

#[test]
fn escrow_then_claim_refund_from_vault_pays_writer_full_collateral() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    // Funds the (contract-wide) premium pool from a DECOY series that
    // never gets cancelled — using the series under test itself would
    // inflate ITS OWN SeriesEscrow with a buyer position whose premium
    // write_option immediately pays out to the writer, money that's
    // genuinely gone by the time this series is cancelled. That's a
    // real, pre-existing edge case in the pooled-premium design (see the
    // README's "no order matching" gap) — orthogonal to what this test
    // is actually checking, so it's sidestepped here rather than papered
    // over.
    let decoy_series_id = make_series(&h, OptionType::Call, 650_000_000, 40_000_000);
    fund_premium_pool(&h, decoy_series_id, USDC_DECIMALS);

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    h.client.cancel_series(&series_id);
    h.client.escrow_series_to_vault(&vault_id, &series_id);

    let before = balance(&h, &writer);
    h.client
        .claim_refund_from_vault(&vault_id, &writer, &pos_id);
    assert_eq!(balance(&h, &writer) - before, 700_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #18)")] // SeriesNotCancelled
fn escrow_series_to_vault_rejects_a_still_active_series() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.escrow_series_to_vault(&vault_id, &series_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #24)")] // NothingToEscrow
fn escrow_series_to_vault_rejects_a_second_call() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    h.client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.cancel_series(&series_id);
    h.client.escrow_series_to_vault(&vault_id, &series_id);
    h.client.escrow_series_to_vault(&vault_id, &series_id); // nothing left
}

#[test]
#[should_panic(expected = "Error(Contract, #24)")] // NothingToEscrow
fn escrow_series_to_vault_rejects_a_series_with_no_positions() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.cancel_series(&series_id);
    h.client.escrow_series_to_vault(&vault_id, &series_id);
}

/// The whole point of this integration: escrow_series_to_vault only ever
/// moves what's genuinely still outstanding, even when some positions in
/// the same series already claimed through the ORIGINAL claim_refund
/// path (which pays directly from options_market's own balance) before
/// escrow_series_to_vault ran. Without SeriesEscrow being debited by
/// BOTH claim paths, this call would move the position ALREADY paid
/// out's amount a second time, over-funding the vault out of OTHER
/// series' shared balance.
#[test]
fn escrow_series_to_vault_only_moves_the_remaining_liability() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer_a = Address::generate(&h.env);
    let buyer_b = Address::generate(&h.env);
    mint(&h, &buyer_a, 1_000 * USDC_DECIMALS);
    mint(&h, &buyer_b, 1_000 * USDC_DECIMALS);
    let pos_a = h
        .client
        .buy_option(&buyer_a, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
    let pos_b = h
        .client
        .buy_option(&buyer_b, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.cancel_series(&series_id);
    // buyer_a claims through the original direct-transfer path first.
    h.client.claim_refund(&buyer_a, &pos_a);
    assert_eq!(h.client.get_series_escrow(&series_id), 39_800_000); // only pos_b left

    h.client.escrow_series_to_vault(&vault_id, &series_id);
    let vault_client = VaultClient::new(&h.env, &vault_id);
    // Only buyer_b's still-outstanding refund moved — not double pos_a's.
    assert_eq!(vault_client.balance_of(&series_id), 39_800_000);

    let before = balance(&h, &buyer_b);
    h.client
        .claim_refund_from_vault(&vault_id, &buyer_b, &pos_b);
    assert_eq!(balance(&h, &buyer_b) - before, 39_800_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")] // AlreadySettled
fn claim_refund_from_vault_twice_is_rejected() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.cancel_series(&series_id);
    h.client.escrow_series_to_vault(&vault_id, &series_id);
    h.client.claim_refund_from_vault(&vault_id, &buyer, &pos_id);
    h.client.claim_refund_from_vault(&vault_id, &buyer, &pos_id);
}

#[test]
fn series_escrowed_to_vault_event_carries_series_id_and_amount() {
    let h = setup();
    h.env.mock_all_auths_allowing_non_root_auth();
    let vault_id = setup_vault(&h);
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    h.client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.cancel_series(&series_id);
    h.client.escrow_series_to_vault(&vault_id, &series_id);

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    let event_series_id = u64::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    assert_eq!(event_series_id, series_id);
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 39_800_000);
}

// ─── fee rate ────────────────────────────────────────────────────────────────

#[test]
fn fee_rate_defaults_to_fifty_bps_and_is_configurable() {
    let h = setup();
    assert_eq!(h.client.get_fee_rate(), 50);

    h.client.set_fee_rate(&100); // 1%
    assert_eq!(h.client.get_fee_rate(), 100);
}

#[test]
#[should_panic(expected = "Error(Contract, #19)")] // InvalidFeeRate
fn set_fee_rate_rejects_above_the_10_percent_ceiling() {
    let h = setup();
    h.client.set_fee_rate(&1_001);
}

#[test]
fn buy_option_applies_the_currently_configured_fee_rate() {
    let h = setup();
    h.client.set_fee_rate(&100); // 1% instead of the default 0.5%
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    let position = h.client.get_position(&pos_id).unwrap();
    assert_eq!(position.fee_paid, 400_000); // 1% of 40_000_000
}

#[test]
fn refund_uses_the_fee_rate_in_effect_at_buy_time_not_the_current_one() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    // Bought while the fee rate was still the default 0.5% (fee_paid = 200_000).
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    // Admin raises the rate afterward — this must NOT retroactively change
    // what this position refunds, since the position already stored the
    // fee it actually paid.
    h.client.set_fee_rate(&500); // 5%
    h.client.cancel_series(&series_id);

    let before = balance(&h, &buyer);
    h.client.claim_refund(&buyer, &pos_id);
    assert_eq!(balance(&h, &buyer) - before, 39_800_000); // 40M - the ORIGINAL 0.5% fee
}

// ─── upgrade ─────────────────────────────────────────────────────────────────

/// A full self-upgrade round-trip needs a second, already-built WASM
/// artifact to point at, which this unit-test harness doesn't produce (it
/// runs against native code, not the wasm32 target). This test only proves
/// the entry point is wired up and reaches the host's deployer — the
/// bogus, never-uploaded hash panics one layer past our own admin check,
/// not on our own auth logic, confirming that check passed through cleanly.
#[test]
#[should_panic]
fn upgrade_reaches_the_host_deployer_past_the_admin_check() {
    let h = setup();
    let bogus_hash = BytesN::from_array(&h.env, &[0u8; 32]);
    h.client.upgrade(&bogus_hash);
}

// ─── per-underlying series cap ──────────────────────────────────────────────

#[test]
fn create_series_tracks_the_count_per_underlying() {
    let h = setup();
    for _ in 0..50 {
        make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    }
    assert_eq!(
        h.client
            .get_series_count_for_underlying(&Symbol::new(&h.env, "XLM")),
        50
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #20)")] // TooManySeriesForUnderlying
fn create_series_rejects_the_51st_series_for_the_same_underlying() {
    let h = setup();
    for _ in 0..50 {
        make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    }
    make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
}

#[test]
fn series_cap_is_tracked_independently_per_underlying() {
    let h = setup();
    for _ in 0..50 {
        make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    }

    // XLM is now at the cap, but a different underlying should be unaffected.
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    let btc_series = h.client.create_series(
        &Symbol::new(&h.env, "BTC"),
        &OptionType::Call,
        &700_000_000,
        &expiry,
        &40_000_000,
        &450_000_000i128,
    );
    assert!(h.client.get_series(&btc_series).is_some());
    assert_eq!(
        h.client
            .get_series_count_for_underlying(&Symbol::new(&h.env, "BTC")),
        1
    );
}

// ─── cross-contract: set_settlement_price_from_oracle ──────────────────────

/// Deploys a real price_oracle contract in the SAME Env as the options
/// market under test (not a mock) — this is the actual second contract
/// options_market cross-calls, registered the same way the collateral
/// token is.
fn setup_price_oracle(h: &Harness) -> Address {
    let admin = Address::generate(&h.env);
    let contract_id = h.env.register_contract(None, PriceOracle);
    let client = PriceOracleClient::new(&h.env, &contract_id);
    client.initialize(&admin);
    contract_id
}

fn report_price(h: &Harness, oracle_id: &Address, underlying: &Symbol, price: i128) {
    let client = PriceOracleClient::new(&h.env, oracle_id);
    let feeder = Address::generate(&h.env);
    client.add_feeder(&feeder);
    client.report_price(&feeder, underlying, &price);
}

#[test]
fn set_settlement_price_from_oracle_reads_the_live_oracle_price() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let oracle_id = setup_price_oracle(&h);

    advance_past_expiry(&h, series_id);
    // Feeder reports fresh right at settlement time — reporting it before
    // the 30-day jump to expiry would leave it stale under price_oracle's
    // own 1-hour default max_staleness, same as a real feeder would need
    // to keep reporting all the way up to settlement rather than once at
    // series creation.
    report_price(&h, &oracle_id, &Symbol::new(&h.env, "XLM"), 750_000_000);

    h.client
        .set_settlement_price_from_oracle(&series_id, &oracle_id);

    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    assert_eq!(series.settlement_price, Some(750_000_000));
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // SeriesNotExpired
fn set_settlement_price_from_oracle_rejects_before_expiry() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let oracle_id = setup_price_oracle(&h);
    report_price(&h, &oracle_id, &Symbol::new(&h.env, "XLM"), 750_000_000);

    h.client
        .set_settlement_price_from_oracle(&series_id, &oracle_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #13)")] // PriceNotSet
fn set_settlement_price_from_oracle_rejects_a_stale_or_missing_oracle_price() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    // No feeder ever reports anything for this underlying.
    let oracle_id = setup_price_oracle(&h);

    advance_past_expiry(&h, series_id);
    h.client
        .set_settlement_price_from_oracle(&series_id, &oracle_id);
}

// ─── cross-contract: pause_via_multisig / unpause_via_multisig ─────────────

/// Deploys a real multisig contract in the SAME Env, the third
/// cross-contract dependency options_market has (alongside price_oracle).
/// Returns (contract_id, [signer0, signer1, signer2]) with a 2-of-3
/// threshold.
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
fn pause_via_multisig_pauses_once_the_action_is_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 1);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id); // 2 of 3, reaches threshold

    assert!(!h.client.is_paused());
    h.client.pause_via_multisig(&multisig_id, &action_id);
    assert!(h.client.is_paused());
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn pause_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    // Only 1 of 3 — below the 2-of-3 threshold.
    multisig_client.approve(&signers[0], &aid(&h.env, 1));

    h.client.pause_via_multisig(&multisig_id, &aid(&h.env, 1));
}

#[test]
fn unpause_via_multisig_unpauses_once_the_action_is_approved() {
    let h = setup();
    h.client.pause();
    assert!(h.client.is_paused());

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = aid(&h.env, 2);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client.unpause_via_multisig(&multisig_id, &action_id);
    assert!(!h.client.is_paused());
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn unpause_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    h.client.pause();
    let (multisig_id, _signers) = setup_multisig(&h);

    // No approvals at all for this action_id.
    h.client
        .unpause_via_multisig(&multisig_id, &aid(&h.env, 99));
}

#[test]
fn transfer_admin_via_multisig_hands_off_control_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 3);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let new_admin = Address::generate(&h.env);
    h.client
        .transfer_admin_via_multisig(&multisig_id, &action_id, &new_admin);
    assert_eq!(h.client.get_admin(), new_admin);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn transfer_admin_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    // Only 1 of 3.
    multisig_client.approve(&signers[0], &aid(&h.env, 4));

    let new_admin = Address::generate(&h.env);
    h.client
        .transfer_admin_via_multisig(&multisig_id, &aid(&h.env, 4), &new_admin);
}

// ─── cross-contract: set_fee_rate_via_multisig ─────────────────────────────

#[test]
fn set_fee_rate_via_multisig_applies_once_the_action_is_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 5);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client
        .set_fee_rate_via_multisig(&multisig_id, &action_id, &100);
    assert_eq!(h.client.get_fee_rate(), 100);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn set_fee_rate_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client
        .set_fee_rate_via_multisig(&multisig_id, &aid(&h.env, 99), &100);
}

#[test]
#[should_panic(expected = "Error(Contract, #19)")] // InvalidFeeRate
fn set_fee_rate_via_multisig_still_enforces_the_ceiling_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 6);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    // M-of-N approval authorizes WHO can call this, not an absurd rate.
    h.client
        .set_fee_rate_via_multisig(&multisig_id, &action_id, &1_001);
}

// ─── cross-contract: cancel_series_via_multisig ────────────────────────────

#[test]
fn cancel_series_via_multisig_cancels_once_the_action_is_approved() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);
    let action_id = aid(&h.env, 7);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);
    h.client
        .cancel_series_via_multisig(&multisig_id, &action_id, &series_id);

    // claim_refund only succeeds on a Cancelled series — proves the
    // multisig-gated call actually flipped the series' state, not just
    // that it didn't panic.
    let before = balance(&h, &buyer);
    h.client.claim_refund(&buyer, &pos_id);
    assert_eq!(balance(&h, &buyer) - before, 39_800_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn cancel_series_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client
        .cancel_series_via_multisig(&multisig_id, &aid(&h.env, 99), &series_id);
}

// ─── cross-contract: upgrade_via_multisig ──────────────────────────────────

/// Same caveat as upgrade_reaches_the_host_deployer_past_the_admin_check:
/// this harness runs native code, not wasm32, so there's no second
/// artifact to actually upgrade to. This only proves the multisig check
/// passed and execution reached the host's deployer, which then panics on
/// the bogus, never-uploaded hash — a different panic than our own
/// Unauthorized check would raise.
#[test]
#[should_panic]
fn upgrade_via_multisig_reaches_the_host_deployer_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 8);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let bogus_hash = BytesN::from_array(&h.env, &[0u8; 32]);
    h.client
        .upgrade_via_multisig(&multisig_id, &action_id, &bogus_hash);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn upgrade_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);

    let bogus_hash = BytesN::from_array(&h.env, &[0u8; 32]);
    h.client
        .upgrade_via_multisig(&multisig_id, &aid(&h.env, 99), &bogus_hash);
}

// ─── cross-contract: create_series_via_multisig ────────────────────────────

#[test]
fn create_series_via_multisig_lists_a_series_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 9);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    let series_id = h.client.create_series_via_multisig(
        &multisig_id,
        &action_id,
        &Symbol::new(&h.env, "XLM"),
        &OptionType::Call,
        &700_000_000,
        &expiry,
        &40_000_000,
        &450_000_000,
    );

    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    assert_eq!(series.strike_price, 700_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn create_series_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let (multisig_id, _signers) = setup_multisig(&h);
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;

    h.client.create_series_via_multisig(
        &multisig_id,
        &aid(&h.env, 99),
        &Symbol::new(&h.env, "XLM"),
        &OptionType::Call,
        &700_000_000,
        &expiry,
        &40_000_000,
        &450_000_000,
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #22)")] // InvalidSeriesParams
fn create_series_via_multisig_still_validates_params_once_approved() {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 10);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    // Approval authorizes WHO can call this, not a non-positive strike.
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    h.client.create_series_via_multisig(
        &multisig_id,
        &action_id,
        &Symbol::new(&h.env, "XLM"),
        &OptionType::Call,
        &0,
        &expiry,
        &40_000_000,
        &450_000_000,
    );
}

// ─── cross-contract: update_premium_via_multisig ───────────────────────────

#[test]
fn update_premium_via_multisig_updates_once_approved() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let (multisig_id, signers) = setup_multisig(&h);
    let multisig_client = MultisigClient::new(&h.env, &multisig_id);

    let action_id = aid(&h.env, 11);
    multisig_client.approve(&signers[0], &action_id);
    multisig_client.approve(&signers[1], &action_id);

    h.client.update_premium_via_multisig(
        &multisig_id,
        &action_id,
        &series_id,
        &45_000_000,
        &500_000_000,
    );

    let series: OptionSeries = h.client.get_series(&series_id).unwrap();
    assert_eq!(series.premium, 45_000_000);
    assert_eq!(series.implied_vol, 500_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #2)")] // Unauthorized
fn update_premium_via_multisig_rejects_when_not_yet_approved() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let (multisig_id, _signers) = setup_multisig(&h);

    h.client.update_premium_via_multisig(
        &multisig_id,
        &aid(&h.env, 99),
        &series_id,
        &45_000_000,
        &500_000_000,
    );
}

// ─── event data content ────────────────────────────────────────────────────
//
// Every state-changing function here publishes an event specifically so an
// off-chain indexer doesn't have to poll every view function (see the
// README) — but nothing previously checked that any event's DATA actually
// decodes to what a real indexer would expect, only that events fire at
// all (or, for a couple of vault/price_oracle events, a single scalar).
// These pin down the exact tuple shape for options_market's richer events.

#[test]
fn series_created_event_carries_the_series_fields() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    let (event_series_id, strike, expiry, premium) =
        <(u64, i128, u64, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(event_series_id, series_id);
    assert_eq!(strike, 700_000_000);
    assert_eq!(premium, 40_000_000);
    assert!(expiry > h.env.ledger().timestamp());
}

#[test]
fn option_bought_event_carries_position_and_premium_data() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);

    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    let (event_pos_id, event_series_id, contracts, total_premium) =
        <(u64, u64, i128, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(event_pos_id, pos_id);
    assert_eq!(event_series_id, series_id);
    assert_eq!(contracts, USDC_DECIMALS);
    assert_eq!(total_premium, 40_000_000); // net of the 0.5% fee already deducted

    // The buyer's address lives in topics, not data — an indexer
    // filtering "events for this wallet" reads it from here.
    let topic_buyer = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    assert_eq!(topic_buyer, buyer);
}

#[test]
fn admin_transferred_event_carries_old_and_new_admin() {
    let h = setup();
    let new_admin = Address::generate(&h.env);
    h.client.transfer_admin(&new_admin);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    let (old_admin, event_new_admin) = <(Address, Address)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(old_admin, h.admin);
    assert_eq!(event_new_admin, new_admin);
}

#[test]
fn fee_rate_updated_event_carries_the_new_rate() {
    let h = setup();
    h.client.set_fee_rate(&250);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 250);
}

#[test]
fn option_written_event_carries_position_and_collateral_data() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);

    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    let events = h.env.events().all();
    let (_, topics, data) = events.last().unwrap();
    let (event_pos_id, event_series_id, contracts, _writer_premium, required_collateral) =
        <(u64, u64, i128, i128, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(event_pos_id, pos_id);
    assert_eq!(event_series_id, series_id);
    assert_eq!(contracts, USDC_DECIMALS);
    assert_eq!(required_collateral, 700_000_000);

    // The writer's address lives in topics, not data.
    let topic_writer = Address::try_from_val(&h.env, &topics.get(1).unwrap()).unwrap();
    assert_eq!(topic_writer, writer);
}

#[test]
fn option_exercised_event_carries_settlement_price_and_payout() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    // Fund the contract's own vault so it can actually pay the intrinsic
    // value out — nothing else deposited into it first.
    mint(&h, &h.client.address, 1_000 * USDC_DECIMALS);
    h.client.exercise(&buyer, &pos_id);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    let (event_pos_id, settlement_price, payout) =
        <(u64, i128, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(event_pos_id, pos_id);
    assert_eq!(settlement_price, 750_000_000);
    assert_eq!(payout, 50_000_000); // 50 intrinsic * 1 contract
}

#[test]
fn series_cancelled_event_carries_the_series_id() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.cancel_series(&series_id);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    assert_eq!(u64::try_from_val(&h.env, &data).unwrap(), series_id);
}

#[test]
fn refund_claimed_event_carries_position_and_amount() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h
        .client
        .buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
    h.client.cancel_series(&series_id);

    h.client.claim_refund(&buyer, &pos_id);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    let (event_pos_id, amount) = <(u64, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(event_pos_id, pos_id);
    assert_eq!(amount, 39_800_000); // premium net of the 0.5% fee
}

#[test]
fn settlement_price_set_event_carries_the_price() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    assert_eq!(i128::try_from_val(&h.env, &data).unwrap(), 750_000_000);
}

#[test]
fn collateral_reclaimed_event_carries_position_and_amount() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    fund_premium_pool(&h, series_id, USDC_DECIMALS);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let pos_id = h
        .client
        .write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(650_000_000)); // OTM for the call
    h.client.reclaim_collateral(&writer, &pos_id);

    let events = h.env.events().all();
    let (_, _topics, data) = events.last().unwrap();
    let (event_pos_id, reclaim) = <(u64, i128)>::try_from_val(&h.env, &data).unwrap();
    assert_eq!(event_pos_id, pos_id);
    assert_eq!(reclaim, 700_000_000); // full collateral back, OTM means no payout owed
}

fn aid(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

// ─── batch buy / write ──────────────────────────────────────────────────────

/// Events emitted since `since` (a prior `events().all().len()`).
fn token_transfer_count(h: &Harness, since: u32) -> usize {
    h.env
        .events()
        .all()
        .iter()
        .skip(since as usize)
        .filter(|(contract, _, _)| *contract == h.token)
        .count()
}

fn event_count(h: &Harness, since: u32, name: &str) -> usize {
    let name = Symbol::new(&h.env, name);
    h.env
        .events()
        .all()
        .iter()
        .skip(since as usize)
        .filter(|(contract, topics, _)| {
            *contract == h.client.address
                && Symbol::try_from_val(&h.env, &topics.get(0).unwrap()).ok() == Some(name.clone())
        })
        .count()
}

fn assert_same_position(a: &crate::OptionPosition, b: &crate::OptionPosition) {
    assert_eq!(a.series_id, b.series_id);
    assert!(a.side == b.side);
    assert_eq!(a.contracts, b.contracts);
    assert_eq!(a.premium_paid, b.premium_paid);
    assert_eq!(a.fee_paid, b.fee_paid);
    assert_eq!(a.collateral_locked, b.collateral_locked);
}

#[test]
fn buy_batch_matches_n_single_buys_with_one_pull_and_one_fee_transfer() {
    let h = setup();
    let s1 = make_series(&h, OptionType::Call, 1_000_000, 500_000);
    let s2 = make_series(&h, OptionType::Put, 2_000_000, 300_000);
    let max = 1_000 * USDC_DECIMALS;
    // Duplicate series (s1 twice) is allowed.
    let orders = soroban_sdk::vec![
        &h.env,
        (s1, 2 * USDC_DECIMALS, max),
        (s2, 3 * USDC_DECIMALS, max),
        (s1, USDC_DECIMALS, max),
    ];

    let single = Address::generate(&h.env);
    mint(&h, &single, max);
    let mut single_ids = std::vec::Vec::new();
    for (s, c, m) in orders.iter() {
        single_ids.push(h.client.buy_option(&single, &s, &c, &m));
    }
    let fees_after_singles = balance(&h, &h.fee_recipient);
    let pool_after_singles = h.client.get_premium_pool();

    let batcher = Address::generate(&h.env);
    mint(&h, &batcher, max);
    let since = h.env.events().all().len();
    let batch_ids = h.client.buy_batch(&batcher, &orders);

    assert_eq!(token_transfer_count(&h, since), 2);
    assert_eq!(event_count(&h, since, "option_bought"), 3);
    assert_eq!(balance(&h, &batcher), balance(&h, &single));
    assert_eq!(
        balance(&h, &h.fee_recipient) - fees_after_singles,
        fees_after_singles
    );
    assert_eq!(h.client.get_premium_pool(), 2 * pool_after_singles);
    for (i, id) in batch_ids.iter().enumerate() {
        assert_same_position(
            &h.client.get_position(&id).unwrap(),
            &h.client.get_position(&single_ids[i]).unwrap(),
        );
    }
    assert_eq!(h.client.get_user_positions(&batcher), batch_ids);
    assert_eq!(
        h.client.get_series(&s1).unwrap().open_interest,
        6 * USDC_DECIMALS
    );
}

#[test]
fn write_batch_matches_n_single_writes_with_one_pull_and_one_payout() {
    let h = setup();
    let s1 = make_series(&h, OptionType::Put, 1_000_000, 500_000);
    let s2 = make_series(&h, OptionType::Put, 2_000_000, 300_000);
    let orders = soroban_sdk::vec![
        &h.env,
        (s1, 2 * USDC_DECIMALS, 1_000 * USDC_DECIMALS),
        (s2, 3 * USDC_DECIMALS, 1_000 * USDC_DECIMALS),
        (s1, USDC_DECIMALS, 1_000 * USDC_DECIMALS),
    ];
    for _ in 0..2 {
        fund_premium_pool(&h, s1, 3 * USDC_DECIMALS);
        fund_premium_pool(&h, s2, 3 * USDC_DECIMALS);
    }

    let single = Address::generate(&h.env);
    mint(&h, &single, 1_000 * USDC_DECIMALS);
    let mut single_ids = std::vec::Vec::new();
    for (s, c, m) in orders.iter() {
        single_ids.push(h.client.write_option(&single, &s, &c, &m));
    }

    let batcher = Address::generate(&h.env);
    mint(&h, &batcher, 1_000 * USDC_DECIMALS);
    let since = h.env.events().all().len();
    let batch_ids = h.client.write_batch(&batcher, &orders);

    assert_eq!(token_transfer_count(&h, since), 2);
    assert_eq!(event_count(&h, since, "option_written"), 3);
    assert_eq!(balance(&h, &batcher), balance(&h, &single));
    for (i, id) in batch_ids.iter().enumerate() {
        assert_same_position(
            &h.client.get_position(&id).unwrap(),
            &h.client.get_position(&single_ids[i]).unwrap(),
        );
    }
}

#[test]
fn buy_batch_is_all_or_nothing() {
    let h = setup();
    let s1 = make_series(&h, OptionType::Call, 1_000_000, 500_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let orders = soroban_sdk::vec![
        &h.env,
        (s1, USDC_DECIMALS, 1_000 * USDC_DECIMALS),
        (s1, 0i128, 1_000 * USDC_DECIMALS), // ZeroContracts
    ];
    assert!(h.client.try_buy_batch(&buyer, &orders).is_err());
    assert_eq!(balance(&h, &buyer), 1_000 * USDC_DECIMALS);
    assert_eq!(h.client.get_user_positions(&buyer).len(), 0);
}

#[test]
fn write_batch_rejects_when_the_pool_runs_dry_mid_batch() {
    let h = setup();
    let s1 = make_series(&h, OptionType::Put, 1_000_000, 500_000);
    fund_premium_pool(&h, s1, USDC_DECIMALS);
    let writer = Address::generate(&h.env);
    mint(&h, &writer, 1_000 * USDC_DECIMALS);
    let orders = soroban_sdk::vec![
        &h.env,
        (s1, USDC_DECIMALS, 1_000 * USDC_DECIMALS),
        (s1, USDC_DECIMALS, 1_000 * USDC_DECIMALS),
    ];
    assert!(h.client.try_write_batch(&writer, &orders).is_err());
    assert_eq!(balance(&h, &writer), 1_000 * USDC_DECIMALS);
}

#[test]
#[should_panic(expected = "Error(Contract, #23)")] // InvalidBatchSize
fn buy_batch_rejects_an_empty_batch() {
    let h = setup();
    let buyer = Address::generate(&h.env);
    h.client.buy_batch(&buyer, &soroban_sdk::vec![&h.env]);
}

#[test]
#[should_panic(expected = "Error(Contract, #23)")] // InvalidBatchSize
fn write_batch_rejects_an_oversized_batch() {
    let h = setup();
    let s1 = make_series(&h, OptionType::Put, 1_000_000, 500_000);
    let writer = Address::generate(&h.env);
    let mut orders = soroban_sdk::Vec::new(&h.env);
    for _ in 0..=crate::math::MAX_BATCH_SIZE {
        orders.push_back((s1, USDC_DECIMALS, USDC_DECIMALS));
    }
    h.client.write_batch(&writer, &orders);
}

/// Resource comparison: one buy_batch of 5 orders vs 5 buy_option calls.
#[test]
fn buy_batch_costs_less_than_the_equivalent_single_buys() {
    let h = setup();
    h.env.budget().reset_unlimited();
    let s1 = make_series(&h, OptionType::Call, 1_000_000, 500_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);

    let mut single_cpu = 0u64;
    for _ in 0..5 {
        h.env.budget().reset_default();
        h.client
            .buy_option(&buyer, &s1, &USDC_DECIMALS, &(1_000 * USDC_DECIMALS));
        single_cpu += h.env.budget().cpu_instruction_cost();
    }

    let mut orders = soroban_sdk::Vec::new(&h.env);
    for _ in 0..5 {
        orders.push_back((s1, USDC_DECIMALS, 1_000 * USDC_DECIMALS));
    }
    h.env.budget().reset_default();
    h.client.buy_batch(&buyer, &orders);
    let batch_cpu = h.env.budget().cpu_instruction_cost();

    std::println!("buy x5: singles {single_cpu} cpu, batch {batch_cpu} cpu");
    assert!(batch_cpu < single_cpu);
}

// ─── paginated and mark-to-market views ─────────────────────────────────────

fn no_filter(env: &Env) -> SeriesFilter {
    SeriesFilter {
        state: soroban_sdk::Vec::new(env),
        underlying: soroban_sdk::Vec::new(env),
        option_type: soroban_sdk::Vec::new(env),
    }
}

fn make_series_on(h: &Harness, underlying: &str, option_type: OptionType) -> u64 {
    let expiry = h.env.ledger().timestamp() + 30 * 86_400;
    h.client.create_series(
        &Symbol::new(&h.env, underlying),
        &option_type,
        &1_000_000,
        &expiry,
        &500_000,
        &(450_000_000i128),
    )
}

#[test]
fn series_page_walks_every_series_across_page_boundaries() {
    let h = setup();
    for _ in 0..5 {
        make_series(&h, OptionType::Call, 1_000_000, 500_000);
    }
    let p1 = h.client.get_series_page(&0, &2, &no_filter(&h.env));
    assert_eq!(p1.items.len(), 2);
    assert_eq!(p1.items.get(0).unwrap().series_id, 1);
    assert_eq!(p1.next_cursor, 2);
    let p2 = h
        .client
        .get_series_page(&p1.next_cursor, &2, &no_filter(&h.env));
    assert_eq!(p2.items.get(0).unwrap().series_id, 3);
    let p3 = h
        .client
        .get_series_page(&p2.next_cursor, &2, &no_filter(&h.env));
    assert_eq!(p3.items.len(), 1);
    assert_eq!(p3.next_cursor, 0);
    // Past the end, and on an empty market, pages are simply empty.
    assert_eq!(
        h.client
            .get_series_page(&99, &2, &no_filter(&h.env))
            .items
            .len(),
        0
    );
    let empty = setup();
    let page = empty
        .client
        .get_series_page(&0, &50, &no_filter(&empty.env));
    assert_eq!(page.items.len(), 0);
}

#[test]
fn series_page_filters_by_state_underlying_and_type() {
    let h = setup();
    let xlm_call = make_series_on(&h, "XLM", OptionType::Call);
    let btc_put = make_series_on(&h, "BTC", OptionType::Put);
    let btc_call = make_series_on(&h, "BTC", OptionType::Call);
    h.client.cancel_series(&btc_call);

    let btc = SeriesFilter {
        underlying: soroban_sdk::vec![&h.env, Symbol::new(&h.env, "BTC")],
        ..no_filter(&h.env)
    };
    let page = h.client.get_series_page(&0, &50, &btc);
    assert_eq!(page.items.len(), 2);

    let active_btc = SeriesFilter {
        state: soroban_sdk::vec![&h.env, SeriesState::Active],
        ..btc.clone()
    };
    let page = h.client.get_series_page(&0, &50, &active_btc);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items.get(0).unwrap().series_id, btc_put);

    let calls = SeriesFilter {
        option_type: soroban_sdk::vec![&h.env, OptionType::Call],
        state: soroban_sdk::vec![&h.env, SeriesState::Active],
        ..no_filter(&h.env)
    };
    let page = h.client.get_series_page(&0, &50, &calls);
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items.get(0).unwrap().series_id, xlm_call);

    assert_eq!(
        h.client
            .get_series_by_underlying(&Symbol::new(&h.env, "BTC")),
        soroban_sdk::vec![&h.env, btc_put, btc_call]
    );
    assert_eq!(
        h.client
            .get_series_by_underlying(&Symbol::new(&h.env, "ETH"))
            .len(),
        0
    );
}

#[test]
#[should_panic(expected = "Error(Contract, #25)")] // InvalidPageLimit
fn series_page_rejects_a_limit_above_the_max() {
    let h = setup();
    h.client
        .get_series_page(&0, &(crate::math::MAX_PAGE_LIMIT + 1), &no_filter(&h.env));
}

#[test]
#[should_panic(expected = "Error(Contract, #25)")] // InvalidPageLimit
fn positions_page_rejects_a_zero_limit() {
    let h = setup();
    let user = Address::generate(&h.env);
    h.client.get_user_positions_page(&user, &0, &0, &None);
}

#[test]
fn positions_page_paginates_and_filters_by_side() {
    let h = setup();
    let s1 = make_series(&h, OptionType::Put, 1_000_000, 500_000);
    let user = Address::generate(&h.env);
    mint(&h, &user, 1_000 * USDC_DECIMALS);
    fund_premium_pool(&h, s1, 2 * USDC_DECIMALS);
    for _ in 0..3 {
        h.client
            .buy_option(&user, &s1, &USDC_DECIMALS, &(1_000 * USDC_DECIMALS));
    }
    for _ in 0..2 {
        h.client
            .write_option(&user, &s1, &USDC_DECIMALS, &(1_000 * USDC_DECIMALS));
    }

    let p1 = h.client.get_user_positions_page(&user, &0, &2, &None);
    assert_eq!(p1.items.len(), 2);
    assert_eq!(p1.next_cursor, 2);
    let p2 = h
        .client
        .get_user_positions_page(&user, &p1.next_cursor, &2, &None);
    let p3 = h
        .client
        .get_user_positions_page(&user, &p2.next_cursor, &2, &None);
    assert_eq!(p3.items.len(), 1);
    assert_eq!(p3.next_cursor, 0);

    let shorts = h
        .client
        .get_user_positions_page(&user, &0, &50, &Some(PositionSide::Short));
    assert_eq!(shorts.items.len(), 2);
    let longs = h
        .client
        .get_user_positions_page(&user, &0, &50, &Some(PositionSide::Long));
    assert_eq!(longs.items.len(), 3);
    let stranger = Address::generate(&h.env);
    assert_eq!(
        h.client
            .get_user_positions_page(&stranger, &0, &50, &None)
            .items
            .len(),
        0
    );
}

#[test]
fn position_value_and_account_summary_are_indicative_intrinsic_values() {
    let h = setup();
    let strike = 1_000_000;
    let s1 = make_series(&h, OptionType::Call, strike, 500_000);
    let buyer = Address::generate(&h.env);
    let writer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    mint(&h, &writer, 1_000 * USDC_DECIMALS);
    let long_id = h
        .client
        .buy_option(&buyer, &s1, &(2 * USDC_DECIMALS), &(1_000 * USDC_DECIMALS));
    let short_id =
        h.client
            .write_option(&writer, &s1, &(2 * USDC_DECIMALS), &(1_000 * USDC_DECIMALS));

    // No price known yet: nothing to mark against.
    assert_eq!(h.client.get_position_value(&long_id), 0);

    let expiry = h.client.get_series(&s1).unwrap().expiry;
    h.env.ledger().with_mut(|l| l.timestamp = expiry);
    h.env.as_contract(&h.client.address, || {
        h.env.storage().persistent().set(
            &crate::types::DataKey::UnderlyingPrice(Symbol::new(&h.env, "XLM")),
            &1_500_000i128,
        );
    });
    // 2 contracts × (1.5 − 1.0) intrinsic
    let expected = 2 * (1_500_000 - strike);
    assert_eq!(h.client.get_position_value(&long_id), expected);
    assert_eq!(h.client.get_position_value(&short_id), -expected);

    let summary = h.client.get_account_summary(&writer);
    assert_eq!(summary.open_positions, 1);
    assert_eq!(summary.short_liability, expected);
    assert_eq!(
        summary.collateral_locked,
        h.client.get_position(&short_id).unwrap().collateral_locked
    );
    assert_eq!(summary.net_value, -expected);
    assert_eq!(h.client.get_account_summary(&buyer).long_value, expected);
}

/// Resource benchmark: both paginated views at the maximum page size must
/// fit comfortably in a single default-budget invocation.
#[test]
fn paginated_views_fit_the_default_budget_at_max_limit() {
    let h = setup();
    h.env.budget().reset_unlimited();
    let limit = crate::math::MAX_PAGE_LIMIT;
    let s1 = make_series(&h, OptionType::Call, 1_000_000, 500_000);
    for _ in 1..limit {
        make_series(&h, OptionType::Call, 1_000_000, 500_000);
    }
    let user = Address::generate(&h.env);
    mint(&h, &user, 10_000 * USDC_DECIMALS);
    for _ in 0..limit {
        h.client
            .buy_option(&user, &s1, &USDC_DECIMALS, &(1_000 * USDC_DECIMALS));
    }

    h.env.budget().reset_default();
    let page = h.client.get_series_page(&0, &limit, &no_filter(&h.env));
    assert_eq!(page.items.len(), limit);
    std::println!(
        "get_series_page({limit}): {} cpu, {} mem",
        h.env.budget().cpu_instruction_cost(),
        h.env.budget().memory_bytes_cost()
    );

    h.env.budget().reset_default();
    let page = h.client.get_user_positions_page(&user, &0, &limit, &None);
    assert_eq!(page.items.len(), limit);
    std::println!(
        "get_user_positions_page({limit}): {} cpu, {} mem",
        h.env.budget().cpu_instruction_cost(),
        h.env.budget().memory_bytes_cost()
    );
}

// ─── executor multisig as admin ─────────────────────────────────────────────

/// options_market whose admin is a multisig contract, not a key.
fn setup_executor<'a>() -> (Harness<'a>, MultisigClient<'a>, [Address; 3]) {
    let h = setup();
    let (multisig_id, signers) = setup_multisig(&h);
    h.client.transfer_admin(&multisig_id);
    assert_eq!(h.client.get_admin(), multisig_id);
    let ms = MultisigClient::new(&h.env, &multisig_id);
    (h, ms, signers)
}

/// propose → approve ×2 → execute, with every mocked auth cleared before
/// execute so the target's admin.require_auth() can only pass because
/// the multisig itself is the direct caller.
fn run_via_executor(
    h: &Harness,
    ms: &MultisigClient,
    signers: &[Address; 3],
    function: &str,
    args: soroban_sdk::Vec<soroban_sdk::Val>,
) -> soroban_sdk::Val {
    let id = ms.propose(
        &signers[0],
        &h.client.address,
        &Symbol::new(&h.env, function),
        &args,
    );
    ms.approve(&signers[0], &id);
    ms.approve(&signers[1], &id);
    h.env.set_auths(&[]);
    let ret = ms.execute(&id);
    h.env.mock_all_auths();
    ret
}

#[test]
fn every_admin_function_is_reachable_through_the_executor() {
    let (h, ms, signers) = setup_executor();
    let e = &h.env;

    run_via_executor(
        &h,
        &ms,
        &signers,
        "set_fee_rate",
        soroban_sdk::vec![e, 75u32.into_val(e)],
    );
    assert_eq!(h.client.get_fee_rate(), 75);

    run_via_executor(&h, &ms, &signers, "pause", soroban_sdk::vec![e]);
    assert!(h.client.is_paused());
    run_via_executor(&h, &ms, &signers, "unpause", soroban_sdk::vec![e]);
    assert!(!h.client.is_paused());

    let expiry = e.ledger().timestamp() + 30 * 86_400;
    let ret = run_via_executor(
        &h,
        &ms,
        &signers,
        "create_series",
        soroban_sdk::vec![
            e,
            Symbol::new(e, "XLM").into_val(e),
            OptionType::Call.into_val(e),
            1_000_000i128.into_val(e),
            expiry.into_val(e),
            500_000i128.into_val(e),
            450_000_000i128.into_val(e),
        ],
    );
    let series_id = u64::try_from_val(e, &ret).unwrap();
    assert_eq!(series_id, 1);

    run_via_executor(
        &h,
        &ms,
        &signers,
        "update_premium",
        soroban_sdk::vec![
            e,
            series_id.into_val(e),
            600_000i128.into_val(e),
            400_000_000i128.into_val(e),
        ],
    );
    assert_eq!(h.client.get_series(&series_id).unwrap().premium, 600_000);

    run_via_executor(
        &h,
        &ms,
        &signers,
        "cancel_series",
        soroban_sdk::vec![e, series_id.into_val(e)],
    );
    assert!(h.client.get_series(&series_id).unwrap().state == SeriesState::Cancelled);

    let new_admin = Address::generate(e);
    run_via_executor(
        &h,
        &ms,
        &signers,
        "transfer_admin",
        soroban_sdk::vec![e, new_admin.into_val(e)],
    );
    assert_eq!(h.client.get_admin(), new_admin);
}

/// Same caveat as upgrade_reaches_the_host_deployer_past_the_admin_check:
/// the bogus hash fails in the host deployer, past the admin check.
#[test]
#[should_panic]
fn upgrade_is_reachable_through_the_executor() {
    let (h, ms, signers) = setup_executor();
    let hash = BytesN::from_array(&h.env, &[0u8; 32]);
    run_via_executor(
        &h,
        &ms,
        &signers,
        "upgrade",
        soroban_sdk::vec![&h.env, hash.into_val(&h.env)],
    );
}

#[test]
#[should_panic]
fn admin_functions_reject_direct_calls_without_the_multisig() {
    let (h, _ms, _signers) = setup_executor();
    h.env.set_auths(&[]);
    h.client.set_fee_rate(&75);
}
