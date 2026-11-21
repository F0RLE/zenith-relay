use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{fmt, str::FromStr};

pub(super) const SCALE: u128 = 1_000_000_000_000;
const MAX: u128 = 10u128.pow(30) - 1;

/// Nonnegative numeric(30,12). Accept strings only; emit a fixed twelve-place decimal.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct Rate(pub(super) u128);

impl FromStr for Rate {
    type Err = &'static str;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let (whole, fraction) = match value.split_once('.') {
            Some((_, "")) => return Err("empty fractional part"),
            Some(pair) => pair,
            None => (value, ""),
        };
        if whole.is_empty()
            || whole.len() > 18
            || (whole.len() > 1 && whole.starts_with('0'))
            || fraction.len() > 12
            || !whole
                .bytes()
                .chain(fraction.bytes())
                .all(|b| b.is_ascii_digit())
        {
            return Err("expected a nonnegative numeric(30,12) decimal string");
        }
        let digits = |s: &str| s.bytes().fold(0u128, |n, b| n * 10 + u128::from(b - b'0'));
        Ok(Self(
            digits(whole) * SCALE + digits(fraction) * 10u128.pow(12 - fraction.len() as u32),
        ))
    }
}

impl fmt::Display for Rate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{:012}", self.0 / SCALE, self.0 % SCALE)
    }
}

impl Serialize for Rate {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for Rate {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let value = <std::borrow::Cow<'de, str>>::deserialize(d)?;
        value.parse().map_err(serde::de::Error::custom)
    }
}

impl Rate {
    pub(crate) fn fits_text_catalog(self) -> bool {
        self.0 < SCALE * SCALE
    }

    /// Reduce the denominator first: even a valid full-width rate times 100% can exceed u128.
    pub(super) fn with_markup(self, percent: Self) -> Option<Self> {
        fn gcd(mut a: u128, mut b: u128) -> u128 {
            while b != 0 {
                (a, b) = (b, a % b);
            }
            a
        }
        let denominator = 100 * SCALE;
        let divisor = gcd(self.0, denominator);
        let rest = denominator / divisor;
        let factor = denominator + percent.0;
        if !factor.is_multiple_of(rest) {
            return None;
        }
        let marked = (self.0 / divisor).checked_mul(factor / rest)?;
        (marked <= MAX).then_some(Self(marked))
    }
}
