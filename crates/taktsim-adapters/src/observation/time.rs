//! UTC instants and the UTC offsets their sources wrote them in (RFC 3339).
//!
//! Parsing and formatting are exact integer arithmetic on the proleptic
//! Gregorian calendar, with no time-zone database and no platform calls, so
//! the same text gives the same instant on every platform (ADR-0017).

use std::fmt;
use std::str::FromStr;

const NANOS_PER_SEC: i64 = 1_000_000_000;
const SECS_PER_DAY: i64 = 86_400;
/// Days from 0000-03-01 to 1970-01-01, proleptic Gregorian.
const DAYS_TO_UNIX_EPOCH: i64 = 719_468;
/// Days in one 400-year cycle of the Gregorian calendar.
const DAYS_PER_ERA: i64 = 146_097;
/// The largest offset, 23:59, in minutes (RFC 3339 `time-numoffset`).
const MAX_OFFSET_MINUTES: i16 = 23 * 60 + 59;

/// An exact instant in UTC: whole nanoseconds since the Unix epoch,
/// 1970-01-01T00:00:00Z, without leap seconds, as Unix time counts.
///
/// An `i64` covers 1677-09-21T00:12:43.145224192Z to
/// 2262-04-11T23:47:16.854775807Z. [`Display`](fmt::Display) writes RFC 3339
/// in UTC; [`FromStr`] reads RFC 3339 with an offset and drops the offset
/// (use [`Timestamp::parse_rfc3339`] to keep it).
///
/// ```
/// use taktsim_adapters::observation::Timestamp;
///
/// let t: Timestamp = "2026-03-02T06:00:00.25+01:00".parse()?;
/// assert_eq!(t.as_unix_nanos(), 1_772_427_600_250_000_000);
/// assert_eq!(t.to_string(), "2026-03-02T05:00:00.250Z");
/// # Ok::<(), taktsim_adapters::observation::TimestampError>(())
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    /// 1970-01-01T00:00:00Z.
    ///
    /// ```
    /// use taktsim_adapters::observation::Timestamp;
    ///
    /// assert_eq!(Timestamp::UNIX_EPOCH.to_string(), "1970-01-01T00:00:00Z");
    /// ```
    pub const UNIX_EPOCH: Self = Self(0);

    /// The earliest instant, 1677-09-21T00:12:43.145224192Z.
    ///
    /// ```
    /// use taktsim_adapters::observation::Timestamp;
    ///
    /// assert_eq!(Timestamp::MIN.to_string(), "1677-09-21T00:12:43.145224192Z");
    /// ```
    pub const MIN: Self = Self(i64::MIN);

    /// The latest instant, 2262-04-11T23:47:16.854775807Z.
    ///
    /// ```
    /// use taktsim_adapters::observation::Timestamp;
    ///
    /// assert_eq!(Timestamp::MAX.to_string(), "2262-04-11T23:47:16.854775807Z");
    /// ```
    pub const MAX: Self = Self(i64::MAX);

    /// The instant `nanos` nanoseconds after the Unix epoch (before it if
    /// negative).
    ///
    /// ```
    /// use taktsim_adapters::observation::Timestamp;
    ///
    /// let t = Timestamp::from_unix_nanos(-1_000_000_000);
    /// assert_eq!(t.to_string(), "1969-12-31T23:59:59Z");
    /// ```
    #[must_use]
    pub const fn from_unix_nanos(nanos: i64) -> Self {
        Self(nanos)
    }

    /// Nanoseconds since the Unix epoch.
    ///
    /// ```
    /// use taktsim_adapters::observation::Timestamp;
    ///
    /// assert_eq!(Timestamp::from_unix_nanos(42).as_unix_nanos(), 42);
    /// ```
    #[must_use]
    pub const fn as_unix_nanos(self) -> i64 {
        self.0
    }

    /// Parses an RFC 3339 date-time with an offset, `Z` or `±hh:mm`, and
    /// returns the UTC instant together with the offset the text used.
    ///
    /// The grammar is RFC 3339 §5.6: `YYYY-MM-DDThh:mm:ss`, an optional
    /// fraction of a second, then the offset. `T` may also be `t` or a
    /// space, `Z` may be `z` (§5.6, notes). `-00:00` means "UTC known, local
    /// offset unknown" (§4.3) and gives [`SourceOffset::Unknown`].
    ///
    /// # Errors
    ///
    /// A [`TimestampError`] naming the first problem: malformed text, a
    /// field out of range, no offset, a leap second (Unix time has none), a
    /// fraction finer than 1 ns, or an instant outside [`Timestamp::MIN`] to
    /// [`Timestamp::MAX`].
    ///
    /// ```
    /// use taktsim_adapters::observation::{SourceOffset, Timestamp, TimestampError, UtcOffset};
    ///
    /// let (t, offset) = Timestamp::parse_rfc3339("2026-10-25T02:30:00+01:00")?;
    /// assert_eq!(t.to_string(), "2026-10-25T01:30:00Z");
    /// assert_eq!(offset, SourceOffset::Known(UtcOffset::from_minutes(60).unwrap()));
    /// assert_eq!(
    ///     Timestamp::parse_rfc3339("2026-10-25T02:30:00"),
    ///     Err(TimestampError::MissingOffset)
    /// );
    /// # Ok::<(), TimestampError>(())
    /// ```
    pub fn parse_rfc3339(text: &str) -> Result<(Self, SourceOffset), TimestampError> {
        Self::parse(text, NaiveTimestamps::Reject)
    }

    /// Like [`Timestamp::parse_rfc3339`], but a date-time without an offset
    /// is handled as `naive` says: rejected, or read in a declared fixed
    /// offset.
    ///
    /// # Errors
    ///
    /// As [`Timestamp::parse_rfc3339`]; [`TimestampError::MissingOffset`]
    /// only with [`NaiveTimestamps::Reject`].
    ///
    /// ```
    /// use taktsim_adapters::observation::{NaiveTimestamps, SourceOffset, Timestamp, UtcOffset};
    ///
    /// let utc_plus_one = UtcOffset::from_minutes(60).unwrap();
    /// let naive = NaiveTimestamps::AtOffset(utc_plus_one);
    /// let (t, offset) = Timestamp::parse("2026-03-02T06:00:00", naive)?;
    /// assert_eq!(t.to_string(), "2026-03-02T05:00:00Z");
    /// assert_eq!(offset, SourceOffset::Known(utc_plus_one));
    /// // An explicit offset in the text always wins.
    /// let (t, _) = Timestamp::parse("2026-03-02T06:00:00Z", naive)?;
    /// assert_eq!(t.to_string(), "2026-03-02T06:00:00Z");
    /// # Ok::<(), taktsim_adapters::observation::TimestampError>(())
    /// ```
    pub fn parse(
        text: &str,
        naive: NaiveTimestamps,
    ) -> Result<(Self, SourceOffset), TimestampError> {
        let fields = Fields::parse(text)?;
        let offset = match (fields.offset, naive) {
            (Some(offset), _) => offset,
            (None, NaiveTimestamps::AtOffset(offset)) => SourceOffset::Known(offset),
            (None, NaiveTimestamps::Reject) => return Err(TimestampError::MissingOffset),
        };
        let days = days_from_civil(fields.year, fields.month, fields.day);
        let local_secs = days * SECS_PER_DAY
            + i64::from(fields.hour) * 3_600
            + i64::from(fields.minute) * 60
            + i64::from(fields.second);
        let utc_secs = local_secs - offset.seconds();
        let nanos = i128::from(utc_secs) * i128::from(NANOS_PER_SEC) + i128::from(fields.nanos);
        let nanos = i64::try_from(nanos).map_err(|_| TimestampError::Unrepresentable)?;
        Ok((Self(nanos), offset))
    }

    /// This instant as RFC 3339 text in `offset`, the source's local time.
    ///
    /// The fraction of a second has 0, 3, 6 or 9 digits, whichever is the
    /// fewest that is exact; a zero offset is written `Z`. The text parses
    /// back to the same instant and offset.
    ///
    /// ```
    /// use taktsim_adapters::observation::{SourceOffset, Timestamp, UtcOffset};
    ///
    /// let t = Timestamp::from_unix_nanos(1_792_891_800_000_000_000);
    /// let cet = SourceOffset::Known(UtcOffset::from_minutes(60).unwrap());
    /// assert_eq!(t.rfc3339(cet).to_string(), "2026-10-25T02:30:00+01:00");
    /// assert_eq!(t.rfc3339(SourceOffset::Unknown).to_string(), "2026-10-25T01:30:00-00:00");
    /// assert_eq!(Timestamp::parse_rfc3339("2026-10-25T02:30:00+01:00")?, (t, cet));
    /// # Ok::<(), taktsim_adapters::observation::TimestampError>(())
    /// ```
    #[must_use]
    pub const fn rfc3339(self, offset: SourceOffset) -> Rfc3339 {
        Rfc3339 {
            timestamp: self,
            offset,
        }
    }
}

