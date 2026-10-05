//! Rational numbers for time bases and aspect ratios (FFmpeg `AVRational`).

use std::fmt;

/// Reduced rational number `num / den`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Rational {
    /// Numerator.
    pub num: i32,
    /// Denominator (must be non-zero for a valid time base).
    pub den: i32,
}

impl Rational {
    /// Create a rational; does not reduce.
    pub const fn new(num: i32, den: i32) -> Self {
        Self { num, den }
    }

    /// Common 1/1 identity.
    pub const fn one() -> Self {
        Self { num: 1, den: 1 }
    }

    /// Reduce by GCD.
    pub fn reduce(self) -> Self {
        if self.num == 0 || self.den == 0 {
            return self;
        }
        let g = gcd(self.num.unsigned_abs(), self.den.unsigned_abs()) as i32;
        Self {
            num: self.num / g,
            den: self.den / g,
        }
    }

    /// Convert to `f64`.
    pub fn as_f64(self) -> f64 {
        if self.den == 0 {
            0.0
        } else {
            self.num as f64 / self.den as f64
        }
    }

    /// Invert the rational (`den/num`), or return `None` if `num == 0`.
    pub fn invert(self) -> Option<Self> {
        if self.num == 0 {
            None
        } else {
            Some(Self {
                num: self.den,
                den: self.num,
            })
        }
    }

    /// Rescale an integer timestamp from `self` to `to` time base.
    ///
    /// Equivalent to FFmpeg `av_rescale_q`.
    pub fn rescale(self, value: i64, to: Rational) -> i64 {
        if self.den == 0 || to.den == 0 {
            return 0;
        }
        // value * self.num * to.den / (self.den * to.num) — use i128 to avoid overflow
        let num = (value as i128)
            .saturating_mul(self.num as i128)
            .saturating_mul(to.den as i128);
        let den = (self.den as i128).saturating_mul(to.num as i128);
        if den == 0 {
            0
        } else {
            (num / den) as i64
        }
    }
}

impl fmt::Display for Rational {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}/{}", self.num, self.den)
    }
}

impl Default for Rational {
    fn default() -> Self {
        Self::one()
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        let t = b;
        b = a % b;
        a = t;
    }
    a
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reduce_and_rescale() {
        assert_eq!(Rational::new(2, 4).reduce(), Rational::new(1, 2));
        let from = Rational::new(1, 48_000);
        let to = Rational::new(1, 1_000_000);
        // 48000 samples @ 48kHz -> 1 second -> 1_000_000 us
        assert_eq!(from.rescale(48_000, to), 1_000_000);
    }
}
