#![no_std]

pub struct SafeMathRounding;

impl SafeMathRounding {
    /// Safe directional mul_div calculation rounding towards floor.
    /// Favors protocol solvency when calculating user withdrawals or payouts.
    pub fn mul_div_floor(a: i128, b: i128, denominator: i128) -> Option<i128> {
        if denominator == 0 {
            return None;
        }
        let prod = a.checked_mul(b)?;
        let res = prod / denominator;
        let rem = prod % denominator;
        if rem != 0 && ((prod < 0) ^ (denominator < 0)) {
            res.checked_sub(1)
        } else {
            Some(res)
        }
    }

    /// Safe directional mul_div calculation rounding towards ceiling.
    /// Favors protocol solvency when assessing fees, collateral liabilities, or margins.
    pub fn mul_div_ceil(a: i128, b: i128, denominator: i128) -> Option<i128> {
        if denominator == 0 {
            return None;
        }
        let prod = a.checked_mul(b)?;
        let res = prod / denominator;
        let rem = prod % denominator;
        if rem != 0 && !((prod < 0) ^ (denominator < 0)) {
            res.checked_add(1)
        } else {
            Some(res)
        }
    }
}
