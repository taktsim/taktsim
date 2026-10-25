//! Independent random streams, one per component and purpose.

use std::f64::consts::TAU;

use libm::{cos, log, sin};

use super::philox::philox4x64_10;

/// Identifies one random stream within a run: a component and what the
/// stream is for, e.g. `("cut", "cycle_time")`.
///
/// The key is derived from the names, not from positions, so adding a
/// component to a model leaves every other stream unchanged (spec §9).
/// Separate purposes keep, say, cycle times identical when only a failure
/// parameter changes, which makes design comparisons sharper.
///
/// ```
/// use taktsim_core::random::StreamKey;
///
/// let a = StreamKey::new("cut", "cycle_time");
/// assert_eq!(a, StreamKey::new("cut", "cycle_time"));
/// assert_ne!(a, StreamKey::new("cut", "failures"));
/// assert_ne!(a, StreamKey::new("weld", "cycle_time"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct StreamKey(u64);

impl StreamKey {
    /// The key for `purpose` of `component`: FNV-1a (64 bit) over the
    /// component id, a `0xff` separator and the purpose. `0xff` never occurs
    /// in UTF-8, so `("ab", "c")` and `("a", "bc")` differ.
    ///
    /// ```
    /// use taktsim_core::random::StreamKey;
    ///
    /// assert_ne!(StreamKey::new("ab", "c"), StreamKey::new("a", "bc"));
    /// ```
    #[must_use]
    pub fn new(component: &str, purpose: &str) -> Self {
        const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
        const PRIME: u64 = 0x0000_0100_0000_01b3;
        let bytes = component
            .bytes()
            .chain(std::iter::once(0xff))
            .chain(purpose.bytes());
        Self(bytes.fold(OFFSET, |hash, b| (hash ^ u64::from(b)).wrapping_mul(PRIME)))
    }

    /// The key as a number, e.g. for traces.
    ///
    /// ```
    /// use taktsim_core::random::StreamKey;
    ///
    /// // FNV-1a of the single byte 0xff.
    /// assert_eq!(StreamKey::new("", "").as_u64(), 0xaf64_724c_8602_eb6e);
    /// ```
    #[must_use]
    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// A deterministic random stream: Philox4x64-10 keyed by the run's seed and
/// a [`StreamKey`] (ADR-0013).
///
/// The values depend only on the seed, the key and the sequence of calls on
/// this stream, never on other streams or on the order in which streams are
/// used.
///
/// ```
/// use taktsim_core::random::{Stream, StreamKey};
///
/// let key = StreamKey::new("cut", "cycle_time");
/// let mut a = Stream::new(42, key);
/// let mut b = Stream::new(42, key);
/// assert_eq!(a.next_u64(), b.next_u64());
/// let u = a.next_open01();
/// assert!(u > 0.0 && u < 1.0);
/// ```
#[derive(Debug, Clone, PartialEq)]
pub struct Stream {
    key: [u64; 2],
    /// The counter of the next Philox block to generate.
    next_block: u64,
    buffer: [u64; 4],
    /// How many values of `buffer` have been used.
    used: usize,
    /// The second value of the last Box–Muller pair, if not yet used.
    spare_normal: Option<f64>,
}

impl Stream {
    /// The stream `key` for a run with `seed`.
    ///
    /// ```
    /// use taktsim_core::random::{Stream, StreamKey};
    ///
    /// let key = StreamKey::new("cut", "cycle_time");
    /// assert_ne!(Stream::new(1, key).next_u64(), Stream::new(2, key).next_u64());
    /// ```
    #[must_use]
    pub fn new(seed: u64, key: StreamKey) -> Self {
        Self {
            key: [seed, key.0],
            next_block: 0,
            buffer: [0; 4],
            used: 4,
            spare_normal: None,
        }
    }

    /// The next 64 random bits.
    ///
    /// ```
    /// use taktsim_core::random::{Stream, StreamKey};
    ///
    /// let mut s = Stream::new(0, StreamKey::new("a", "b"));
    /// assert_ne!(s.next_u64(), s.next_u64());
    /// ```
    pub fn next_u64(&mut self) -> u64 {
        if self.used == 4 {
            self.buffer = philox4x64_10([self.next_block, 0, 0, 0], self.key);
            // 2^64 blocks of four values cannot be used up in a run.
            self.next_block = self.next_block.wrapping_add(1);
            self.used = 0;
        }
        let value = self.buffer[self.used];
        self.used += 1;
        value
    }

