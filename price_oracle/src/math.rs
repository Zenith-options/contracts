/// Caps how many feeders can be authorized at once, so aggregation (a
/// simple O(n^2) sort over collected reports) stays bounded regardless of
/// how many feeders the admin adds over the contract's lifetime.
pub const MAX_FEEDERS: u32 = 16;

/// Median of a small, fixed-size buffer of fresh feeder prices. `count` is
/// how many of the leading entries in `values` are actually populated —
/// the buffer itself is always MAX_FEEDERS-sized regardless of how many
/// feeders are currently fresh, since it's built on the stack rather than
/// a heap-allocated Vec. Panics on `count == 0`; callers must check for
/// "no fresh reports" before calling this.
pub fn median(values: &mut [i128; MAX_FEEDERS as usize], count: usize) -> i128 {
    assert!(count > 0);
    // Insertion sort: fine for MAX_FEEDERS-sized input, no allocation needed.
    for i in 1..count {
        let mut j = i;
        while j > 0 && values[j - 1] > values[j] {
            values.swap(j - 1, j);
            j -= 1;
        }
    }
    if count % 2 == 1 {
        values[count / 2]
    } else {
        (values[count / 2 - 1] + values[count / 2]) / 2
    }
}

#[cfg(test)]
mod test {
    use super::*;

    fn buf(values: &[i128]) -> ([i128; MAX_FEEDERS as usize], usize) {
        let mut buffer = [0i128; MAX_FEEDERS as usize];
        for (i, v) in values.iter().enumerate() {
            buffer[i] = *v;
        }
        (buffer, values.len())
    }

    #[test]
    fn single_value_is_its_own_median() {
        let (mut b, n) = buf(&[42]);
        assert_eq!(median(&mut b, n), 42);
    }

    #[test]
    fn odd_count_takes_the_middle_after_sorting() {
        let (mut b, n) = buf(&[5, 1, 3]);
        assert_eq!(median(&mut b, n), 3);
    }

    #[test]
    fn even_count_averages_the_two_middle_values() {
        let (mut b, n) = buf(&[10, 20, 30, 40]);
        assert_eq!(median(&mut b, n), 25);
    }

    #[test]
    fn already_sorted_input_is_unaffected() {
        let (mut b, n) = buf(&[1, 2, 3, 4, 5]);
        assert_eq!(median(&mut b, n), 3);
    }

    #[test]
    fn a_single_outlier_does_not_move_the_median() {
        // Median resists outliers the way a mean wouldn't — a stale or
        // manipulated report of 1_000_000 shouldn't move this far from 20.
        let (mut b, n) = buf(&[18, 19, 20, 21, 1_000_000]);
        assert_eq!(median(&mut b, n), 20);
    }
}
