/// floor(a × b / c) for non-negative operands. Panics on overflow or c == 0.
pub fn mul_div_floor(a: i128, b: i128, c: i128) -> i128 {
    a.checked_mul(b).unwrap().checked_div(c).unwrap()
}