    /// A uniform value in the open interval (0, 1), with 52 random bits:
    /// `(m + ½) × 2⁻⁵²` for a random 52-bit `m`. Never 0 or 1, so its
    /// logarithm is always finite.
    ///
    /// ```
    /// use taktsim_core::random::{Stream, StreamKey};
    ///
    /// let mut s = Stream::new(7, StreamKey::new("src", "arrivals"));
    /// assert!((0..1000).map(|_| s.next_open01()).all(|u| u > 0.0 && u < 1.0));
    /// ```
    pub fn next_open01(&mut self) -> f64 {
        open01_from_bits(self.next_u64())
    }

    /// A standard normal value (mean 0, standard deviation 1).
    ///
    /// Box–Muller: each pair of uniforms gives two independent normals; the
    /// second is kept and returned by the next call.
    ///
    /// ```
    /// use taktsim_core::random::{Stream, StreamKey};
    ///
    /// let mut s = Stream::new(5, StreamKey::new("cut", "noise"));
    /// let (a, b) = (s.next_standard_normal(), s.next_standard_normal());
    /// assert!(a.is_finite() && b.is_finite() && a != b);
    /// ```
    pub fn next_standard_normal(&mut self) -> f64 {
        if let Some(z) = self.spare_normal.take() {
            return z;
        }
        let radius = (-2.0 * log(self.next_open01())).sqrt();
        let angle = TAU * self.next_open01();
        self.spare_normal = Some(radius * sin(angle));
        radius * cos(angle)
    }
}

/// `(m + ½) × 2⁻⁵²` for the top 52 bits `m` of `bits`.
fn open01_from_bits(bits: u64) -> f64 {
    // 1 + m × 2^-52 lies in [1, 2); subtracting 1 - 2^-53 is exact
    // (Sterbenz lemma) and leaves (m + 1/2) × 2^-52.
    let one_plus = f64::from_bits(0x3ff0_0000_0000_0000 | (bits >> 12));
    one_plus - (1.0 - f64::EPSILON / 2.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_are_philox_blocks_in_order() {
        let key = StreamKey::new("a", "b");
        let mut s = Stream::new(9, key);
        let first = philox4x64_10([0, 0, 0, 0], [9, key.0]);
        let second = philox4x64_10([1, 0, 0, 0], [9, key.0]);
        let drawn: Vec<u64> = (0..8).map(|_| s.next_u64()).collect();
        assert_eq!(drawn[..4], first);
        assert_eq!(drawn[4..], second);
    }

    #[test]
    fn open01_covers_its_extremes() {
        assert_eq!(open01_from_bits(0), f64::EPSILON / 2.0);
        assert_eq!(open01_from_bits(u64::MAX), 1.0 - f64::EPSILON / 2.0);
        // The low 12 bits are dropped.
        assert_eq!(open01_from_bits(0xfff), f64::EPSILON / 2.0);
        assert_eq!(open01_from_bits(1 << 12), 1.5 * f64::EPSILON);
    }

    #[test]
    fn normals_use_both_values_of_each_pair() {
        let key = StreamKey::new("a", "normal");
        let mut s = Stream::new(1, key);
        let mut uniforms = Stream::new(1, key);
        let (u1, u2) = (uniforms.next_open01(), uniforms.next_open01());
        let radius = (-2.0 * log(u1)).sqrt();
        assert_eq!(s.next_standard_normal(), radius * cos(TAU * u2));
        assert_eq!(s.next_standard_normal(), radius * sin(TAU * u2));
        // The pair used exactly two uniforms.
        assert_eq!(s.next_open01(), uniforms.next_open01());
    }

    #[test]
    fn keys_hash_known_values() {
        // FNV-1a of b"cut\xffcycle_time", computed independently in Python.
        assert_eq!(
            StreamKey::new("cut", "cycle_time").as_u64(),
            0xa404_bae2_f242_abfe
        );
    }
}
