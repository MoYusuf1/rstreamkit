//! Exact removable sums of binary64 durations with bounded storage.
// Each finite f64 is an integer multiple of 2^-1074. Its magnitude occupies at
// most 2098 bits; summing at most usize::MAX values on <=64-bit targets needs
// at most 2162 bits. A signed 2176-bit accumulator therefore cannot overflow.
// Arithmetic below deliberately wraps as two's complement, including sign changes.
const WORDS: usize = 34;
const FRACTION: u64 = (1 << 52) - 1;

pub(crate) enum DurationSum {
    Uniform { count: usize, duration: f64 },
    Small { units: u128, scale: usize },
    Wide(Box<WideSum>),
}
impl DurationSum {
    pub fn new(values: impl Iterator<Item = f64> + Clone) -> Self {
        let mut low = usize::MAX;
        let mut high = 0;
        let mut count = 0usize;
        let mut entries = 0usize;
        let mut first = None;
        let mut uniform = true;
        for value in values.clone() {
            entries += 1;
            if let Some(initial) = first {
                uniform &= initial == value;
            } else {
                first = Some(value);
            }
            if value == 0.0 {
                continue;
            }
            if !value.is_finite() || value < 0.0 {
                return Self::Wide(Box::new(values.collect()));
            }
            count += 1;
            if count == 1 || Some(value) != first {
                let (mantissa, shift) = parts(value);
                low = low.min(shift + mantissa.trailing_zeros() as usize);
                high = high.max(shift + 63 - mantissa.leading_zeros() as usize);
            }
        }
        // Count is exactly representable: one correctly rounded multiplication
        // is then the exact sum's binary64 rounding, including overflow.
        if uniform && entries as u64 <= 1u64 << 53 {
            return Self::Uniform {
                count: entries,
                duration: first.unwrap_or(0.0),
            };
        }
        if count == 0 {
            return Self::Small { units: 0, scale: 0 };
        }
        let growth = (usize::BITS - (count - 1).leading_zeros()) as usize;
        if high - low + 1 + growth > 128 {
            return Self::Wide(Box::new(values.collect()));
        }
        let units = values.map(|value| scaled(value, low)).sum();
        Self::Small { units, scale: low }
    }
    pub fn remove(&mut self, value: f64) {
        match self {
            Self::Uniform { count, .. } => *count -= 1,
            Self::Small { units, scale } => *units -= scaled(value, *scale),
            Self::Wide(sum) => sum.remove(value),
        }
    }
    pub fn value(&self) -> f64 {
        match self {
            Self::Uniform { count, duration } => {
                if *count == 0 || *duration == 0.0 {
                    0.0
                } else {
                    *duration * *count as f64
                }
            }
            Self::Wide(sum) => sum.value(),
            Self::Small { units: 0, .. } => 0.0,
            Self::Small { units, scale } => {
                let relative = 127 - units.leading_zeros() as usize;
                let mut highest = relative + scale;
                if highest < 52 {
                    return f64::from_bits((*units as u64) << scale);
                }
                let mut mantissa = if relative <= 52 {
                    *units << (52 - relative)
                } else {
                    *units >> (relative - 52)
                };
                if relative > 52 {
                    let cut = relative - 52;
                    let discarded = *units & ((1u128 << cut) - 1);
                    let half = 1u128 << (cut - 1);
                    if discarded > half || (discarded == half && mantissa & 1 != 0) {
                        mantissa += 1;
                    }
                }
                if mantissa == 1 << 53 {
                    mantissa >>= 1;
                    highest += 1;
                }
                let exponent = highest - 51;
                if exponent >= 0x7ff {
                    return f64::INFINITY;
                }
                f64::from_bits((exponent as u64) << 52 | (mantissa as u64 & FRACTION))
            }
        }
    }
}
fn parts(value: f64) -> (u64, usize) {
    let bits = value.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as usize;
    (
        bits & FRACTION | if exponent == 0 { 0 } else { 1 << 52 },
        exponent.saturating_sub(1),
    )
}
fn scaled(value: f64, scale: usize) -> u128 {
    let (mantissa, shift) = parts(value);
    if mantissa == 0 {
        return 0;
    }
    let zeros = mantissa.trailing_zeros() as usize;
    u128::from(mantissa >> zeros) << (shift + zeros - scale)
}

pub(crate) struct WideSum {
    units: [u64; WORDS],
    positive_infinity: usize,
    negative_infinity: usize,
    nans: usize,
}
impl Default for WideSum {
    fn default() -> Self {
        Self {
            units: [0; WORDS],
            positive_infinity: 0,
            negative_infinity: 0,
            nans: 0,
        }
    }
}
impl FromIterator<f64> for WideSum {
    fn from_iter<T: IntoIterator<Item = f64>>(values: T) -> Self {
        let mut sum = Self::default();
        for value in values {
            sum.change(value, false);
        }
        sum
    }
}
impl WideSum {
    pub fn remove(&mut self, value: f64) {
        self.change(value, true);
    }

