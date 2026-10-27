//! SHA-256 and CRC32 against published values (ADR-0021).
//!
//! SHA-256 test data, unchanged apart from line endings:
//! - `tests/data/SHA256ShortMsg.rsp`: all 65 vectors (0 to 64 bytes) of
//!   that file from NIST CAVP `shabytetestvectors.zip` (CAVS 11.0);
//! - `tests/data/SHA256LongMsg-first8.rsp`: the first 8 of the 64 vectors
//!   (163 to 856 bytes) of `SHA256LongMsg.rsp` from the same archive;
//! - the examples of FIPS 180-4 (NIST "SHA256.pdf", "SHA256 2-block",
//!   one million `a`).
//!
//! Padding edge cases were computed with Python's `hashlib`; CRC32 values
//! are the CRC-32/ISO-HDLC check value and values of zlib's `crc32`.

use proptest::prelude::*;
use taktsim_core::digest::{Crc32, Sha256, crc32, sha256};

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    bytes.iter().fold(String::new(), |mut text, b| {
        let _ = write!(text, "{b:02x}");
        text
    })
}

#[allow(clippy::unwrap_used)] // test helper
fn unhex(text: &str) -> Vec<u8> {
    (0..text.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&text[i..i + 2], 16).unwrap())
        .collect()
}

/// The (message, digest) pairs of a CAVP `.rsp` file. `Len` is in bits;
/// a zero-length message is written as `Msg = 00`.
#[allow(clippy::unwrap_used)] // test helper
fn cavp(text: &str) -> Vec<(Vec<u8>, String)> {
    let mut vectors = Vec::new();
    let mut lines = text.lines().map(str::trim);
    while let Some(line) = lines.next() {
        let Some(bits) = line.strip_prefix("Len = ") else {
            continue;
        };
        let bits: usize = bits.parse().unwrap();
        let msg = lines.next().unwrap().strip_prefix("Msg = ").unwrap();
        let md = lines.next().unwrap().strip_prefix("MD = ").unwrap();
        let mut message = unhex(msg);
        message.truncate(bits / 8);
        vectors.push((message, md.to_owned()));
    }
    vectors
}

#[test]
fn nist_cavp_short_messages_of_every_length_from_0_to_64_bytes() {
    let vectors = cavp(include_str!("data/SHA256ShortMsg.rsp"));
    let lengths: Vec<usize> = vectors.iter().map(|(m, _)| m.len()).collect();
    assert_eq!(lengths, (0..=64).collect::<Vec<_>>());
    for (message, digest) in vectors {
        assert_eq!(hex(&sha256(&message)), digest, "{} bytes", message.len());
    }
}

#[test]
fn nist_cavp_long_messages() {
    let vectors = cavp(include_str!("data/SHA256LongMsg-first8.rsp"));
    assert_eq!(vectors.len(), 8);
    for (message, digest) in vectors {
        assert_eq!(hex(&sha256(&message)), digest, "{} bytes", message.len());
        // The same, fed in uneven pieces.
        let mut hasher = Sha256::new();
        for piece in message.chunks(37) {
            hasher.update(piece);
        }
        assert_eq!(hex(&hasher.finalize()), digest);
    }
}

