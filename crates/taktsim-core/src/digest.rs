//! Checksums and hashes in portable code: SHA-256 (FIPS 180-4) for model
//! hashes and trace hashes (ADR-0002, ADR-0004), CRC32 for trace chunks
//! (ADR-0021).
//!
//! Plain scalar Rust without CPU-specific paths or dependencies, so every
//! platform runs the same code; both are exact integer functions, so the
//! result is the same everywhere anyway.
//!
//! ```
//! use taktsim_core::digest::{crc32, sha256, Sha256};
//!
//! let mut hasher = Sha256::new();
//! hasher.update(b"ab");
//! hasher.update(b"c");
//! assert_eq!(hasher.finalize(), sha256(b"abc"));
//! assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
//! ```

/// The SHA-256 of `bytes`.
///
/// ```
/// let digest = taktsim_core::digest::sha256(b"abc");
/// assert_eq!(digest[..4], [0xba, 0x78, 0x16, 0xbf]);
/// ```
#[must_use]
pub fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    hasher.finalize()
}

/// The CRC32 of `bytes`: CRC-32/ISO-HDLC (the CRC of zlib, PNG and
/// Ethernet; reflected polynomial `0xEDB88320`, initial value and final
/// XOR `0xFFFFFFFF`).
///
/// ```
/// assert_eq!(taktsim_core::digest::crc32(b"123456789"), 0xcbf4_3926);
/// assert_eq!(taktsim_core::digest::crc32(b""), 0);
/// ```
#[must_use]
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = Crc32::new();
    crc.update(bytes);
    crc.finalize()
}

/// The initial hash value H(0) (FIPS 180-4, 5.3.3).
const H0: [u32; 8] = [
    0x6a09_e667,
    0xbb67_ae85,
    0x3c6e_f372,
    0xa54f_f53a,
    0x510e_527f,
    0x9b05_688c,
    0x1f83_d9ab,
    0x5be0_cd19,
];

/// The round constants K (FIPS 180-4, 4.2.2).
const K: [u32; 64] = [
    0x428a_2f98,
    0x7137_4491,
    0xb5c0_fbcf,
    0xe9b5_dba5,
    0x3956_c25b,
    0x59f1_11f1,
    0x923f_82a4,
    0xab1c_5ed5,
    0xd807_aa98,
    0x1283_5b01,
    0x2431_85be,
    0x550c_7dc3,
    0x72be_5d74,
    0x80de_b1fe,
    0x9bdc_06a7,
    0xc19b_f174,
    0xe49b_69c1,
    0xefbe_4786,
    0x0fc1_9dc6,
    0x240c_a1cc,
    0x2de9_2c6f,
    0x4a74_84aa,
    0x5cb0_a9dc,
    0x76f9_88da,
    0x983e_5152,
    0xa831_c66d,
    0xb003_27c8,
    0xbf59_7fc7,
    0xc6e0_0bf3,
    0xd5a7_9147,
    0x06ca_6351,
    0x1429_2967,
    0x27b7_0a85,
    0x2e1b_2138,
    0x4d2c_6dfc,
    0x5338_0d13,
    0x650a_7354,
    0x766a_0abb,
    0x81c2_c92e,
    0x9272_2c85,
    0xa2bf_e8a1,
    0xa81a_664b,
    0xc24b_8b70,
    0xc76c_51a3,
    0xd192_e819,
    0xd699_0624,
    0xf40e_3585,
    0x106a_a070,
    0x19a4_c116,
    0x1e37_6c08,
    0x2748_774c,
    0x34b0_bcb5,
    0x391c_0cb3,
    0x4ed8_aa4a,
    0x5b9c_ca4f,
    0x682e_6ff3,
    0x748f_82ee,
    0x78a5_636f,
    0x84c8_7814,
    0x8cc7_0208,
    0x90be_fffa,
    0xa450_6ceb,
    0xbef9_a3f7,
    0xc671_78f2,
];