    fn change(&mut self, value: f64, remove: bool) {
        let bits = value.to_bits();
        let exponent = ((bits >> 52) & 0x7ff) as usize;
        let fraction = bits & FRACTION;
        let negative = bits >> 63 != 0;
        if exponent == 0x7ff {
            let count = if fraction != 0 {
                &mut self.nans
            } else if negative {
                &mut self.negative_infinity
            } else {
                &mut self.positive_infinity
            };
            if remove {
                *count -= 1;
            } else {
                *count += 1;
            }
            return;
        }
        let mantissa = fraction | if exponent == 0 { 0 } else { 1 << 52 };
        let shift = exponent.saturating_sub(1);
        let word = shift / 64;
        let offset = shift % 64;
        let subtract = negative != remove;
        self.change_word(word, mantissa << offset, subtract);
        if offset != 0 {
            self.change_word(word + 1, mantissa >> (64 - offset), subtract);
        }
    }

    fn change_word(&mut self, mut index: usize, mut value: u64, subtract: bool) {
        while value != 0 && index < WORDS {
            let (next, carry) = if subtract {
                self.units[index].overflowing_sub(value)
            } else {
                self.units[index].overflowing_add(value)
            };
            self.units[index] = next;
            value = u64::from(carry);
            index += 1;
        }
    }

    pub fn value(&self) -> f64 {
        if self.nans != 0 || (self.positive_infinity != 0 && self.negative_infinity != 0) {
            return f64::NAN;
        }
        if self.positive_infinity != 0 {
            return f64::INFINITY;
        }
        if self.negative_infinity != 0 {
            return f64::NEG_INFINITY;
        }
        if self.units[WORDS - 1] >> 63 == 0 {
            return Self::rounded(&self.units, false);
        }
        let mut magnitude = self.units.map(|word| !word);
        for word in &mut magnitude {
            let (next, carry) = word.overflowing_add(1);
            *word = next;
            if !carry {
                break;
            }
        }
        Self::rounded(&magnitude, true)
    }

    // Convert to binary64 with round-to-nearest, ties-to-even. Small integers in
    // these units are already exact subnormals; larger values retain 53 bits.
    fn rounded(units: &[u64; WORDS], negative: bool) -> f64 {
        let Some(top) = units.iter().rposition(|&word| word != 0) else {
            return 0.0;
        };
        let mut highest = top * 64 + 63 - units[top].leading_zeros() as usize;
        let sign = u64::from(negative) << 63;
        if highest < 52 {
            return f64::from_bits(sign | units[0]);
        }
        let shift = highest - 52;
        let word = shift / 64;
        let offset = shift % 64;
        let mut mantissa = units[word] >> offset;
        if offset != 0 {
            mantissa |= units.get(word + 1).copied().unwrap_or(0) << (64 - offset);
        }
        if shift != 0 {
            let round_at = shift - 1;
            let round_word = round_at / 64;
            let round_bit = round_at % 64;
            if units[round_word] & (1 << round_bit) != 0 {
                let sticky = units[..round_word].iter().any(|&value| value != 0)
                    || units[round_word] & ((1 << round_bit) - 1) != 0;
                if sticky || mantissa & 1 != 0 {
                    mantissa += 1;
                }
            }
        }
        if mantissa == 1 << 53 {
            mantissa >>= 1;
            highest += 1;
        }
        let exponent = highest - 51;
        if exponent >= 0x7ff {
            return f64::from_bits(sign | 0x7ff0_0000_0000_0000);
        }
        f64::from_bits(sign | (exponent as u64) << 52 | (mantissa & FRACTION))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn removable_sums_match_independent_exact_decimal_vectors() {
        // Generated with Python Decimal.from_float at 1600-digit precision,
        // converting each exact suffix once to binary64. No Python at runtime.
        for line in include_str!("../tests/fixtures/duration_sums.txt").lines() {
            if line.starts_with('#') {
                continue;
            }
            let (input, expected) = line.split_once('|').unwrap();
            let values: Vec<_> = input
                .split_whitespace()
                .map(|s| f64::from_bits(u64::from_str_radix(s, 16).unwrap()))
                .collect();
            let expected: Vec<_> = expected
                .split_whitespace()
                .map(|s| u64::from_str_radix(s, 16).unwrap())
                .collect();
            let mut sum = DurationSum::new(values.iter().copied());
            assert_eq!(sum.value().to_bits(), expected[0], "{line}");
            for (index, value) in values.into_iter().enumerate() {
                sum.remove(value);
                assert_eq!(
                    sum.value().to_bits(),
                    expected[index + 1],
                    "suffix {}: {line}",
                    index + 1
                );
            }
        }
    }
    #[test]
    fn removing_nonfinite_values_restores_finite_tails() {
        let mut sum =
            DurationSum::new([f64::NAN, f64::INFINITY, f64::NEG_INFINITY, 0.25].into_iter());
        assert!(sum.value().is_nan());
        sum.remove(f64::NAN);
        assert!(sum.value().is_nan());
        sum.remove(f64::INFINITY);
        assert_eq!(sum.value(), f64::NEG_INFINITY);
        sum.remove(f64::NEG_INFINITY);
        assert_eq!(sum.value(), 0.25);
        sum.remove(0.25);
        assert_eq!(sum.value(), 0.0);
    }
}
