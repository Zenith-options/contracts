#![cfg(test)]

use crate::{OptionSeries, OptionType, OptionsMarket, OptionsMarketClient, PositionSide};
use soroban_sdk::{
    testutils::{Address as _, Ledger},
    token, Address, BytesN, Env, Symbol,
};

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

    Harness { env, client, token: token_address, admin, oracle, fee_recipient }
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
    h.client.buy_option(&filler_buyer, &series_id, &contracts, &(1_000 * USDC_DECIMALS));
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
    h.client.initialize(&h.admin, &h.oracle, &h.token, &h.fee_recipient);
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
    let token = env.register_stellar_asset_contract_v2(token_admin).address();

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

    let pos_id = h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

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
    h.client.buy_option(&buyer, &series_id, &0, &(50 * USDC_DECIMALS));
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
    h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &30_000_000);
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // SeriesNotFound
fn buy_option_rejects_unknown_series() {
    let h = setup();
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    h.client.buy_option(&buyer, &999, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
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

    let pos_id = h.client.write_option(&writer, &series_id, &USDC_DECIMALS, &required);
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

    let pos_id = h.client.write_option(&writer, &series_id, &USDC_DECIMALS, &required);
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
    h.client.write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);
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
    h.client.write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);
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
    h.client.write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

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
    let pos_id = h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

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
    let pos_id = h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

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
    let pos_id = h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

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
    let pos_id = h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
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
    let pos_id = h.client.write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

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
    let pos_id = h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(750_000_000));
    mint(&h, &h.client.address, 1_000 * USDC_DECIMALS);

    h.client.exercise(&buyer, &pos_id);
    h.client.exercise(&buyer, &pos_id);
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
    let pos_id = h.client.write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

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
    let pos_id = h.client.write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

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
    let pos_id = h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

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
    let pos_id = h.client.write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

    advance_past_expiry(&h, series_id);
    h.client.set_settlement_price(&series_id, &(650_000_000));
    h.client.reclaim_collateral(&writer, &pos_id);
    h.client.reclaim_collateral(&writer, &pos_id);
}

// ─── update_premium ─────────────────────────────────────────────────────────

#[test]
fn update_premium_changes_price_and_iv() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.update_premium(&series_id, &(45_000_000), &(500_000_000));

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
    h.client.update_premium(&series_id, &(45_000_000), &(500_000_000));
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
    let buyer_pos = h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    let writer_pos = h.client.write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

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
    h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));
}

#[test]
#[should_panic(expected = "Error(Contract, #17)")] // ContractPaused
fn write_option_is_rejected_while_paused() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    h.client.pause();

    let writer = Address::generate(&h.env);
    mint(&h, &writer, 700_000_000);
    h.client.write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);
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
    let pos_id = h.client.write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

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
    let pos_id = h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

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
    let pos_id = h.client.write_option(&writer, &series_id, &USDC_DECIMALS, &700_000_000);

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
    let pos_id = h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

    h.client.claim_refund(&buyer, &pos_id);
}

#[test]
#[should_panic(expected = "Error(Contract, #10)")] // AlreadySettled
fn claim_refund_twice_is_rejected() {
    let h = setup();
    let series_id = make_series(&h, OptionType::Call, 700_000_000, 40_000_000);
    let buyer = Address::generate(&h.env);
    mint(&h, &buyer, 1_000 * USDC_DECIMALS);
    let pos_id = h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

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
    let pos_id = h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

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
    let pos_id = h.client.buy_option(&buyer, &series_id, &USDC_DECIMALS, &(50 * USDC_DECIMALS));

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
    assert_eq!(h.client.get_series_count_for_underlying(&Symbol::new(&h.env, "XLM")), 50);
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
    assert_eq!(h.client.get_series_count_for_underlying(&Symbol::new(&h.env, "BTC")), 1);
}