#[test]
fn fips_180_4_examples() {
    let cases: [(&[u8], &str); 4] = [
        (
            b"",
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855",
        ),
        (
            b"abc",
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
        ),
        // 448 bits.
        (
            b"abcdbcdecdefdefgefghfghighijhijkijkljklmklmnlmnomnopnopq",
            "248d6a61d20638b8e5c026930c3e6039a33ce45964ff2167f6ecedd419db06c1",
        ),
        // 896 bits.
        (
            b"abcdefghbcdefghicdefghijdefghijkefghijklfghijklmghijklmnhijklmnoijklmnopjklmnopqklmnopqrlmnopqrsmnopqrstnopqrstu",
            "cf5b16a778af8380036ce59e7b0492370b249b11e8f07a51afac45037afee9d1",
        ),
    ];
    for (message, digest) in cases {
        assert_eq!(hex(&sha256(message)), digest);
    }
    let million = vec![b'a'; 1_000_000];
    assert_eq!(
        hex(&sha256(&million)),
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

#[test]
fn padding_edge_cases() {
    // 55 bytes: padding fits the block; 56 to 63: it needs a second block;
    // 64, 119 and 120: the same around the second block.
    let cases = [
        (
            55,
            "9f4390f8d30c2dd92ec9f095b65e2b9ae9b0a925a5258e241c9f1e910f734318",
        ),
        (
            56,
            "b35439a4ac6f0948b6d6f9e3c6af0f5f590ce20f1bde7090ef7970686ec6738a",
        ),
        (
            63,
            "7d3e74a05d7db15bce4ad9ec0658ea98e3f06eeecf16b4c6fff2da457ddc2f34",
        ),
        (
            64,
            "ffe054fe7ae0cb6dc65c3af9b61d5209f439851db43d0ba5997337df154668eb",
        ),
        (
            65,
            "635361c48bb9eab14198e76ea8ab7f1a41685d6ad62aa9146d301d4f17eb0ae0",
        ),
        (
            119,
            "31eba51c313a5c08226adf18d4a359cfdfd8d2e816b13f4af952f7ea6584dcfb",
        ),
        (
            120,
            "2f3d335432c70b580af0e8e1b3674a7c020d683aa5f73aaaedfdc55af904c21c",
        ),
        (
            127,
            "c57e9278af78fa3cab38667bef4ce29d783787a2f731d4e12200270f0c32320a",
        ),
        (
            128,
            "6836cf13bac400e9105071cd6af47084dfacad4e5e302c94bfed24e013afb73e",
        ),
    ];
    for (n, digest) in cases {
        let message = vec![b'a'; n];
        assert_eq!(hex(&sha256(&message)), digest, "{n} bytes");
        // Byte by byte, so every padding path starts from a partial block.
        let mut hasher = Sha256::default();
        for byte in &message {
            hasher.update(std::slice::from_ref(byte));
        }
        assert_eq!(hex(&hasher.finalize()), digest, "{n} bytes, byte by byte");
    }
}

#[test]
fn crc32_check_values() {
    assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    assert_eq!(crc32(b""), 0);
    assert_eq!(crc32(b"a"), 0xe8b7_be43);
    assert_eq!(
        crc32(b"The quick brown fox jumps over the lazy dog"),
        0x414f_a339
    );
    let all: Vec<u8> = (0..=255).collect();
    assert_eq!(crc32(&all), 0x2905_8c73);
    assert_eq!(Crc32::default().finalize(), 0);
}

#[test]
fn hashers_can_be_cloned_midway() {
    let mut a = Sha256::new();
    a.update(b"common prefix, then ");
    let mut b = a.clone();
    a.update(b"one");
    b.update(b"one");
    assert_eq!(a.finalize(), b.finalize());
    let mut c = Crc32::new();
    c.update(b"1234");
    let mut d = c.clone();
    d.update(b"56789");
    assert_eq!(d.finalize(), 0xcbf4_3926);
    assert!(format!("{c:?}").starts_with("Crc32"));
    assert!(format!("{:?}", Sha256::new()).starts_with("Sha256"));
}

/// `data` cut at the sorted positions `cuts`.
fn pieces<'a>(data: &'a [u8], cuts: &[prop::sample::Index]) -> Vec<&'a [u8]> {
    let mut at: Vec<usize> = cuts.iter().map(|c| c.index(data.len() + 1)).collect();
    at.sort_unstable();
    let mut out = Vec::new();
    let mut start = 0;
    for end in at {
        out.push(&data[start..end]);
        start = end;
    }
    out.push(&data[start..]);
    out
}

proptest! {
    #[test]
    fn any_split_gives_the_one_shot_sha256(
        data in prop::collection::vec(any::<u8>(), 0..600),
        cuts in prop::collection::vec(any::<prop::sample::Index>(), 0..12),
    ) {
        let mut hasher = Sha256::new();
        for piece in pieces(&data, &cuts) {
            hasher.update(piece);
        }
        prop_assert_eq!(hasher.finalize(), sha256(&data));
    }

    #[test]
    fn any_split_gives_the_one_shot_crc32(
        data in prop::collection::vec(any::<u8>(), 0..600),
        cuts in prop::collection::vec(any::<prop::sample::Index>(), 0..12),
    ) {
        let mut crc = Crc32::new();
        for piece in pieces(&data, &cuts) {
            crc.update(piece);
        }
        prop_assert_eq!(crc.finalize(), crc32(&data));
    }
}
