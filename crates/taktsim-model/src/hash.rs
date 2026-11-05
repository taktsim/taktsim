//! The two hashes of a model (ADR-0002, ADR-0020 §8): the model hash,
//! SHA-256 of the canonical JSON of the whole normalized model, and the
//! system hash, SHA-256 of the canonical JSON of only its system sections.

use std::fmt;
use std::str::FromStr;

use sha2::{Digest, Sha256};

/// The SHA-256 of a model's canonical form: the same for every spelling of
/// the same model (YAML or JSON, any key order, comments, shorthands,
/// quantity spellings, includes), on every platform.
///
/// Displayed and parsed as 64 lowercase hex digits.
///
/// ```
/// use taktsim_model::ModelHash;
///
/// let hash = ModelHash::of(b"abc");
/// assert_eq!(
///     hash.to_string(),
///     "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
/// );
/// assert_eq!(hash.to_string().parse::<ModelHash>(), Ok(hash));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ModelHash([u8; 32]);

impl ModelHash {
    /// The SHA-256 of `bytes`.
    ///
    /// ```
    /// use taktsim_model::ModelHash;
    ///
    /// assert_eq!(
    ///     ModelHash::of(b"").to_string(),
    ///     "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    /// );
    /// ```
    #[must_use]
    pub fn of(bytes: &[u8]) -> Self {
        Self(Sha256::digest(bytes).into())
    }

    /// The 32 bytes of the hash.
    ///
    /// ```
    /// assert_eq!(taktsim_model::ModelHash::of(b"").as_bytes()[0], 0xe3);
    /// ```
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
}

impl fmt::Display for ModelHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(f, "{byte:02x}")?;
        }
        Ok(())
    }
}

/// The sections of the normalized model that the system hash covers: what
/// the system is. Everything else (`model`, `experiment`, `observe`) is
/// what is asked of it, and only the model hash covers it.
///
/// ```
/// assert_eq!(taktsim_model::SYSTEM_SECTIONS, ["format", "components", "flow"]);
/// ```
pub const SYSTEM_SECTIONS: [&str; 3] = ["format", "components", "flow"];

/// The SHA-256 of a model's system: the canonical JSON (RFC 8785) of the
/// object holding only the [`SYSTEM_SECTIONS`] of the normalized model
/// (ADR-0020 §8; owner's decision, 2026-11-04).
///
/// The model hash ([`ModelHash`]) answers "is this exactly the run I can
/// redo?"; the system hash answers "is this the same system?". Changing the
/// model's name, seed or time unit, its experiment or its observables
/// changes the model hash and keeps the system hash; changing a component
/// or an edge changes both. Like the model hash, it is the same for every
/// spelling and on every platform. Displayed and parsed as 64 lowercase
/// hex digits.
///
/// ```
/// use taktsim_model::SystemHash;
///
/// let text = "18c3cbaad4e836fd4c29a17ba0a88ff553dfb516a69886db10ed2efb09d89904";
/// let hash: SystemHash = text.parse()?;
/// assert_eq!(hash.to_string(), text);
/// # Ok::<(), taktsim_model::ParseHashError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct SystemHash(ModelHash);

impl SystemHash {
    /// The SHA-256 of the system's canonical JSON.
    pub(crate) fn of(canonical: &[u8]) -> Self {
        Self(ModelHash::of(canonical))
    }

    /// The 32 bytes of the hash.
    ///
    /// ```
    /// use taktsim_model::SystemHash;
    ///
    /// let text = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
    /// assert_eq!(text.parse::<SystemHash>()?.as_bytes()[0], 0xe3);
    /// # Ok::<(), taktsim_model::ParseHashError>(())
    /// ```
    #[must_use]
    pub const fn as_bytes(&self) -> &[u8; 32] {
        self.0.as_bytes()
    }
}

impl fmt::Display for SystemHash {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

impl FromStr for SystemHash {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.parse().map(Self)
    }
}

/// Text that is not 64 lowercase hex digits.
///
/// ```
/// use taktsim_model::{ModelHash, ParseHashError, SystemHash};
///
/// assert_eq!("abc".parse::<ModelHash>(), Err(ParseHashError));
/// assert_eq!("abc".parse::<SystemHash>(), Err(ParseHashError));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a hash is 64 lowercase hex digits")]
pub struct ParseHashError;

impl FromStr for ModelHash {
    type Err = ParseHashError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        let bytes = s.as_bytes();
        if bytes.len() != 64 {
            return Err(ParseHashError);
        }
        let mut out = [0_u8; 32];
        for (i, pair) in bytes.as_chunks::<2>().0.iter().enumerate() {
            let digit = |b: u8| match b {
                b'0'..=b'9' => Ok(b - b'0'),
                b'a'..=b'f' => Ok(b - b'a' + 10),
                _ => Err(ParseHashError),
            };
            out[i] = digit(pair[0])? << 4 | digit(pair[1])?;
        }
        Ok(Self(out))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parsing_rejects_anything_but_lowercase_hex() {
        let good = "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855";
        assert!(good.parse::<ModelHash>().is_ok());
        assert!(good.to_uppercase().parse::<ModelHash>().is_err());
        assert!(good[..63].parse::<ModelHash>().is_err());
        assert!(format!("{}g", &good[..63]).parse::<ModelHash>().is_err());
        assert!(format!("g{}", &good[1..]).parse::<ModelHash>().is_err());
    }
}