impl FromStr for Timestamp {
    type Err = TimestampError;

    /// [`Timestamp::parse_rfc3339`] without the offset.
    fn from_str(text: &str) -> Result<Self, TimestampError> {
        Self::parse_rfc3339(text).map(|(timestamp, _)| timestamp)
    }
}

impl fmt::Display for Timestamp {
    /// RFC 3339 in UTC, e.g. `2026-03-02T05:00:00Z`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.rfc3339(SourceOffset::Known(UtcOffset::UTC)).fmt(f)
    }
}

/// A [`Timestamp`] written as RFC 3339 in a given offset; made by
/// [`Timestamp::rfc3339`].
///
/// ```
/// use taktsim_adapters::observation::{SourceOffset, Timestamp, UtcOffset};
///
/// let ist = SourceOffset::Known(UtcOffset::from_minutes(330).unwrap());
/// let text = Timestamp::UNIX_EPOCH.rfc3339(ist).to_string();
/// assert_eq!(text, "1970-01-01T05:30:00+05:30");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rfc3339 {
    timestamp: Timestamp,
    offset: SourceOffset,
}

impl fmt::Display for Rfc3339 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let nanos = self.timestamp.0;
        // |nanos / 10^9| < 9.3 × 10^9, so adding an offset cannot overflow.
        let local_secs = nanos.div_euclid(NANOS_PER_SEC) + self.offset.seconds();
        let fraction = nanos.rem_euclid(NANOS_PER_SEC);
        let (year, month, day) = civil_from_days(local_secs.div_euclid(SECS_PER_DAY));
        let secs_of_day = local_secs.rem_euclid(SECS_PER_DAY);
        write!(
            f,
            "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}",
            secs_of_day / 3_600,
            secs_of_day % 3_600 / 60,
            secs_of_day % 60
        )?;
        if fraction != 0 {
            if fraction % 1_000_000 == 0 {
                write!(f, ".{:03}", fraction / 1_000_000)?;
            } else if fraction % 1_000 == 0 {
                write!(f, ".{:06}", fraction / 1_000)?;
            } else {
                write!(f, ".{fraction:09}")?;
            }
        }
        match self.offset {
            SourceOffset::Known(UtcOffset(0)) => f.write_str("Z"),
            SourceOffset::Known(offset) => offset.fmt(f),
            SourceOffset::Unknown => f.write_str("-00:00"),
        }
    }
}