/// A SHA-256 hash computed from data that arrives in pieces (FIPS 180-4).
///
/// ```
/// use taktsim_core::digest::{sha256, Sha256};
///
/// let mut hasher = Sha256::new();
/// for piece in [&b"hello "[..], b"world"] {
///     hasher.update(piece);
/// }
/// assert_eq!(hasher.finalize(), sha256(b"hello world"));
/// ```
#[derive(Debug, Clone)]
pub struct Sha256 {
    state: [u32; 8],
    /// The bytes of the current, incomplete block.
    block: [u8; 64],
    /// How many bytes of `block` are filled: always below 64.
    filled: usize,
    /// The message length in bits, modulo 2^64 (FIPS 180-4 allows
    /// messages below 2^64 bits).
    bits: u64,
}

impl Default for Sha256 {
    fn default() -> Self {
        Self::new()
    }
}

impl Sha256 {
    /// A hasher that has seen no data.
    ///
    /// ```
    /// use taktsim_core::digest::{sha256, Sha256};
    ///
    /// assert_eq!(Sha256::new().finalize(), sha256(b""));
    /// assert_eq!(Sha256::default().finalize(), sha256(b""));
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: H0,
            block: [0; 64],
            filled: 0,
            bits: 0,
        }
    }

    /// Adds `bytes` to the data hashed.
    ///
    /// ```
    /// let mut hasher = taktsim_core::digest::Sha256::new();
    /// hasher.update(b"a");
    /// hasher.update(b"");
    /// assert_eq!(hasher.finalize(), taktsim_core::digest::sha256(b"a"));
    /// ```
    pub fn update(&mut self, bytes: &[u8]) {
        // usize is at most 64 bits on every supported target.
        self.bits = self.bits.wrapping_add((bytes.len() as u64).wrapping_mul(8));
        // Fill the current block first (it may be empty), then hash whole
        // blocks straight from `bytes`, and keep the rest.
        let take = (64 - self.filled).min(bytes.len());
        self.block[self.filled..self.filled + take].copy_from_slice(&bytes[..take]);
        self.filled += take;
        if self.filled < 64 {
            return;
        }
        compress(&mut self.state, &self.block);
        let (blocks, tail) = bytes[take..].as_chunks::<64>();
        for block in blocks {
            compress(&mut self.state, block);
        }
        self.block[..tail.len()].copy_from_slice(tail);
        self.filled = tail.len();
    }

    /// The hash of all data added (FIPS 180-4, 5.1.1 padding).
    ///
    /// ```
    /// let mut hasher = taktsim_core::digest::Sha256::new();
    /// hasher.update(b"abc");
    /// assert_eq!(hasher.finalize()[31], 0xad);
    /// ```
    #[must_use]
    pub fn finalize(mut self) -> [u8; 32] {
        let bits = self.bits;
        // A 1 bit, zeros up to 56 bytes into a block, the length in 64 bits.
        let zeros = (64 + 55 - self.filled) % 64;
        self.update(&[0x80]);
        self.update(&[0; 64][..zeros]);
        self.update(&bits.to_be_bytes());
        let mut digest = [0; 32];
        for (out, word) in digest.as_chunks_mut::<4>().0.iter_mut().zip(self.state) {
            *out = word.to_be_bytes();
        }
        digest
    }
}

/// The SHA-256 compression function on one 512-bit block (FIPS 180-4,
/// 6.2.2); all additions are modulo 2^32.
fn compress(state: &mut [u32; 8], block: &[u8; 64]) {
    // The message schedule W.
    let mut schedule = [0_u32; 64];
    for (word, bytes) in schedule.iter_mut().zip(block.as_chunks::<4>().0) {
        *word = u32::from_be_bytes(*bytes);
    }
    for t in 16..64 {
        let w15 = schedule[t - 15];
        let w2 = schedule[t - 2];
        let sigma0 = w15.rotate_right(7) ^ w15.rotate_right(18) ^ (w15 >> 3);
        let sigma1 = w2.rotate_right(17) ^ w2.rotate_right(19) ^ (w2 >> 10);
        schedule[t] = schedule[t - 16]
            .wrapping_add(sigma0)
            .wrapping_add(schedule[t - 7])
            .wrapping_add(sigma1);
    }
    // The working variables a to h are vars[0] to vars[7].
    let mut vars = *state;
    for (constant, word) in K.iter().zip(schedule) {
        let big_sigma1 =
            vars[4].rotate_right(6) ^ vars[4].rotate_right(11) ^ vars[4].rotate_right(25);
        // Ch(e, f, g) = (e AND f) XOR (NOT e AND g), and Maj(a, b, c) =
        // (a AND b) XOR (a AND c) XOR (b AND c) (FIPS 180-4, 4.1.2), in
        // equivalent forms where every operator matters, so a changed
        // operator always changes the result.
        let choose = vars[6] ^ (vars[4] & (vars[5] ^ vars[6]));
        let t1 = vars[7]
            .wrapping_add(big_sigma1)
            .wrapping_add(choose)
            .wrapping_add(*constant)
            .wrapping_add(word);
        let big_sigma0 =
            vars[0].rotate_right(2) ^ vars[0].rotate_right(13) ^ vars[0].rotate_right(22);
        let majority = vars[1] ^ ((vars[0] ^ vars[1]) & (vars[1] ^ vars[2]));
        let t2 = big_sigma0.wrapping_add(majority);
        vars = [
            t1.wrapping_add(t2),
            vars[0],
            vars[1],
            vars[2],
            vars[3].wrapping_add(t1),
            vars[4],
            vars[5],
            vars[6],
        ];
    }
    for (hash, var) in state.iter_mut().zip(vars) {
        *hash = hash.wrapping_add(var);
    }
}

