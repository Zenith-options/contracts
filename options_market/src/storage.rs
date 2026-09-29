use soroban_sdk::{Address, Env, Vec};

use crate::types::{Config, DataKey, OptionPosition, Stats, POSITIONS_PER_PAGE};

// ─── Config / Stats (issue #102) ───────────────────────────────────────────

pub fn load_config(env: &Env) -> Config {
    env.storage().instance().get(&DataKey::Config).unwrap()
}

pub fn save_config(env: &Env, config: &Config) {
    env.storage().instance().set(&DataKey::Config, config);
}

pub fn load_stats(env: &Env) -> Stats {
    env.storage()
        .instance()
        .get(&DataKey::Stats)
        .unwrap_or(Stats {
            premiums_collected: 0,
            open_interest: 0,
            premium_pool: 0,
            series_count: 0,
            position_counter: 0,
        })
}

pub fn save_stats(env: &Env, stats: &Stats) {
    env.storage().instance().set(&DataKey::Stats, stats);
}

// ─── Positions (issue #103) ────────────────────────────────────────────────

pub fn load_position(env: &Env, position_id: u64) -> Option<OptionPosition> {
    env.storage()
        .persistent()
        .get(&DataKey::Position(position_id))
}

pub fn save_position(env: &Env, position_id: u64, position: &OptionPosition) {
    env.storage()
        .persistent()
        .set(&DataKey::Position(position_id), position);
}

// ─── Paginated user position index (issue #100) ────────────────────────────
//
// Ids are appended to fixed-size pages (`POSITIONS_PER_PAGE` per page)
// instead of one ever-growing `Vec<u64>`. Each page is its own persistent
// entry, so a single trade only reads and rewrites the current page rather
// than the user's whole history — bounded write bytes no matter how many
// positions the user has opened.

pub fn user_position_count(env: &Env, user: &Address) -> u32 {
    env.storage()
        .persistent()
        .get(&DataKey::UserPositionCount(user.clone()))
        .unwrap_or(0)
}

fn load_page(env: &Env, user: &Address, page: u32) -> Vec<u64> {
    env.storage()
        .persistent()
        .get(&DataKey::UserPositionPage(user.clone(), page))
        .unwrap_or(Vec::new(env))
}

/// O(1) append: only the current (partially-filled) page is read and
/// rewritten; earlier pages are untouched.
pub fn add_user_position(env: &Env, user: &Address, position_id: u64) {
    let count = user_position_count(env, user);
    let page_index = count / POSITIONS_PER_PAGE;
    let mut page = load_page(env, user, page_index);
    page.push_back(position_id);
    env.storage()
        .persistent()
        .set(&DataKey::UserPositionPage(user.clone(), page_index), &page);
    env.storage()
        .persistent()
        .set(&DataKey::UserPositionCount(user.clone()), &(count + 1));
}

/// Returns up to `limit` position ids starting at `cursor` (an index into
/// the user's full history, oldest first).
pub fn get_user_positions_page(env: &Env, user: &Address, cursor: u32, limit: u32) -> Vec<u64> {
    let total = user_position_count(env, user);
    let mut out = Vec::new(env);
    let mut i = cursor;
    let end = total.min(cursor.saturating_add(limit));
    while i < end {
        let page = load_page(env, user, i / POSITIONS_PER_PAGE);
        if let Some(id) = page.get(i % POSITIONS_PER_PAGE) {
            out.push_back(id);
        }
        i += 1;
    }
    out
}