/// A fixed offset from UTC, in whole minutes east of Greenwich, from
/// `-23:59` to `+23:59` (RFC 3339 `time-numoffset`).
///
/// ```
/// use taktsim_adapters::observation::UtcOffset;
///
/// let new_york_winter = UtcOffset::from_minutes(-300).unwrap();
/// assert_eq!(new_york_winter.to_string(), "-05:00");
/// assert_eq!(new_york_winter.minutes(), -300);
/// assert!(UtcOffset::from_minutes(24 * 60).is_none());
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct UtcOffset(i16);

impl UtcOffset {
    /// UTC itself, `+00:00`.
    ///
    /// ```
    /// use taktsim_adapters::observation::UtcOffset;
    ///
    /// assert_eq!(UtcOffset::UTC.to_string(), "+00:00");
    /// ```
    pub const UTC: Self = Self(0);

    /// The offset of `minutes` east of UTC, or `None` beyond ±23:59.
    ///
    /// ```
    /// use taktsim_adapters::observation::UtcOffset;
    ///
    /// assert_eq!(UtcOffset::from_minutes(120).unwrap().to_string(), "+02:00");
    /// assert!(UtcOffset::from_minutes(-1440).is_none());
    /// ```
    #[must_use]
    pub const fn from_minutes(minutes: i16) -> Option<Self> {
        if minutes.unsigned_abs() <= MAX_OFFSET_MINUTES.unsigned_abs() {
            Some(Self(minutes))
        } else {
            None
        }
    }

    /// Minutes east of UTC; negative west of it.
    ///
    /// ```
    /// use taktsim_adapters::observation::UtcOffset;
    ///
    /// assert_eq!(UtcOffset::from_minutes(-210).unwrap().minutes(), -210);
    /// ```
    #[must_use]
    pub const fn minutes(self) -> i16 {
        self.0
    }
}

impl fmt::Display for UtcOffset {
    /// `±hh:mm`, e.g. `+01:00`; UTC is `+00:00`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { '-' } else { '+' };
        let minutes = self.0.unsigned_abs();
        write!(f, "{sign}{:02}:{:02}", minutes / 60, minutes % 60)
    }
}

/// The UTC offset a source wrote a timestamp in: the time-zone metadata kept
/// with every observation (spec §8).
///
/// ```
/// use taktsim_adapters::observation::{SourceOffset, UtcOffset};
///
/// assert_eq!(SourceOffset::Known(UtcOffset::UTC).to_string(), "+00:00");
/// assert_eq!(SourceOffset::Unknown.to_string(), "-00:00");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum SourceOffset {
    /// The source wrote local time at this offset (`Z` is `+00:00`).
    Known(UtcOffset),
    /// The source wrote UTC and said its local offset is unknown, RFC 3339
    /// `-00:00` (§4.3).
    Unknown,
}

