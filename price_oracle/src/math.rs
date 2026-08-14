/// Caps how many feeders can be authorized at once, so aggregation (a
/// simple O(n^2) sort over collected reports) stays bounded regardless of
/// how many feeders the admin adds over the contract's lifetime.
pub const MAX_FEEDERS: u32 = 16;
