#![no_std]

use soroban_sdk::{contracttype, Address, Env, Symbol};

#[contracttype]
#[derive(Clone, Debug, PartialEq)]
pub struct HistoricalRoundReport {
    pub round_id: u64,
    pub price: i128,
    pub timestamp: u64,
    pub reporter: Address,
}

pub struct RoundHistoryBuffer;

impl RoundHistoryBuffer {
    pub const MAX_ROUNDS: u64 = 256;

    pub fn insert_round(env: &Env, symbol: Symbol, round_id: u64, price: i128, reporter: Address) {
        let index = round_id % Self::MAX_ROUNDS;
        let report = HistoricalRoundReport {
            round_id,
            price,
            timestamp: env.ledger().timestamp(),
            reporter,
        };

        env.storage().persistent().set(&(symbol, index), &report);
    }

    pub fn query_round(env: &Env, symbol: Symbol, round_id: u64) -> Option<HistoricalRoundReport> {
        let index = round_id % Self::MAX_ROUNDS;
        let report: HistoricalRoundReport = env.storage().persistent().get(&(symbol, index))?;
        if report.round_id == round_id {
            Some(report)
        } else {
            None
        }
    }
}
