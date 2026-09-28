#![cfg(test)]

use crate::{call, Call, Role, Timelock, TimelockClient};
extern crate std;

use soroban_sdk::{
    testutils::{Address as _, Ledger as _},
    vec, Address, BytesN, Env, IntoVal, Val, Vec,
};

use options_market::{OptionsMarket, OptionsMarketClient};

/// Minimal valid Soroban wasm for the upgrade test: an empty module
/// carrying only the `contractenvmetav0` section, lifted from a real
/// contract's wasm so the interface version always matches the SDK.
/// (A full contract wasm built by current stable rustc enables
/// reference-types, which the v21 test VM rejects on upload.)
fn empty_contract_wasm() -> std::vec::Vec<u8> {
    let src =
        include_bytes!("../../multisig/target/wasm32-unknown-unknown/release/zenith_multisig.wasm");
    let leb = |i: &mut usize| {
        let (mut v, mut shift) = (0usize, 0);
        loop {
            let b = src[*i];
            *i += 1;
            v |= ((b & 0x7f) as usize) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                return v;
            }
        }
    };
    let mut i = 8;
    while i < src.len() {
        let (id, start) = (src[i], i);
        i += 1;
        let size = leb(&mut i);
        let body = i;
        i += size;
        if id == 0 {
            let mut j = body;
            let n = leb(&mut j);
            if &src[j..j + n] == b"contractenvmetav0" {
                let mut out = src[..8].to_vec();
                out.extend_from_slice(&src[start..i]);
                return out;
            }
        }
    }
    panic!("contractenvmetav0 section not found");
}

const DELAY: u64 = 3_600;

struct Harness<'a> {
    env: Env,
    tl: TimelockClient<'a>,
    proposer: Address,
    executor: Address,
    canceller: Address,
}

fn setup<'a>() -> Harness<'a> {
    let env = Env::default();
    env.mock_all_auths();
    env.ledger().with_mut(|l| l.timestamp = 10_000);
    let proposer = Address::generate(&env);
    let executor = Address::generate(&env);
    let canceller = Address::generate(&env);
    let id = env.register_contract(None, Timelock);
    let tl = TimelockClient::new(&env, &id);
    tl.initialize(
        &DELAY,
        &vec![&env, proposer.clone()],
        &vec![&env, executor.clone()],
        &vec![&env, canceller.clone()],
    );
    Harness {
        env,
        tl,
        proposer,
        executor,
        canceller,
    }
}

fn salt(env: &Env, n: u8) -> BytesN<32> {
    BytesN::from_array(env, &[n; 32])
}

fn self_op(h: &Harness, function: &str, args: Vec<Val>) -> Vec<Call> {
    vec![&h.env, call(&h.env, &h.tl.address, function, args)]
}

fn advance(h: &Harness, secs: u64) {
    h.env.ledger().with_mut(|l| l.timestamp += secs);
}

#[test]
fn schedule_and_execute_at_exact_delay_boundary() {
    let h = setup();
    let ops = self_op(&h, "update_delay", vec![&h.env, 7_200u64.into_val(&h.env)]);
    let id =
        h.tl.schedule(&h.proposer, &ops, &None, &salt(&h.env, 1), &DELAY);
    assert!(h.tl.is_operation_pending(&id));
    assert_eq!(h.tl.get_timestamp(&id), 10_000 + DELAY);

    advance(&h, DELAY - 1);
    assert!(!h.tl.is_operation_ready(&id));
    advance(&h, 1);
    assert!(h.tl.is_operation_ready(&id));

    h.tl.execute(&h.executor, &ops, &None, &salt(&h.env, 1));
    assert!(h.tl.is_operation_done(&id));
    assert_eq!(h.tl.get_min_delay(), 7_200);
}

#[test]
#[should_panic(expected = "Error(Contract, #5)")] // NotReady
fn execute_before_delay_panics() {
    let h = setup();
    let ops = self_op(&h, "update_delay", vec![&h.env, 1u64.into_val(&h.env)]);
    h.tl.schedule(&h.proposer, &ops, &None, &salt(&h.env, 1), &DELAY);
    advance(&h, DELAY - 1);
    h.tl.execute(&h.executor, &ops, &None, &salt(&h.env, 1));
}

#[test]
#[should_panic(expected = "Error(Contract, #3)")] // InsufficientDelay
fn schedule_below_min_delay_panics() {
    let h = setup();
    let ops = self_op(&h, "update_delay", vec![&h.env, 1u64.into_val(&h.env)]);
    h.tl.schedule(&h.proposer, &ops, &None, &salt(&h.env, 1), &(DELAY - 1));
}

#[test]
#[should_panic(expected = "Error(Contract, #4)")] // AlreadyScheduled
fn rescheduling_a_done_operation_panics() {
    let h = setup();
    let ops = self_op(&h, "update_delay", vec![&h.env, DELAY.into_val(&h.env)]);
    h.tl.schedule(&h.proposer, &ops, &None, &salt(&h.env, 1), &DELAY);
    advance(&h, DELAY);
    h.tl.execute(&h.executor, &ops, &None, &salt(&h.env, 1));
    h.tl.schedule(&h.proposer, &ops, &None, &salt(&h.env, 1), &DELAY);
}