impl SourceOffset {
    /// Seconds to add to UTC to get the source's local time.
    fn seconds(self) -> i64 {
        match self {
            Self::Known(UtcOffset(minutes)) => i64::from(minutes) * 60,
            Self::Unknown => 0,
        }
    }
}

impl fmt::Display for SourceOffset {
    /// `±hh:mm`; `-00:00` if unknown.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Known(offset) => offset.fmt(f),
            Self::Unknown => f.write_str("-00:00"),
        }
    }
}

/// What to do with a date-time that has no UTC offset, such as
/// `2026-03-02T06:00:00`.
///
/// v0.1 has no time-zone database, so only a fixed offset can be declared.
/// That is exact for a source whose clock never changes offset (UTC, or a
/// controller kept on standard time all year). It is wrong for local
/// wall-clock time in a zone with daylight saving time: summer readings
/// would be off by the DST shift, and the repeated hour in autumn is
/// ambiguous. For such data, write the offset into each timestamp at the
/// source (ADR-0017).
///
/// ```
/// use taktsim_adapters::observation::{NaiveTimestamps, UtcOffset};
///
/// assert_eq!(NaiveTimestamps::default(), NaiveTimestamps::Reject);
/// let utc = NaiveTimestamps::AtOffset(UtcOffset::UTC);
/// assert_ne!(utc, NaiveTimestamps::Reject);
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NaiveTimestamps {
    /// Reject it with [`TimestampError::MissingOffset`] (the default).
    #[default]
    Reject,
    /// Read it as local time at this fixed offset.
    AtOffset(UtcOffset),
}

/// Why a timestamp could not be read.
///
/// [`Display`](fmt::Display) says what is wrong; [`TimestampError::ALLOWED`]
/// and [`TimestampError::EXAMPLE`] say what is accepted.
///
/// ```
/// use taktsim_adapters::observation::{Timestamp, TimestampError};
///
/// let err = "2026-02-29T06:00:00Z".parse::<Timestamp>().unwrap_err();
/// assert_eq!(err, TimestampError::OutOfRange { field: "day", value: 29, min: 1, max: 28 });
/// assert_eq!(err.to_string(), "day 29 is out of range 01 to 28");
/// assert_eq!(TimestampError::EXAMPLE, "2026-03-02T06:00:00+01:00");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum TimestampError {
    /// The text is empty.
    #[error("the timestamp is empty")]
    Empty,
    /// The text does not follow the RFC 3339 grammar.
    #[error("expected {expected} at character {position}")]
    Malformed {
        /// What the grammar requires at `position`, e.g. "a 2-digit month".
        expected: &'static str,
        /// 1-based character position.
        position: usize,
    },
    /// A date-time without `Z` or `±hh:mm`.
    #[error(
        "the timestamp has no UTC offset; add `Z` or `±hh:mm`, or declare the \
         source's fixed offset (`NaiveTimestamps::AtOffset`)"
    )]
    MissingOffset,
    /// A field is outside its range, e.g. month 13 or 31 April.
    #[error("{field} {value:02} is out of range {min:02} to {max:02}")]
    OutOfRange {
        /// The field: "month", "day", "hour", "minute", "second",
        /// "offset hour" or "offset minute".
        field: &'static str,
        /// The value found.
        value: u32,
        /// The smallest allowed value.
        min: u32,
        /// The largest allowed value, e.g. 28 for February 2026.
        max: u32,
    },
    /// Second 60. RFC 3339 allows leap seconds, but a [`Timestamp`] counts
    /// seconds without them, as Unix time does.
    #[error(
        "second 60 is a leap second, which a UTC timestamp without leap seconds cannot represent"
    )]
    LeapSecond,
    /// More than nine fractional digits, not all of them zero.
    #[error("the fraction of a second is finer than 1 ns; use at most 9 digits after `.`")]
    TooPrecise,
    /// The instant is before [`Timestamp::MIN`] or after [`Timestamp::MAX`].
    #[error(
        "the instant is outside the representable range \
         1677-09-21T00:12:43.145224192Z to 2262-04-11T23:47:16.854775807Z"
    )]
    Unrepresentable,
}

impl TimestampError {
    /// What a timestamp may look like.
    ///
    /// ```
    /// use taktsim_adapters::observation::TimestampError;
    ///
    /// assert!(TimestampError::ALLOWED.starts_with("an RFC 3339 date-time"));
    /// ```
    pub const ALLOWED: &'static str = "an RFC 3339 date-time YYYY-MM-DDThh:mm:ss with an optional fraction \
         of up to 9 digits and `Z` or a UTC offset `±hh:mm`, between 1677-09-21 and 2262-04-11";

    /// One valid timestamp.
    ///
    /// ```
    /// use taktsim_adapters::observation::{Timestamp, TimestampError};
    ///
    /// assert!(TimestampError::EXAMPLE.parse::<Timestamp>().is_ok());
    /// ```
    pub const EXAMPLE: &'static str = "2026-03-02T06:00:00+01:00";
}

