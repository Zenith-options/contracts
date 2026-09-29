#![no_std]

//! Zenith Streams — linear, per-second token streams with an optional
//! cliff, funded upfront by the funder (typically the treasury).
//!
//! `streamed(t) = total × (t − start) / (end − start)`, floored, clamped to
//! `[0, total]` and zero before `cliff`. Because every intermediate value
//! is floored and the recipient can only ever withdraw
//! `streamed(now) − withdrawn`, the total paid out can never exceed the
//! deposit (see the property tests in test.rs).
//!
//! The recipient withdraws at any time and may transfer the stream to a
//! new recipient. The funder or governance can cancel: the recipient is
//! paid everything accrued but not yet withdrawn, and the funder is
//! refunded the unstreamed remainder.

use soroban_sdk::{contract, contractimpl, panic_with_error, token, Address, Env};

#[cfg(test)]
mod test;

mod error;
mod events;
pub mod math;
mod types;

use error::Error;
use types::{DataKey, Stream};

#[contract]
pub struct Streams;

fn load(env: &Env, stream_id: u64) -> Stream {
    env.storage()
        .persistent()
        .get(&DataKey::Stream(stream_id))
        .unwrap_or_else(|| panic_with_error!(env, Error::StreamNotFound))
}

fn save(env: &Env, stream_id: u64, stream: &Stream) {
    env.storage()
        .persistent()
        .set(&DataKey::Stream(stream_id), stream);
}

/// Amount streamed to the recipient as of `now`, in `[0, total]`.
fn streamed_at(stream: &Stream, now: u64) -> i128 {
    if stream.canceled || now >= stream.end {
        return stream.total;
    }
    if now < stream.cliff || now <= stream.start {
        return 0;
    }
    math::mul_div_floor(
        stream.total,
        (now - stream.start) as i128,
        (stream.end - stream.start) as i128,
    )
}

#[contractimpl]
impl Streams {
    pub fn initialize(env: Env, governance: Address) {
        if env.storage().instance().has(&DataKey::Governance) {
            panic_with_error!(&env, Error::AlreadyInitialized);
        }
        env.storage()
            .instance()
            .set(&DataKey::Governance, &governance);
        env.storage().instance().set(&DataKey::StreamCounter, &0u64);
    }

    pub fn get_governance(env: Env) -> Address {
        env.storage().instance().get(&DataKey::Governance).unwrap()
    }

    /// Creates a stream and pulls `total` of `token` from `funder` upfront.
    /// `start` may be in the past (the elapsed portion is withdrawable
    /// immediately, subject to the cliff). Zero-duration streams
    /// (`start == end`) are rejected.
    #[allow(clippy::too_many_arguments)]
    pub fn create_stream(
        env: Env,
        funder: Address,
        recipient: Address,
        token: Address,
        total: i128,
        start: u64,
        cliff: u64,
        end: u64,
    ) -> u64 {
        funder.require_auth();
        if total <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        if start >= end || cliff < start || cliff > end {
            panic_with_error!(&env, Error::InvalidSchedule);
        }

        token::Client::new(&env, &token).transfer(&funder, &env.current_contract_address(), &total);

        let stream_id: u64 = env
            .storage()
            .instance()
            .get(&DataKey::StreamCounter)
            .unwrap();
        env.storage()
            .instance()
            .set(&DataKey::StreamCounter, &(stream_id + 1));

        save(
            &env,
            stream_id,
            &Stream {
                funder: funder.clone(),
                recipient: recipient.clone(),
                token,
                total,
                start,
                cliff,
                end,
                withdrawn: 0,
                canceled: false,
            },
        );
        events::created(&env, stream_id, funder, recipient, total);
        stream_id
    }

    /// Recipient withdraws up to their currently accrued balance.
    pub fn withdraw(env: Env, stream_id: u64, amount: i128) {
        let mut stream = load(&env, stream_id);
        stream.recipient.require_auth();
        if amount <= 0 {
            panic_with_error!(&env, Error::InvalidAmount);
        }
        let available = streamed_at(&stream, env.ledger().timestamp()) - stream.withdrawn;
        if amount > available {
            panic_with_error!(&env, Error::InsufficientBalance);
        }
        stream.withdrawn += amount;
        save(&env, stream_id, &stream);

        token::Client::new(&env, &stream.token).transfer(
            &env.current_contract_address(),
            &stream.recipient,
            &amount,
        );
        events::withdrawn(&env, stream_id, stream.recipient, amount);
    }

    /// Funder or governance cancels. The recipient receives everything
    /// accrued but not yet withdrawn (zero if still before the cliff) and
    /// the funder is refunded the unstreamed remainder.
    pub fn cancel(env: Env, caller: Address, stream_id: u64) {
        caller.require_auth();
        let mut stream = load(&env, stream_id);
        if caller != stream.funder && caller != Self::get_governance(env.clone()) {
            panic_with_error!(&env, Error::Unauthorized);
        }
        if stream.canceled {
            panic_with_error!(&env, Error::StreamCanceled);
        }

        let streamed = streamed_at(&stream, env.ledger().timestamp());
        let to_recipient = streamed - stream.withdrawn;
        let to_funder = stream.total - streamed;

        stream.total = streamed;
        stream.withdrawn = streamed;
        stream.canceled = true;
        save(&env, stream_id, &stream);

        let token = token::Client::new(&env, &stream.token);
        let this = env.current_contract_address();
        if to_recipient > 0 {
            token.transfer(&this, &stream.recipient, &to_recipient);
        }
        if to_funder > 0 {
            token.transfer(&this, &stream.funder, &to_funder);
        }
        events::canceled(&env, stream_id, to_recipient, to_funder);
    }

    /// Recipient hands the stream (including any accrued, unwithdrawn
    /// balance) to a new recipient.
    pub fn transfer(env: Env, stream_id: u64, new_recipient: Address) {
        let mut stream = load(&env, stream_id);
        stream.recipient.require_auth();
        if stream.canceled {
            panic_with_error!(&env, Error::StreamCanceled);
        }
        let old = stream.recipient.clone();
        stream.recipient = new_recipient.clone();
        save(&env, stream_id, &stream);
        events::transferred(&env, stream_id, old, new_recipient);
    }

    /// `(recipient_withdrawable, funder_refundable)` as of now: what the
    /// recipient could withdraw right now, and what the funder would get
    /// back if the stream were cancelled right now.
    pub fn balance_of(env: Env, stream_id: u64) -> (i128, i128) {
        let stream = load(&env, stream_id);
        let streamed = streamed_at(&stream, env.ledger().timestamp());
        (streamed - stream.withdrawn, stream.total - streamed)
    }

    pub fn get_stream(env: Env, stream_id: u64) -> Option<Stream> {
        env.storage().persistent().get(&DataKey::Stream(stream_id))
    }

    pub fn get_stream_count(env: Env) -> u64 {
        env.storage()
            .instance()
            .get(&DataKey::StreamCounter)
            .unwrap()
    }
}
