//! The Philox4x64-10 counter-based generator (Salmon et al., "Parallel
//! random numbers: as easy as 1, 2, 3", SC 2011), as in Random123 and
//! `numpy.random.Philox`.

const M0: u64 = 0xD2E7_470E_E14C_6C93;
const M1: u64 = 0xCA5A_8263_9512_1157;
const W0: u64 = 0x9E37_79B9_7F4A_7C15;
const W1: u64 = 0xBB67_AE85_84CA_A73B;

/// The high and low 64 bits of `a × b`.
fn mul_hi_lo(a: u64, b: u64) -> (u64, u64) {
    let product = u128::from(a) * u128::from(b);
    // The high half of a u128 always fits a u64; the low half is the
    // intended truncation.
    #[allow(clippy::cast_possible_truncation)]
    ((product >> 64) as u64, product as u64)
}

/// Ten Philox rounds: four 64-bit outputs for one counter and key.
pub(crate) fn philox4x64_10(counter: [u64; 4], key: [u64; 2]) -> [u64; 4] {
    let [mut c0, mut c1, mut c2, mut c3] = counter;
    let [mut k0, mut k1] = key;
    for round in 0..10 {
        if round > 0 {
            k0 = k0.wrapping_add(W0);
            k1 = k1.wrapping_add(W1);
        }
        let (hi0, lo0) = mul_hi_lo(M0, c0);
        let (hi1, lo1) = mul_hi_lo(M1, c2);
        (c0, c1, c2, c3) = (hi1 ^ c1 ^ k0, lo1, hi0 ^ c3 ^ k1, lo0);
    }
    [c0, c1, c2, c3]
}

#[cfg(test)]
mod tests {
    use super::philox4x64_10;

    // Known-answer vectors of Random123 (kat_vectors, philox4x64 10),
    // confirmed against numpy.random.Philox 2.5.4.
    #[test]
    fn matches_random123_known_answers() {
        assert_eq!(
            philox4x64_10([0; 4], [0; 2]),
            [
                0x1655_4d9e_ca36_314c,
                0xdb20_fe9d_672d_0fdc,
                0xd7e7_72ce_e186_176b,
                0x7e68_b68a_ec7b_a23b
            ]
        );
        assert_eq!(
            philox4x64_10([u64::MAX; 4], [u64::MAX; 2]),
            [
                0x87b0_92c3_013f_e90b,
                0x438c_3c67_be8d_0224,
                0x9cc7_d7c6_9cd7_77b6,
                0xa09c_aebf_594f_0ba0
            ]
        );
        assert_eq!(
            philox4x64_10(
                [
                    0x243f_6a88_85a3_08d3,
                    0x1319_8a2e_0370_7344,
                    0xa409_3822_299f_31d0,
                    0x082e_fa98_ec4e_6c89
                ],
                [0x4528_21e6_38d0_1377, 0xbe54_66cf_34e9_0c6c]
            ),
            [
                0xa528_f454_03e6_1d95,
                0x38c7_2dbd_566e_9788,
                0xa5a1_610e_72fd_18b5,
                0x57bd_43b5_e52b_7fe6
            ]
        );
    }

    // From numpy.random.Philox(key=1 | 2 << 64) at counter 1.
    #[test]
    fn matches_numpy_with_a_nonzero_key_and_counter() {
        assert_eq!(
            philox4x64_10([1, 0, 0, 0], [1, 2]),
            [
                0x4f2f_4313_b553_6b09,
                0x5b61_7be3_219f_f32a,
                0x0972_9347_6f92_75cb,
                0xf63f_3bf4_962c_3942
            ]
        );
    }
}