/// The fields of an RFC 3339 date-time, each within its range.
struct Fields {
    year: i64,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    nanos: u32,
    offset: Option<SourceOffset>,
}

impl Fields {
    fn parse(text: &str) -> Result<Self, TimestampError> {
        if text.is_empty() {
            return Err(TimestampError::Empty);
        }
        let mut c = Cursor {
            bytes: text.as_bytes(),
            pos: 0,
        };
        let year = c.digits(4, "a 4-digit year")?;
        c.byte(b"-", "`-` after the year")?;
        let month = in_range(c.digits(2, "a 2-digit month")?, "month", 1, 12)?;
        c.byte(b"-", "`-` after the month")?;
        let day = in_range(
            c.digits(2, "a 2-digit day")?,
            "day",
            1,
            days_in_month(year, month),
        )?;
        c.byte(b"Tt ", "`T` between the date and the time")?;
        let hour = in_range(c.digits(2, "a 2-digit hour")?, "hour", 0, 23)?;
        c.byte(b":", "`:` after the hour")?;
        let minute = in_range(c.digits(2, "a 2-digit minute")?, "minute", 0, 59)?;
        c.byte(b":", "`:` after the minute")?;
        let second = c.digits(2, "a 2-digit second")?;
        if second == 60 {
            return Err(TimestampError::LeapSecond);
        }
        let second = in_range(second, "second", 0, 59)?;
        let nanos = if c.peek() == Some(b'.') {
            c.pos += 1;
            c.fraction()?
        } else {
            0
        };
        let offset = match c.peek() {
            None => None,
            Some(b'Z' | b'z') => {
                c.pos += 1;
                Some(SourceOffset::Known(UtcOffset::UTC))
            }
            Some(sign @ (b'+' | b'-')) => {
                c.pos += 1;
                let hours = in_range(c.digits(2, "a 2-digit offset hour")?, "offset hour", 0, 23)?;
                c.byte(b":", "`:` in the offset")?;
                let minutes = in_range(
                    c.digits(2, "a 2-digit offset minute")?,
                    "offset minute",
                    0,
                    59,
                )?;
                // At most 23 × 60 + 59 = 1439, so the cast is exact.
                #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
                let minutes = (hours * 60 + minutes) as i16;
                Some(match (sign, minutes) {
                    (b'-', 0) => SourceOffset::Unknown,
                    (b'-', m) => SourceOffset::Known(UtcOffset(-m)),
                    (_, m) => SourceOffset::Known(UtcOffset(m)),
                })
            }
            Some(_) => return Err(c.malformed("`Z` or a UTC offset such as `+01:00`")),
        };
        if c.pos < c.bytes.len() {
            return Err(c.malformed("the end of the timestamp"));
        }
        Ok(Self {
            year: i64::from(year),
            month,
            day,
            hour,
            minute,
            second,
            nanos,
            offset,
        })
    }
}

/// Reads ASCII text left to right. Everything before `pos` is ASCII, so
/// `pos` is also a character position.
struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl Cursor<'_> {
    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn malformed(&self, expected: &'static str) -> TimestampError {
        TimestampError::Malformed {
            expected,
            position: self.pos + 1,
        }
    }

    /// Exactly `n` ASCII digits (n ≤ 4).
    fn digits(&mut self, n: usize, expected: &'static str) -> Result<u32, TimestampError> {
        let mut value = 0;
        for _ in 0..n {
            match self.peek() {
                Some(b @ b'0'..=b'9') => {
                    value = value * 10 + u32::from(b - b'0');
                    self.pos += 1;
                }
                _ => return Err(self.malformed(expected)),
            }
        }
        Ok(value)
    }

    fn byte(&mut self, allowed: &[u8], expected: &'static str) -> Result<(), TimestampError> {
        match self.peek() {
            Some(b) if allowed.contains(&b) => {
                self.pos += 1;
                Ok(())
            }
            _ => Err(self.malformed(expected)),
        }
    }

    /// The digits after `.`, as nanoseconds. Digits beyond the ninth must be
    /// zero, so the value is exact.
    fn fraction(&mut self) -> Result<u32, TimestampError> {
        let start = self.pos;
        let mut nanos = 0;
        let mut too_precise = false;
        while let Some(b @ b'0'..=b'9') = self.peek() {
            let digit = u32::from(b - b'0');
            if self.pos - start < 9 {
                nanos = nanos * 10 + digit;
            } else if digit != 0 {
                too_precise = true;
            }
            self.pos += 1;
        }
        let digits = self.pos - start;
        if digits == 0 {
            return Err(self.malformed("a digit after `.`"));
        }
        if too_precise {
            return Err(TimestampError::TooPrecise);
        }
        for _ in digits..9 {
            nanos *= 10;
        }
        Ok(nanos)
    }
}

