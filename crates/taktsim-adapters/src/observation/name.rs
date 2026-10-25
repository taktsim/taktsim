//! Validated names for the `entity` and `signal` columns.

use std::borrow::Borrow;
use std::fmt;
use std::str::FromStr;

/// The name of an entity (a machine, a buffer, a line) or of a signal (a
/// counter, a state, a cycle time), as the source calls it.
///
/// A name is 1 to [`Name::MAX_BYTES`] bytes of UTF-8 without control
/// characters (so no line breaks or tabs) and without leading or trailing
/// whitespace. Anything else is allowed, e.g. `Press 1`, `Fräse_2` or an
/// OPC UA node id such as `ns=2;s=Line1.Press.Counter`, because names are
/// matched to model components later, not here. Names compare by their
/// bytes, which is also the order of Unicode code points.
///
/// ```
/// use taktsim_adapters::observation::{Name, NameError};
///
/// let entity = Name::new("Press 1")?;
/// assert_eq!(entity.as_str(), "Press 1");
/// assert_eq!(Name::new(" M1"), Err(NameError::Whitespace));
/// assert_eq!("M1\tM2".parse::<Name>(), Err(NameError::ControlCharacter { position: 3 }));
/// # Ok::<(), NameError>(())
/// ```
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Name(Box<str>);

impl Name {
    /// The longest name, in bytes of UTF-8.
    ///
    /// ```
    /// use taktsim_adapters::observation::Name;
    ///
    /// assert!(Name::new(&"x".repeat(Name::MAX_BYTES)).is_ok());
    /// assert!(Name::new(&"x".repeat(Name::MAX_BYTES + 1)).is_err());
    /// ```
    pub const MAX_BYTES: usize = 256;

    /// The name `text`, if it is valid.
    ///
    /// # Errors
    ///
    /// A [`NameError`] if `text` is empty, too long, has a control
    /// character, or starts or ends with whitespace.
    ///
    /// ```
    /// use taktsim_adapters::observation::{Name, NameError};
    ///
    /// assert_eq!(Name::new("good_count")?.to_string(), "good_count");
    /// assert_eq!(Name::new(""), Err(NameError::Empty));
    /// # Ok::<(), NameError>(())
    /// ```
    pub fn new(text: &str) -> Result<Self, NameError> {
        validate(text)?;
        Ok(Self(text.into()))
    }

    /// A name already checked by [`validate`].
    pub(crate) fn new_unchecked(text: &str) -> Self {
        Self(text.into())
    }

    /// The name as text.
    ///
    /// ```
    /// use taktsim_adapters::observation::Name;
    ///
    /// assert_eq!(Name::new("M1")?.as_str(), "M1");
    /// # Ok::<(), taktsim_adapters::observation::NameError>(())
    /// ```
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Checks the rules of [`Name`] without allocating.
pub(crate) fn validate(text: &str) -> Result<(), NameError> {
    if text.is_empty() {
        return Err(NameError::Empty);
    }
    if text.len() > Name::MAX_BYTES {
        return Err(NameError::TooLong { bytes: text.len() });
    }
    if let Some(index) = text.chars().position(char::is_control) {
        return Err(NameError::ControlCharacter {
            position: index + 1,
        });
    }
    if text.starts_with(char::is_whitespace) || text.ends_with(char::is_whitespace) {
        return Err(NameError::Whitespace);
    }
    Ok(())
}

impl FromStr for Name {
    type Err = NameError;

    /// [`Name::new`].
    fn from_str(text: &str) -> Result<Self, NameError> {
        Self::new(text)
    }
}

impl fmt::Display for Name {
    /// The name as it is, without quotes.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl AsRef<str> for Name {
    fn as_ref(&self) -> &str {
        &self.0
    }
}

impl Borrow<str> for Name {
    fn borrow(&self) -> &str {
        &self.0
    }
}

/// Why a text is not a valid [`Name`].
///
/// ```
/// use taktsim_adapters::observation::{Name, NameError};
///
/// let err = Name::new("M1 ").unwrap_err();
/// assert_eq!(err.to_string(), "the name starts or ends with whitespace");
/// assert_eq!(NameError::EXAMPLE, "M1");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum NameError {
    /// The text is empty.
    #[error("the name is empty")]
    Empty,
    /// The text is longer than [`Name::MAX_BYTES`].
    #[error("the name is {bytes} bytes long; the limit is 256")]
    TooLong {
        /// The length found, in bytes of UTF-8.
        bytes: usize,
    },
    /// The text has a control character such as a tab or a line break.
    #[error("the name has a control character at character {position}")]
    ControlCharacter {
        /// 1-based character position of the first one.
        position: usize,
    },
    /// The text starts or ends with whitespace.
    #[error("the name starts or ends with whitespace")]
    Whitespace,
}

impl NameError {
    /// What a name may be.
    ///
    /// ```
    /// use taktsim_adapters::observation::NameError;
    ///
    /// assert!(NameError::ALLOWED.contains("256 bytes"));
    /// ```
    pub const ALLOWED: &'static str = "1 to 256 bytes of UTF-8 text without control characters and \
         without leading or trailing whitespace";

    /// One valid name.
    ///
    /// ```
    /// use taktsim_adapters::observation::{Name, NameError};
    ///
    /// assert!(Name::new(NameError::EXAMPLE).is_ok());
    /// ```
    pub const EXAMPLE: &'static str = "M1";
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valid_names() {
        for text in [
            "M1",
            "Press 1",
            "Fräse_2",
            "ns=2;s=Line1.Press.Counter",
            "a,b",
            "\"q\"",
            "工位",
        ] {
            assert_eq!(Name::new(text).unwrap().as_str(), text);
        }
        let longest = "é".repeat(128);
        assert_eq!(Name::new(&longest).unwrap().as_str().len(), 256);
    }

    #[test]
    fn invalid_names() {
        assert_eq!(Name::new(""), Err(NameError::Empty));
        assert_eq!(
            Name::new(&"x".repeat(257)),
            Err(NameError::TooLong { bytes: 257 })
        );
        assert_eq!(
            Name::new(&"é".repeat(129)),
            Err(NameError::TooLong { bytes: 258 })
        );
        assert_eq!(
            Name::new("a\nb"),
            Err(NameError::ControlCharacter { position: 2 })
        );
        assert_eq!(
            Name::new("é\r"),
            Err(NameError::ControlCharacter { position: 2 })
        );
        assert_eq!(
            Name::new("\u{7f}"),
            Err(NameError::ControlCharacter { position: 1 })
        );
        assert_eq!(Name::new(" M1"), Err(NameError::Whitespace));
        assert_eq!(Name::new("M1\u{a0}"), Err(NameError::Whitespace));
        assert_eq!(Name::new(" "), Err(NameError::Whitespace));
    }

    #[test]
    fn display_and_traits() {
        let name: Name = "M1".parse().unwrap();
        assert_eq!(name.to_string(), "M1");
        assert_eq!(AsRef::<str>::as_ref(&name), "M1");
        assert_eq!(Borrow::<str>::borrow(&name), "M1");
        assert!(Name::new("M10").unwrap() < Name::new("M2").unwrap());
        assert_eq!(
            Name::new(&"x".repeat(300)).unwrap_err().to_string(),
            "the name is 300 bytes long; the limit is 256"
        );
        assert_eq!(
            Name::new("a\tb").unwrap_err().to_string(),
            "the name has a control character at character 2"
        );
        assert_eq!(Name::new("").unwrap_err().to_string(), "the name is empty");
    }
}