#[test]
fn predecessor_must_be_done_first() {
    let h = setup();
    let first = self_op(&h, "update_delay", vec![&h.env, DELAY.into_val(&h.env)]);
    let first_id =
        h.tl.schedule(&h.proposer, &first, &None, &salt(&h.env, 1), &DELAY);
    let second = self_op(
        &h,
        "update_delay",
        vec![&h.env, (DELAY + 1).into_val(&h.env)],
    );
    let pred = Some(first_id);
    h.tl.schedule(&h.proposer, &second, &pred, &salt(&h.env, 2), &DELAY);
    advance(&h, DELAY);

    let res =
        h.tl.try_execute(&h.executor, &second, &pred, &salt(&h.env, 2));
    assert!(res.is_err()); // PredecessorNotDone

    h.tl.execute(&h.executor, &first, &None, &salt(&h.env, 1));
    h.tl.execute(&h.executor, &second, &pred, &salt(&h.env, 2));
    assert_eq!(h.tl.get_min_delay(), DELAY + 1);
}

#[test]
fn cancel_removes_pending_operation() {
    let h = setup();
    let ops = self_op(&h, "update_delay", vec![&h.env, 1u64.into_val(&h.env)]);
    let id =
        h.tl.schedule(&h.proposer, &ops, &None, &salt(&h.env, 1), &DELAY);
    h.tl.cancel(&h.canceller, &id);
    assert!(!h.tl.is_operation(&id));
    advance(&h, DELAY);
    assert!(h
        .tl
        .try_execute(&h.executor, &ops, &None, &salt(&h.env, 1))
        .is_err());
}

#[test]
fn role_checks() {
    let h = setup();
    let stranger = Address::generate(&h.env);
    let ops = self_op(&h, "update_delay", vec![&h.env, 1u64.into_val(&h.env)]);
    assert!(h
        .tl
        .try_schedule(&stranger, &ops, &None, &salt(&h.env, 1), &DELAY)
        .is_err());
    let id =
        h.tl.schedule(&h.proposer, &ops, &None, &salt(&h.env, 1), &DELAY);
    assert!(h.tl.try_cancel(&stranger, &id).is_err());
    advance(&h, DELAY);
    assert!(h
        .tl
        .try_execute(&stranger, &ops, &None, &salt(&h.env, 1))
        .is_err());
}

#[test]
fn roles_are_managed_only_through_the_timelock() {
    let h = setup();
    let newbie = Address::generate(&h.env);
    let ops = vec![
        &h.env,
        call(
            &h.env,
            &h.tl.address,
            "grant_role",
            vec![
                &h.env,
                Role::Proposer.into_val(&h.env),
                newbie.into_val(&h.env),
            ],
        ),
        call(
            &h.env,
            &h.tl.address,
            "set_open_executor",
            vec![&h.env, true.into_val(&h.env)],
        ),
    ];
    h.tl.schedule(&h.proposer, &ops, &None, &salt(&h.env, 1), &DELAY);
    advance(&h, DELAY);
    h.tl.execute(&h.executor, &ops, &None, &salt(&h.env, 1));
    assert!(h.tl.has_role(&Role::Proposer, &newbie));
    assert!(h.tl.is_open_executor());
}

/// The timelock administers a real options_market deployment: a fee
/// change and an upgrade, each only after the delay.
#[test]
fn timelock_administers_options_market_fee_change_and_upgrade() {
    let h = setup();
    let env = &h.env;
    let market_id = env.register_contract(None, OptionsMarket);
    let market = OptionsMarketClient::new(env, &market_id);
    market.initialize(
        &h.tl.address,
        &Address::generate(env),
        &Address::generate(env),
        &Address::generate(env),
    );

    let fee_ops = vec![
        env,
        call(
            env,
            &market_id,
            "set_fee_rate",
            vec![env, 25u32.into_val(env)],
        ),
    ];
    h.tl.schedule(&h.proposer, &fee_ops, &None, &salt(env, 1), &DELAY);
    assert!(h
        .tl
        .try_execute(&h.executor, &fee_ops, &None, &salt(env, 1))
        .is_err());
    advance(&h, DELAY);
    h.tl.execute(&h.executor, &fee_ops, &None, &salt(env, 1));
    assert_eq!(market.get_fee_rate(), 25);

    // Upgrade to an empty module: afterwards the old entrypoints are gone.
    let new_hash = env
        .deployer()
        .upload_contract_wasm(empty_contract_wasm().as_slice());
    let up_ops = vec![
        env,
        call(
            env,
            &market_id,
            "upgrade",
            vec![env, new_hash.into_val(env)],
        ),
    ];
    h.tl.schedule(&h.proposer, &up_ops, &None, &salt(env, 2), &DELAY);
    advance(&h, DELAY);
    h.tl.execute(&h.executor, &up_ops, &None, &salt(env, 2));
    assert!(market.try_get_fee_rate().is_err());
}