fn in_range(value: u32, field: &'static str, min: u32, max: u32) -> Result<u32, TimestampError> {
    if (min..=max).contains(&value) {
        Ok(value)
    } else {
        Err(TimestampError::OutOfRange {
            field,
            value,
            min,
            max,
        })
    }
}

fn is_leap_year(year: i64) -> bool {
    year % 4 == 0 && (year % 100 != 0 || year % 400 == 0)
}

fn days_in_month(year: u32, month: u32) -> u32 {
    match month {
        2 if is_leap_year(i64::from(year)) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

/// Days since 1970-01-01 of a proleptic Gregorian date (H. Hinnant,
/// "chrono-Compatible Low-Level Date Algorithms", `days_from_civil`).
fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = if month <= 2 { year - 1 } else { year };
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let shifted_month = i64::from(if month > 2 { month - 3 } else { month + 9 });
    let day_of_year = (153 * shifted_month + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * DAYS_PER_ERA + day_of_era - DAYS_TO_UNIX_EPOCH
}

/// The proleptic Gregorian date of a day since 1970-01-01 (H. Hinnant,
/// `civil_from_days`), the inverse of [`days_from_civil`].
fn civil_from_days(days: i64) -> (i64, i64, i64) {
    let days = days + DAYS_TO_UNIX_EPOCH;
    let era = days.div_euclid(DAYS_PER_ERA);
    let day_of_era = days.rem_euclid(DAYS_PER_ERA);
    let year_of_era =
        (day_of_era - day_of_era / 1_460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = day_of_year - (153 * shifted_month + 2) / 5 + 1;
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    };
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn err(text: &str) -> TimestampError {
        Timestamp::parse_rfc3339(text).unwrap_err()
    }

    fn nanos(text: &str) -> i64 {
        Timestamp::parse_rfc3339(text).unwrap().0.as_unix_nanos()
    }

    #[test]
    fn known_instants() {
        // Expected values from Python 3.14:
        // (datetime.fromisoformat(s) - datetime(1970,1,1,tzinfo=timezone.utc))
        //   // timedelta(microseconds=1) * 1000
        assert_eq!(
            nanos("2026-03-02T06:00:00+01:00"),
            1_772_427_600_000_000_000
        );
        assert_eq!(nanos("2026-03-02T05:00:00Z"), 1_772_427_600_000_000_000);
        assert_eq!(
            nanos("2026-10-25T02:30:00+02:00"),
            1_792_888_200_000_000_000
        );
        assert_eq!(
            nanos("2026-10-25T02:30:00+01:00"),
            1_792_891_800_000_000_000
        );
        assert_eq!(nanos("1969-12-31T23:59:59Z"), -1_000_000_000);
        assert_eq!(nanos("2000-02-29T12:00:00-05:30"), 951_845_400_000_000_000);
        assert_eq!(nanos("1677-09-21T00:12:44Z"), -9_223_372_036_000_000_000);
        assert_eq!(nanos("2262-04-11T23:47:16Z"), 9_223_372_036_000_000_000);
    }

    #[test]
    fn range_limits_are_exact() {
        assert_eq!(nanos("1677-09-21T00:12:43.145224192Z"), i64::MIN);
        assert_eq!(nanos("2262-04-11T23:47:16.854775807Z"), i64::MAX);
        assert_eq!(
            err("1677-09-21T00:12:43.145224191Z"),
            TimestampError::Unrepresentable
        );
        assert_eq!(
            err("2262-04-11T23:47:16.854775808Z"),
            TimestampError::Unrepresentable
        );
        // The offset moves the instant out of range.
        assert_eq!(
            err("2262-04-11T23:47:16-01:00"),
            TimestampError::Unrepresentable
        );
        assert_eq!(err("0000-01-01T00:00:00Z"), TimestampError::Unrepresentable);
        assert_eq!(err("9999-12-31T23:59:59Z"), TimestampError::Unrepresentable);
    }

    #[test]
    fn fractions_are_exact() {
        assert_eq!(nanos("1970-01-01T00:00:00.5Z"), 500_000_000);
        assert_eq!(nanos("1970-01-01T00:00:00.000000001Z"), 1);
        assert_eq!(nanos("1970-01-01T00:00:00.123456789000Z"), 123_456_789);
        assert_eq!(nanos("1969-12-31T23:59:59.999999999Z"), -1);
        assert_eq!(
            err("1970-01-01T00:00:00.0000000001Z"),
            TimestampError::TooPrecise
        );
        assert_eq!(
            err("1970-01-01T00:00:00.Z"),
            TimestampError::Malformed {
                expected: "a digit after `.`",
                position: 21
            }
        );
    }

    #[test]
    fn separators_and_offsets() {
        let t = nanos("2026-03-02T06:00:00Z");
        assert_eq!(nanos("2026-03-02t06:00:00z"), t);
        assert_eq!(nanos("2026-03-02 06:00:00+00:00"), t);
        assert_eq!(nanos("2026-03-02T06:00:00-00:00"), t);
        assert_eq!(nanos("2026-03-02T07:00:00+01:00"), t);
        assert_eq!(nanos("2026-03-02T00:30:00-05:30"), t);
        assert_eq!(nanos("2026-03-03T05:59:00+23:59"), t);
        let offset = |text| Timestamp::parse_rfc3339(text).unwrap().1;
        assert_eq!(
            offset("2026-03-02T06:00:00Z"),
            SourceOffset::Known(UtcOffset::UTC)
        );
        assert_eq!(
            offset("2026-03-02T06:00:00+00:00"),
            SourceOffset::Known(UtcOffset::UTC)
        );
        assert_eq!(offset("2026-03-02T06:00:00-00:00"), SourceOffset::Unknown);
        assert_eq!(
            offset("2026-03-02T06:00:00-05:30"),
            SourceOffset::Known(UtcOffset(-330))
        );
    }

    #[test]
    fn malformed_text_names_the_position() {
        let malformed = |expected, position| TimestampError::Malformed { expected, position };
        assert_eq!(err(""), TimestampError::Empty);
        assert_eq!(err("26-03-02T06:00:00Z"), malformed("a 4-digit year", 3));
        assert_eq!(
            err("2026/03/02T06:00:00Z"),
            malformed("`-` after the year", 5)
        );
        assert_eq!(err("2026-3-02T06:00:00Z"), malformed("a 2-digit month", 7));
        assert_eq!(
            err("2026-03_02T06:00:00Z"),
            malformed("`-` after the month", 8)
        );
        assert_eq!(err("2026-03-2T06:00:00Z"), malformed("a 2-digit day", 10));
        assert_eq!(
            err("2026-03-02_06:00:00Z"),
            malformed("`T` between the date and the time", 11)
        );
        assert_eq!(err("2026-03-02T6:00:00Z"), malformed("a 2-digit hour", 13));
        assert_eq!(
            err("2026-03-02T06.00:00Z"),
            malformed("`:` after the hour", 14)
        );
        assert_eq!(
            err("2026-03-02T06:0:00Z"),
            malformed("a 2-digit minute", 16)
        );
        assert_eq!(
            err("2026-03-02T06:00Z"),
            malformed("`:` after the minute", 17)
        );
        assert_eq!(
            err("2026-03-02T06:00:0Z"),
            malformed("a 2-digit second", 19)
        );
        assert_eq!(
            err("2026-03-02T06:00:00 UTC"),
            malformed("`Z` or a UTC offset such as `+01:00`", 20)
        );
        assert_eq!(
            err("2026-03-02T06:00:00+1:00"),
            malformed("a 2-digit offset hour", 22)
        );
        assert_eq!(
            err("2026-03-02T06:00:00+0100"),
            malformed("`:` in the offset", 23)
        );
        assert_eq!(
            err("2026-03-02T06:00:00+01:0"),
            malformed("a 2-digit offset minute", 25)
        );
        assert_eq!(
            err("2026-03-02T06:00:00Zjunk"),
            malformed("the end of the timestamp", 21)
        );
        // A non-ASCII character: the position counts characters, not bytes.
        assert_eq!(
            err("2026-03-02T06:00:00\u{2212}01:00"),
            malformed("`Z` or a UTC offset such as `+01:00`", 20)
        );
    }

    #[test]
    fn fields_out_of_range() {
        let out = |field, value, min, max| TimestampError::OutOfRange {
            field,
            value,
            min,
            max,
        };
        assert_eq!(err("2026-00-01T00:00:00Z"), out("month", 0, 1, 12));
        assert_eq!(err("2026-13-01T00:00:00Z"), out("month", 13, 1, 12));
        assert_eq!(err("2026-04-31T00:00:00Z"), out("day", 31, 1, 30));
        assert_eq!(err("2026-02-29T00:00:00Z"), out("day", 29, 1, 28));
        assert_eq!(err("1900-02-29T00:00:00Z"), out("day", 29, 1, 28));
        assert_eq!(nanos("2000-02-29T00:00:00Z"), 951_782_400_000_000_000);
        assert_eq!(nanos("2024-02-29T00:00:00Z"), 1_709_164_800_000_000_000);
        assert_eq!(err("2026-01-00T00:00:00Z"), out("day", 0, 1, 31));
        assert_eq!(err("2026-01-01T24:00:00Z"), out("hour", 24, 0, 23));
        assert_eq!(err("2026-01-01T00:60:00Z"), out("minute", 60, 0, 59));
        assert_eq!(err("2026-01-01T00:00:61Z"), out("second", 61, 0, 59));
        assert_eq!(err("2016-12-31T23:59:60Z"), TimestampError::LeapSecond);
        assert_eq!(
            err("2026-01-01T00:00:00+24:00"),
            out("offset hour", 24, 0, 23)
        );
        assert_eq!(
            err("2026-01-01T00:00:00+01:60"),
            out("offset minute", 60, 0, 59)
        );
    }

    #[test]
    fn naive_timestamps_need_a_declared_offset() {
        assert_eq!(err("2026-03-02T06:00:00"), TimestampError::MissingOffset);
        assert_eq!(err("2026-03-02T06:00:00.5"), TimestampError::MissingOffset);
        let utc = NaiveTimestamps::AtOffset(UtcOffset::UTC);
        let (t, offset) = Timestamp::parse("2026-03-02T06:00:00", utc).unwrap();
        assert_eq!(t.as_unix_nanos(), nanos("2026-03-02T06:00:00Z"));
        assert_eq!(offset, SourceOffset::Known(UtcOffset::UTC));
        let minus_three = NaiveTimestamps::AtOffset(UtcOffset(-180));
        let (t, _) = Timestamp::parse("2026-03-02T06:00:00", minus_three).unwrap();
        assert_eq!(t.as_unix_nanos(), nanos("2026-03-02T09:00:00Z"));
    }

    #[test]
    fn offsets() {
        assert_eq!(
            UtcOffset::from_minutes(-210).map(UtcOffset::minutes),
            Some(-210)
        );
        assert_eq!(SourceOffset::Known(UtcOffset(90)).to_string(), "+01:30");
        assert_eq!(SourceOffset::Known(UtcOffset::UTC).to_string(), "+00:00");
        assert_eq!(SourceOffset::Unknown.to_string(), "-00:00");
        assert!(SourceOffset::Known(UtcOffset(1439)) < SourceOffset::Unknown);
    }

    #[test]
    fn formatting() {
        let at = |n: i64, minutes: i16| {
            Timestamp(n)
                .rfc3339(SourceOffset::Known(UtcOffset(minutes)))
                .to_string()
        };
        assert_eq!(at(0, 0), "1970-01-01T00:00:00Z");
        assert_eq!(at(1, 0), "1970-01-01T00:00:00.000000001Z");
        assert_eq!(at(1_000, 0), "1970-01-01T00:00:00.000001Z");
        assert_eq!(at(1_000_000, 0), "1970-01-01T00:00:00.001Z");
        assert_eq!(at(120_000_000, 0), "1970-01-01T00:00:00.120Z");
        assert_eq!(at(-1, 0), "1969-12-31T23:59:59.999999999Z");
        assert_eq!(at(0, -330), "1969-12-31T18:30:00-05:30");
        assert_eq!(at(0, 1439), "1970-01-01T23:59:00+23:59");
        assert_eq!(at(i64::MAX, 1439), "2262-04-12T23:46:16.854775807+23:59");
        assert_eq!(at(i64::MIN, -1439), "1677-09-20T00:13:43.145224192-23:59");
        assert_eq!(UtcOffset(-1439).to_string(), "-23:59");
        assert_eq!(UtcOffset::from_minutes(1439), Some(UtcOffset(1439)));
        assert_eq!(UtcOffset::from_minutes(-1440), None);
    }

    #[test]
    fn errors_read_well() {
        assert_eq!(
            err("2026-03-02T06:00").to_string(),
            "expected `:` after the minute at character 17"
        );
        assert_eq!(
            err("2026-13-01T00:00:00Z").to_string(),
            "month 13 is out of range 01 to 12"
        );
        assert_eq!(err("").to_string(), "the timestamp is empty");
        assert!(
            err("2026-03-02T06:00:00")
                .to_string()
                .contains("no UTC offset")
        );
        assert!(
            err("2016-12-31T23:59:60Z")
                .to_string()
                .contains("leap second")
        );
        assert!(
            err("2026-03-02T06:00:00.1234567891Z")
                .to_string()
                .contains("1 ns")
        );
        assert!(
            err("9999-01-01T00:00:00Z")
                .to_string()
                .contains("representable range")
        );
    }

    #[test]
    fn calendar_inverse_on_era_boundaries() {
        for days in [
            -719_468, -146_097, -1, 0, 1, 10_956, 11_016, 146_096, 2_932_896,
        ] {
            let (y, m, d) = civil_from_days(days);
            let m = u32::try_from(m).unwrap();
            let d = u32::try_from(d).unwrap();
            assert_eq!(days_from_civil(y, m, d), days, "day {days}");
        }
        assert_eq!(civil_from_days(0), (1970, 1, 1));
        assert_eq!(civil_from_days(-719_468), (0, 3, 1));
        assert_eq!(civil_from_days(11_016), (2000, 2, 29));
    }
}
