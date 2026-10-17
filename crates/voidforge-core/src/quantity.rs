//! Continuous quantities: a value with a constant rate between events (Architectural Design,
//! Time model, State model and Rounding formulas; decision log D-018).
//!
//! A [`Quantity`] is checkpointed: its value in grams, its rate in grams per hour, a carried
//! remainder in 1/3,600,000 g, and the game time of the checkpoint in milliseconds. Reading it at
//! a later time is pure:
//!
//! ```text
//! value(t)   = value + floor((rate * (t - checkpoint) + remainder) / 3_600_000)
//! remainder' =         (rate * (t - checkpoint) + remainder) mod 3_600_000   (in 0..3_600_000)
//! ```
//!
//! Division rounds toward negative infinity and the remainder is never negative, so a falling
//! quantity reads one gram lower as soon as its exact value drops below a whole gram. Crossing
//! times are the first millisecond at which that read reaches the boundary, for rising and
//! falling quantities alike. All arithmetic is integer: 64-bit storage, 128-bit intermediates,
//! and every result that would leave the 64-bit range is an error, never wrapped or saturated.

/// Milliseconds per hour. Game time is in milliseconds and rates are in grams per hour; this
/// constant is fixed and never scaled by rules (the fast test rules change rates, not the clock).
pub const MS_PER_HOUR: i64 = 3_600_000;

const H: i128 = MS_PER_HOUR as i128;

/// A continuous quantity at a checkpoint. See the module documentation for the arithmetic.
/// Deserializing goes through [`Quantity::from_parts`], so a stored remainder outside its range
/// is rejected rather than accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(try_from = "QuantityParts", into = "QuantityParts")]
pub struct Quantity {
    value_g: i64,
    rate_g_per_h: i64,
    remainder: i64,
    checkpoint_ms: i64,
}

/// The serialized form of a [`Quantity`].
#[derive(serde::Serialize, serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct QuantityParts {
    value_g: i64,
    rate_g_per_h: i64,
    remainder: i64,
    checkpoint_ms: i64,
}

impl TryFrom<QuantityParts> for Quantity {
    type Error = QuantityError;

    fn try_from(parts: QuantityParts) -> Result<Self, QuantityError> {
        Self::from_parts(
            parts.value_g,
            parts.rate_g_per_h,
            parts.remainder,
            parts.checkpoint_ms,
        )
    }
}

impl From<Quantity> for QuantityParts {
    fn from(q: Quantity) -> Self {
        Self {
            value_g: q.value_g,
            rate_g_per_h: q.rate_g_per_h,
            remainder: q.remainder,
            checkpoint_ms: q.checkpoint_ms,
        }
    }
}

/// Why a quantity operation has no valid result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum QuantityError {
    /// A remainder must lie in `0..MS_PER_HOUR`.
    #[error("remainder {0} is outside 0..3600000")]
    RemainderOutOfRange(i64),
    /// Quantities are read and changed only at or after their checkpoint.
    #[error("time {time_ms} ms is before the checkpoint at {checkpoint_ms} ms")]
    BeforeCheckpoint {
        /// The requested time.
        time_ms: i64,
        /// The quantity's checkpoint.
        checkpoint_ms: i64,
    },
    /// The value at the requested time does not fit in 64 bits.
    #[error("the value would leave the 64-bit range")]
    ValueOutOfRange,
    /// The crossing time does not fit in 64 bits.
    #[error("the crossing time would leave the 64-bit range")]
    TimeOutOfRange,
}

impl Quantity {
    /// A quantity at `checkpoint_ms` with no carried remainder.
    pub const fn new(value_g: i64, rate_g_per_h: i64, checkpoint_ms: i64) -> Self {
        Self {
            value_g,
            rate_g_per_h,
            remainder: 0,
            checkpoint_ms,
        }
    }

    /// A quantity with a carried remainder, as stored in a snapshot.
    pub const fn from_parts(
        value_g: i64,
        rate_g_per_h: i64,
        remainder: i64,
        checkpoint_ms: i64,
    ) -> Result<Self, QuantityError> {
        if remainder < 0 || remainder >= MS_PER_HOUR {
            return Err(QuantityError::RemainderOutOfRange(remainder));
        }
        Ok(Self {
            value_g,
            rate_g_per_h,
            remainder,
            checkpoint_ms,
        })
    }

    /// The value at the checkpoint, in grams.
    pub const fn value_g(&self) -> i64 {
        self.value_g
    }

    /// The rate, in grams per hour; negative for a falling quantity.
    pub const fn rate_g_per_h(&self) -> i64 {
        self.rate_g_per_h
    }

    /// The carried remainder, in 1/3,600,000 g, always in `0..MS_PER_HOUR`.
    pub const fn remainder(&self) -> i64 {
        self.remainder
    }

    /// The game time of the checkpoint, in milliseconds.
    pub const fn checkpoint_ms(&self) -> i64 {
        self.checkpoint_ms
    }

