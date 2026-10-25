//! Cross-platform golden values (spec §9, acceptance criterion A4): the
//! same seed gives the same samples, bit for bit, on every supported
//! platform. CI runs this on Windows x86-64, Linux x86-64 and ARM64 and
//! macOS ARM64; the constants were recorded once and must never change
//! without a deliberate, documented engine change.

use taktsim_core::random::{DurationDistribution, Stream, StreamKey};
use taktsim_units::SimDuration;

const DRAWS: usize = 10_000;

/// FNV-1a (64 bit) over the little-endian bytes of `values`.
fn digest(values: impl Iterator<Item = u64>) -> u64 {
    values.fold(0xcbf2_9ce4_8422_2325, |hash, v| {
        v.to_le_bytes().iter().fold(hash, |h, &b| {
            (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3)
        })
    })
}

#[allow(clippy::unwrap_used)] // test helper
fn d(text: &str) -> SimDuration {
    text.parse().unwrap()
}

#[allow(clippy::unwrap_used)] // test helper
fn sample_digest(dist: &DurationDistribution, purpose: &str) -> u64 {
    let mut stream = Stream::new(42, StreamKey::new("golden", purpose));
    digest((0..DRAWS).map(|_| dist.sample(&mut stream).unwrap().as_nanos()))
}

#[test]
fn raw_stream_values_are_identical_everywhere() {
    let mut stream = Stream::new(42, StreamKey::new("golden", "raw"));
    assert_eq!(digest((0..DRAWS).map(|_| stream.next_u64())), RAW);
    let mut stream = Stream::new(42, StreamKey::new("golden", "open01"));
    assert_eq!(
        digest((0..DRAWS).map(|_| stream.next_open01().to_bits())),
        OPEN01
    );
}

#[test]
fn samples_are_identical_everywhere() {
    let cases = [
        (
            DurationDistribution::exponential(d("12min")).unwrap(),
            "exponential",
            EXPONENTIAL,
        ),
        (
            DurationDistribution::normal(d("1s"), d("2s")).unwrap(),
            "normal",
            NORMAL,
        ),
        (
            DurationDistribution::lognormal(d("42s"), 0.08).unwrap(),
            "lognormal",
            LOGNORMAL,
        ),
        (
            DurationDistribution::uniform(d("40s"), d("50s")).unwrap(),
            "uniform",
            UNIFORM,
        ),
        (
            DurationDistribution::triangular(d("40s"), d("42s"), d("50s")).unwrap(),
            "triangular",
            TRIANGULAR,
        ),
        (
            DurationDistribution::gamma(d("12min"), 0.5).unwrap(),
            "gamma",
            GAMMA,
        ),
        (
            DurationDistribution::gamma(d("12min"), 2.0).unwrap(),
            "gamma_boost",
            GAMMA_BOOST,
        ),
        // cv 1 is shape 1, the boundary between the two gamma methods.
        (
            DurationDistribution::gamma(d("12min"), 1.0).unwrap(),
            "gamma_shape1",
            GAMMA_SHAPE1,
        ),
        (
            DurationDistribution::erlang(d("12min"), 3).unwrap(),
            "erlang",
            ERLANG,
        ),
        (
            DurationDistribution::weibull(d("6h"), 1.5).unwrap(),
            "weibull",
            WEIBULL,
        ),
    ];
    let mismatches: Vec<String> = cases
        .iter()
        .filter_map(|(dist, purpose, expected)| {
            let actual = sample_digest(dist, purpose);
            (actual != *expected)
                .then(|| format!("{purpose}: {actual:#018x}, expected {expected:#018x}"))
        })
        .collect();
    assert!(mismatches.is_empty(), "{mismatches:#?}");
}

const RAW: u64 = 0x6bae_57eb_1ab6_c756;
const OPEN01: u64 = 0x64ad_9744_a2f7_5803;
const EXPONENTIAL: u64 = 0x929e_142a_d743_137f;
const NORMAL: u64 = 0x3cc8_3721_2b20_cd87;
const LOGNORMAL: u64 = 0x65ca_35e0_77eb_e45a;
const UNIFORM: u64 = 0x0182_5d5a_8332_7e68;
const TRIANGULAR: u64 = 0xfe02_29f7_b506_1a36;
const GAMMA: u64 = 0x81ca_ebf1_0d49_0776;
const GAMMA_SHAPE1: u64 = 0xbe6c_6933_db08_204d;
const GAMMA_BOOST: u64 = 0x1578_7e00_e2c4_7d99;
const ERLANG: u64 = 0xcd81_0733_ac7e_3d9b;
const WEIBULL: u64 = 0xce30_556d_3549_9fc3;
