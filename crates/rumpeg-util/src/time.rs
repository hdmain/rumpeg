//! Timestamps and durations in a media time base.

use crate::rational::Rational;

/// Global high-resolution time base: microseconds (`1/1_000_000`), like FFmpeg `AV_TIME_BASE`.
pub const TIME_BASE: Rational = Rational {
    num: 1,
    den: 1_000_000,
};

/// Presentation / decode timestamp in a stream's time base.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(pub i64);

impl Timestamp {
    /// No timestamp available (FFmpeg `AV_NOPTS_VALUE` analogue).
    pub const NONE: Timestamp = Timestamp(i64::MIN);

    /// Create from raw ticks.
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// Returns `true` if this is [`Timestamp::NONE`].
    pub fn is_none(self) -> bool {
        self.0 == i64::MIN
    }

    /// Rescale into another time base.
    pub fn rescale(self, from: Rational, to: Rational) -> Self {
        if self.is_none() {
            return Self::NONE;
        }
        Self(from.rescale(self.0, to))
    }

    /// Convert to seconds as `f64`, or `None` if unset.
    pub fn as_secs(self, time_base: Rational) -> Option<f64> {
        if self.is_none() || time_base.den == 0 {
            None
        } else {
            Some(self.0 as f64 * time_base.num as f64 / time_base.den as f64)
        }
    }
}

/// Duration between two timestamps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Duration(pub i64);

impl Duration {
    /// Create from raw ticks.
    pub const fn new(value: i64) -> Self {
        Self(value)
    }

    /// Duration in seconds.
    pub fn as_secs(self, time_base: Rational) -> f64 {
        if time_base.den == 0 {
            0.0
        } else {
            self.0 as f64 * time_base.num as f64 / time_base.den as f64
        }
    }
}