/// The reflected CRC-32/ISO-HDLC polynomial.
const POLYNOMIAL: u32 = 0xedb8_8320;

/// The CRC of every byte value, for byte-wise table lookup.
const CRC_TABLE: [u32; 256] = crc_table();

const fn crc_table() -> [u32; 256] {
    let mut table = [0; 256];
    let mut byte: u32 = 0;
    while byte < 256 {
        let mut crc = byte;
        let mut bit = 0;
        while bit < 8 {
            // XOR the polynomial when the low bit is set, without a branch.
            crc = (crc >> 1) ^ (POLYNOMIAL & (crc & 1).wrapping_neg());
            bit += 1;
        }
        table[byte as usize] = crc;
        byte += 1;
    }
    table
}

/// A CRC32 (CRC-32/ISO-HDLC) computed from data that arrives in pieces.
///
/// ```
/// use taktsim_core::digest::{crc32, Crc32};
///
/// let mut crc = Crc32::new();
/// crc.update(b"1234");
/// crc.update(b"56789");
/// assert_eq!(crc.finalize(), crc32(b"123456789"));
/// ```
#[derive(Debug, Clone)]
pub struct Crc32 {
    state: u32,
}

impl Default for Crc32 {
    fn default() -> Self {
        Self::new()
    }
}

impl Crc32 {
    /// A CRC that has seen no data.
    ///
    /// ```
    /// use taktsim_core::digest::Crc32;
    ///
    /// assert_eq!(Crc32::new().finalize(), 0);
    /// assert_eq!(Crc32::default().finalize(), 0);
    /// ```
    #[must_use]
    pub const fn new() -> Self {
        Self { state: u32::MAX }
    }

    /// Adds `bytes` to the data checked.
    ///
    /// ```
    /// let mut crc = taktsim_core::digest::Crc32::new();
    /// crc.update(b"a");
    /// assert_eq!(crc.finalize(), 0xe8b7_be43);
    /// ```
    pub fn update(&mut self, bytes: &[u8]) {
        for &byte in bytes {
            let index = self.state.to_le_bytes()[0] ^ byte;
            self.state = CRC_TABLE[usize::from(index)] ^ (self.state >> 8);
        }
    }

    /// The CRC of all data added.
    ///
    /// ```
    /// let mut crc = taktsim_core::digest::Crc32::new();
    /// crc.update(b"123456789");
    /// assert_eq!(crc.finalize(), 0xcbf4_3926);
    /// ```
    #[must_use]
    pub const fn finalize(self) -> u32 {
        !self.state
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_crc_table_is_the_published_one() {
        // Run the table builder at run time too, so coverage sees it.
        let table = crc_table();
        assert_eq!(table, CRC_TABLE);
        // Entries from the table in RFC 1952 / zlib's crc32.h.
        assert_eq!(table[0], 0);
        assert_eq!(table[1], 0x7707_3096);
        assert_eq!(table[128], 0xedb8_8320);
        assert_eq!(table[255], 0x2d02_ef8d);
    }
}