    /// The value at `time_ms`, without changing the quantity.
    pub fn value_at(&self, time_ms: i64) -> Result<i64, QuantityError> {
        let (whole, _) = self.progress(time_ms)?;
        i64::try_from(i128::from(self.value_g) + whole).map_err(|_| QuantityError::ValueOutOfRange)
    }

    /// The same quantity checkpointed at `time_ms`: value brought forward, remainder carried.
    pub fn advanced_to(&self, time_ms: i64) -> Result<Self, QuantityError> {
        let (whole, remainder) = self.progress(time_ms)?;
        let value_g = i64::try_from(i128::from(self.value_g) + whole)
            .map_err(|_| QuantityError::ValueOutOfRange)?;
        let remainder = i64::try_from(remainder).map_err(|_| QuantityError::ValueOutOfRange)?;
        Ok(Self {
            value_g,
            rate_g_per_h: self.rate_g_per_h,
            remainder,
            checkpoint_ms: time_ms,
        })
    }

    /// Checkpoints at `time_ms`, then continues at `rate_g_per_h`. Rates change only here.
    pub fn with_rate(&self, time_ms: i64, rate_g_per_h: i64) -> Result<Self, QuantityError> {
        Ok(Self {
            rate_g_per_h,
            ..self.advanced_to(time_ms)?
        })
    }

    /// The first time, at or after the checkpoint, at which the value is at least `boundary_g`.
    /// `None` if it never gets there: the rate is zero or negative and it is not there yet.
    pub fn rising_crossing(&self, boundary_g: i64) -> Result<Option<i64>, QuantityError> {
        if self.value_g >= boundary_g {
            return Ok(Some(self.checkpoint_ms));
        }
        if self.rate_g_per_h <= 0 {
            return Ok(None);
        }
        // Smallest dt with rate*dt + remainder >= (boundary - value) * H.
        let needed =
            (i128::from(boundary_g) - i128::from(self.value_g)) * H - i128::from(self.remainder);
        let dt = ceil_div(needed, i128::from(self.rate_g_per_h));
        self.time_after(dt).map(Some)
    }

    /// The first time, at or after the checkpoint, at which the value is at most `boundary_g`.
    /// `None` if it never gets there: the rate is zero or positive and it is not there yet.
    pub fn falling_crossing(&self, boundary_g: i64) -> Result<Option<i64>, QuantityError> {
        if self.value_g <= boundary_g {
            return Ok(Some(self.checkpoint_ms));
        }
        if self.rate_g_per_h >= 0 {
            return Ok(None);
        }
        // value(dt) <= boundary  <=>  rate*dt + remainder < (boundary - value + 1) * H, so with
        // speed = -rate the answer is the smallest dt with speed*dt > (value - boundary - 1) * H
        // + remainder. Not the mirror of the rising formula: flooring makes a falling quantity
        // lose its next whole gram as soon as the remainder would go negative.
        let speed = -i128::from(self.rate_g_per_h);
        let below = (i128::from(self.value_g) - i128::from(boundary_g) - 1) * H
            + i128::from(self.remainder);
        let dt = below.div_euclid(speed) + 1;
        self.time_after(dt).map(Some)
    }

    /// The quantity snapped onto `boundary_g` at `time_ms`, its crossing: exactly the boundary,
    /// remainder 0, same rate (Rounding formulas: a crossing quantity snaps to its boundary).
    pub fn snapped(&self, boundary_g: i64, time_ms: i64) -> Result<Self, QuantityError> {
        self.elapsed(time_ms)?;
        Ok(Self {
            value_g: boundary_g,
            rate_g_per_h: self.rate_g_per_h,
            remainder: 0,
            checkpoint_ms: time_ms,
        })
    }

    /// Whole grams gained (negative when lost) and the new remainder at `time_ms`.
    fn progress(&self, time_ms: i64) -> Result<(i128, i128), QuantityError> {
        let elapsed = self.elapsed(time_ms)?;
        // |rate| <= 2^63 and elapsed < 2^64, so the product and sum fit in i128; checked anyway.
        let total = i128::from(self.rate_g_per_h)
            .checked_mul(elapsed)
            .and_then(|flow| flow.checked_add(i128::from(self.remainder)))
            .ok_or(QuantityError::ValueOutOfRange)?;
        Ok((total.div_euclid(H), total.rem_euclid(H)))
    }

    fn elapsed(&self, time_ms: i64) -> Result<i128, QuantityError> {
        if time_ms < self.checkpoint_ms {
            return Err(QuantityError::BeforeCheckpoint {
                time_ms,
                checkpoint_ms: self.checkpoint_ms,
            });
        }
        Ok(i128::from(time_ms) - i128::from(self.checkpoint_ms))
    }

    fn time_after(&self, dt: i128) -> Result<i64, QuantityError> {
        i64::try_from(i128::from(self.checkpoint_ms) + dt)
            .map_err(|_| QuantityError::TimeOutOfRange)
    }
}

/// `ceil(numerator / denominator)` for a positive denominator, rounding toward positive infinity
/// for either sign of the numerator.
fn ceil_div(numerator: i128, denominator: i128) -> i128 {
    -((-numerator).div_euclid(denominator))
}

#[cfg(test)]
mod tests;
