//! How a model's messages are written to a trace: the [`TraceMessage`]
//! trait and the fixed-width little-endian [`Encoder`] and [`Decoder`] it
//! uses (ADR-0021).

use std::fmt;

use taktsim_core::{ComponentId, EventId, ItemId, Message};
use taktsim_units::{SimDuration, SimTime};

/// The bit pattern every NaN is written as. Arithmetic that produces a NaN
/// gives different sign and payload bits on x86-64 and ARM64, so writing a
/// NaN's own bits would make traces differ between platforms.
pub(crate) const CANONICAL_NAN: u64 = 0x7ff8_0000_0000_0000;

/// A message type that can be written to and read from a trace.
///
/// The engine never looks inside messages (ADR-0018), so the component
/// package that defines a message type also defines its encoding. The
/// encoding is part of the trace contract, like the priorities: the same
/// message must give the same bytes on every platform, and changing the
/// encoding changes traces. The rules:
///
/// - Use only the [`Encoder`] methods. They write fixed-width little-endian
///   integers, floats as their exact bits (every NaN as one canonical NaN),
///   and text with a length prefix; there is no method for `usize`, whose
///   width depends on the platform.
/// - Write an enum as a tag (`u8`) followed by the fields of its variant.
/// - [`decode`](Self::decode) reads exactly what
///   [`encode`](Self::encode) wrote and rejects everything else (an unknown
///   tag is an error, never a default), so
///   `decode(encode(m)) == m` and two different messages never share bytes.
/// - Change [`FORMAT`](Self::FORMAT) whenever the encoding changes. A
///   reader refuses a trace written with another format.
///
/// [`encode_message`] and [`decode_message`] help test an implementation.
///
/// ```
/// use taktsim_core::Message;
/// use taktsim_trace::{decode_message, encode_message, DecodeError, Decoder, Encoder, TraceMessage};
///
/// #[derive(Debug, Clone, PartialEq)]
/// enum Flow {
///     Arrive,
///     Done { lot: u32, yield_share: f64 },
/// }
/// impl Message for Flow {}
///
/// impl TraceMessage for Flow {
///     const FORMAT: &'static str = "example.flow/1";
///
///     fn encode(&self, out: &mut Encoder<'_>) {
///         match self {
///             Flow::Arrive => out.u8(0),
///             Flow::Done { lot, yield_share } => {
///                 out.u8(1);
///                 out.u32(*lot);
///                 out.f64(*yield_share);
///             }
///         }
///     }
///
///     fn decode(input: &mut Decoder<'_>) -> Result<Self, DecodeError> {
///         match input.u8()? {
///             0 => Ok(Flow::Arrive),
///             1 => Ok(Flow::Done { lot: input.u32()?, yield_share: input.f64()? }),
///             tag => Err(input.invalid("Flow tag", tag.into(), "0 or 1")),
///         }
///     }
/// }
///
/// let done = Flow::Done { lot: 7, yield_share: 0.98 };
/// let bytes = encode_message(&done)?;
/// assert_eq!(bytes.len(), 1 + 4 + 8);
/// assert_eq!(decode_message::<Flow>(&bytes)?, done);
/// assert!(decode_message::<Flow>(&[2]).is_err());
/// # Ok::<(), Box<dyn std::error::Error>>(())
/// ```
pub trait TraceMessage: Message + Sized {
    /// The name and version of the encoding, e.g. `"taktsim-components/1"`.
    /// Written into the trace header (at most 65,535 bytes of UTF-8).
    const FORMAT: &'static str;

    /// Writes the message.
    fn encode(&self, out: &mut Encoder<'_>);

    /// Reads a message written by [`encode`](Self::encode).
    ///
    /// # Errors
    ///
    /// A [`DecodeError`] if the bytes are not a message of this type.
    fn decode(input: &mut Decoder<'_>) -> Result<Self, DecodeError>;
}

/// Encodes one message, e.g. to test a [`TraceMessage`] implementation.
///
/// # Errors
///
/// [`EncodeError`] if a value does not fit the format (text longer than
/// 65,535 bytes, a component number above `u32::MAX`).
///
/// ```
/// use taktsim_trace::encode_message;
/// # use taktsim_trace::{DecodeError, Decoder, Encoder, TraceMessage};
/// # #[derive(Debug)] struct Tick(u16);
/// # impl taktsim_core::Message for Tick {}
/// # impl TraceMessage for Tick {
/// #     const FORMAT: &'static str = "tick/1";
/// #     fn encode(&self, out: &mut Encoder<'_>) { out.u16(self.0) }
/// #     fn decode(input: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick(input.u16()?)) }
/// # }
///
/// assert_eq!(encode_message(&Tick(0x0102))?, [0x02, 0x01]);
/// # Ok::<(), taktsim_trace::EncodeError>(())
/// ```
pub fn encode_message<M: TraceMessage>(message: &M) -> Result<Vec<u8>, EncodeError> {
    let mut out = Vec::new();
    let mut encoder = Encoder::new(&mut out);
    message.encode(&mut encoder);
    encoder.finish()?;
    Ok(out)
}

/// Decodes one message that fills `bytes` exactly.
///
/// # Errors
///
/// [`DecodeError`] if `bytes` is not one message of type `M`, including
/// bytes left over after it.
///
/// ```
/// use taktsim_trace::{decode_message, DecodeError};
/// # use taktsim_trace::{Decoder, Encoder, TraceMessage};
/// # #[derive(Debug, PartialEq)] struct Tick(u16);
/// # impl taktsim_core::Message for Tick {}
/// # impl TraceMessage for Tick {
/// #     const FORMAT: &'static str = "tick/1";
/// #     fn encode(&self, out: &mut Encoder<'_>) { out.u16(self.0) }
/// #     fn decode(input: &mut Decoder<'_>) -> Result<Self, DecodeError> { Ok(Tick(input.u16()?)) }
/// # }
///
/// assert_eq!(decode_message::<Tick>(&[0x02, 0x01])?, Tick(0x0102));
/// assert!(matches!(decode_message::<Tick>(&[1, 2, 3]), Err(DecodeError::TrailingBytes { .. })));
/// # Ok::<(), DecodeError>(())
/// ```
pub fn decode_message<M: TraceMessage>(bytes: &[u8]) -> Result<M, DecodeError> {
    let mut decoder = Decoder::new(bytes);
    let message = M::decode(&mut decoder)?;
    decoder.finish()?;
    Ok(message)
}

/// Writes values in the trace's fixed-width little-endian encoding.
///
/// Infallible per call, so encoders read like the data they write; a value
/// that does not fit the format is remembered and reported once the message
/// is complete.
///
/// ```
/// use taktsim_trace::Encoder;
///
/// let mut bytes = Vec::new();
/// let mut out = Encoder::new(&mut bytes);
/// out.u32(1);
/// out.str("M1");
/// out.finish()?;
/// assert_eq!(bytes, [1, 0, 0, 0, 2, 0, b'M', b'1']);
/// # Ok::<(), taktsim_trace::EncodeError>(())
/// ```
#[derive(Debug)]
pub struct Encoder<'a> {
    out: &'a mut Vec<u8>,
    error: Option<EncodeError>,
}

impl<'a> Encoder<'a> {
    /// An encoder that appends to `out`.
    ///
    /// ```
    /// let mut bytes = vec![9];
    /// taktsim_trace::Encoder::new(&mut bytes).u8(1);
    /// assert_eq!(bytes, [9, 1]);
    /// ```
    pub fn new(out: &'a mut Vec<u8>) -> Self {
        Self { out, error: None }
    }

    /// The first value that did not fit the format, if any.
    ///
    /// # Errors
    ///
    /// The [`EncodeError`] of the first value that did not fit.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// let mut out = taktsim_trace::Encoder::new(&mut bytes);
    /// out.str(&"x".repeat(70_000));
    /// assert!(out.finish().is_err());
    /// ```
    pub fn finish(self) -> Result<(), EncodeError> {
        self.error.map_or(Ok(()), Err)
    }

    fn fail(&mut self, error: EncodeError) {
        self.error.get_or_insert(error);
    }

    /// Writes a byte.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// taktsim_trace::Encoder::new(&mut bytes).u8(0xff);
    /// assert_eq!(bytes, [0xff]);
    /// ```
    pub fn u8(&mut self, value: u8) {
        self.out.push(value);
    }

    /// Writes a `u16`, little-endian.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// taktsim_trace::Encoder::new(&mut bytes).u16(0x0102);
    /// assert_eq!(bytes, [2, 1]);
    /// ```
    pub fn u16(&mut self, value: u16) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    /// Writes a `u32`, little-endian.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// taktsim_trace::Encoder::new(&mut bytes).u32(1);
    /// assert_eq!(bytes, [1, 0, 0, 0]);
    /// ```
    pub fn u32(&mut self, value: u32) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    /// Writes a `u64`, little-endian.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// taktsim_trace::Encoder::new(&mut bytes).u64(1);
    /// assert_eq!(bytes, [1, 0, 0, 0, 0, 0, 0, 0]);
    /// ```
    pub fn u64(&mut self, value: u64) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    /// Writes an `i32`, two's complement, little-endian.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// taktsim_trace::Encoder::new(&mut bytes).i32(-2);
    /// assert_eq!(bytes, [0xfe, 0xff, 0xff, 0xff]);
    /// ```
    pub fn i32(&mut self, value: i32) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    /// Writes an `i64`, two's complement, little-endian.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// taktsim_trace::Encoder::new(&mut bytes).i64(-1);
    /// assert_eq!(bytes, [0xff; 8]);
    /// ```
    pub fn i64(&mut self, value: i64) {
        self.out.extend_from_slice(&value.to_le_bytes());
    }

    /// Writes a `bool` as one byte, 0 or 1.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// taktsim_trace::Encoder::new(&mut bytes).bool(true);
    /// assert_eq!(bytes, [1]);
    /// ```
    pub fn bool(&mut self, value: bool) {
        self.out.push(u8::from(value));
    }

    /// Writes an `f64` as its IEEE 754 bits, little-endian, so it reads
    /// back exactly. Every NaN is written as the same quiet NaN
    /// (`0x7ff8000000000000`): processors differ in the sign and payload of
    /// the NaNs that arithmetic produces.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// let mut out = taktsim_trace::Encoder::new(&mut bytes);
    /// out.f64(-0.0);
    /// out.f64(-f64::NAN);
    /// assert_eq!(bytes[..8], (-0.0_f64).to_bits().to_le_bytes());
    /// assert_eq!(bytes[8..], 0x7ff8_0000_0000_0000_u64.to_le_bytes());
    /// ```
    pub fn f64(&mut self, value: f64) {
        let bits = if value.is_nan() {
            CANONICAL_NAN
        } else {
            value.to_bits()
        };
        self.u64(bits);
    }

    /// Writes text: its length in bytes as a `u16`, then its UTF-8 bytes.
    /// Text longer than 65,535 bytes does not fit.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// taktsim_trace::Encoder::new(&mut bytes).str("ok");
    /// assert_eq!(bytes, [2, 0, b'o', b'k']);
    /// ```
    pub fn str(&mut self, value: &str) {
        if let Ok(length) = u16::try_from(value.len()) {
            self.u16(length);
            self.out.extend_from_slice(value.as_bytes());
        } else {
            self.fail(EncodeError::TooLong { bytes: value.len() });
        }
    }

    /// Writes a simulation time in nanoseconds, as a `u64`.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// taktsim_trace::Encoder::new(&mut bytes).time("1s".parse()?);
    /// assert_eq!(bytes, 1_000_000_000_u64.to_le_bytes());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn time(&mut self, value: SimTime) {
        self.u64(value.as_nanos());
    }

    /// Writes a duration in nanoseconds, as a `u64`.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// taktsim_trace::Encoder::new(&mut bytes).duration("2s".parse()?);
    /// assert_eq!(bytes, 2_000_000_000_u64.to_le_bytes());
    /// # Ok::<(), taktsim_units::UnitError>(())
    /// ```
    pub fn duration(&mut self, value: SimDuration) {
        self.u64(value.as_nanos());
    }

    /// Writes a component as its number, a `u32`. A number above
    /// `u32::MAX` does not fit.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// taktsim_trace::Encoder::new(&mut bytes).component(taktsim_core::ComponentId::from_index(3));
    /// assert_eq!(bytes, [3, 0, 0, 0]);
    /// ```
    pub fn component(&mut self, value: ComponentId) {
        if let Ok(index) = u32::try_from(value.index()) {
            self.u32(index);
        } else {
            self.fail(EncodeError::ComponentOutOfRange {
                index: value.index(),
            });
        }
    }

    /// Writes an item as its number, a `u64`.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// taktsim_trace::Encoder::new(&mut bytes).item(taktsim_core::ItemId::from_raw(5));
    /// assert_eq!(bytes, 5_u64.to_le_bytes());
    /// ```
    pub fn item(&mut self, value: ItemId) {
        self.u64(value.as_u64());
    }

    /// Writes an event as its sequence number, a `u64`.
    ///
    /// ```
    /// let mut bytes = Vec::new();
    /// taktsim_trace::Encoder::new(&mut bytes).event(taktsim_core::EventId::from_raw(6));
    /// assert_eq!(bytes, 6_u64.to_le_bytes());
    /// ```
    pub fn event(&mut self, value: EventId) {
        self.u64(value.as_u64());
    }
}

/// A value that does not fit the trace format.
///
/// ```
/// use taktsim_trace::EncodeError;
///
/// let e = EncodeError::TooLong { bytes: 70_000 };
/// assert_eq!(e.to_string(), "text of 70000 bytes does not fit a trace: at most 65535 bytes are allowed");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum EncodeError {
    /// Text longer than a `u16` length prefix allows.
    #[error(
        "text of {bytes} bytes does not fit a trace: at most {max} bytes are allowed",
        max = u16::MAX
    )]
    TooLong {
        /// Its length in bytes.
        bytes: usize,
    },
    /// An encoded record larger than a chunk.
    #[error("a record of {bytes} bytes does not fit a trace: at most {max} bytes are allowed")]
    RecordTooLarge {
        /// Its size in bytes.
        bytes: usize,
        /// The largest record.
        max: usize,
    },
    /// A component number above `u32::MAX`.
    #[error(
        "component number {index} does not fit a trace: at most {max} is allowed",
        max = u32::MAX
    )]
    ComponentOutOfRange {
        /// The component's number.
        index: usize,
    },
}

/// Reads values written by an [`Encoder`], checking each.
///
/// ```
/// use taktsim_trace::Decoder;
///
/// let mut input = Decoder::new(&[1, 0, 0, 0, 2, 0, b'M', b'1']);
/// assert_eq!(input.u32()?, 1);
/// assert_eq!(input.str()?, "M1");
/// input.finish()?;
/// # Ok::<(), taktsim_trace::DecodeError>(())
/// ```
#[derive(Debug, Clone)]
pub struct Decoder<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Decoder<'a> {
    /// A decoder over `bytes`.
    ///
    /// ```
    /// assert_eq!(taktsim_trace::Decoder::new(&[7]).u8()?, 7);
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    #[must_use]
    pub const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    /// Checks that every byte was read.
    ///
    /// # Errors
    ///
    /// [`DecodeError::TrailingBytes`] if bytes are left.
    ///
    /// ```
    /// let mut input = taktsim_trace::Decoder::new(&[1, 2]);
    /// input.u8()?;
    /// assert!(input.finish().is_err());
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    pub fn finish(&self) -> Result<(), DecodeError> {
        let remaining = self.bytes.len() - self.offset;
        if remaining == 0 {
            Ok(())
        } else {
            Err(DecodeError::TrailingBytes {
                offset: self.offset,
                remaining,
            })
        }
    }

    /// The number of bytes read so far.
    ///
    /// ```
    /// let mut input = taktsim_trace::Decoder::new(&[1, 2, 3]);
    /// input.u16()?;
    /// assert_eq!(input.offset(), 2);
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    #[must_use]
    pub const fn offset(&self) -> usize {
        self.offset
    }

    /// An error for a value that is not allowed, e.g. an unknown enum tag,
    /// at the position of the value just read.
    ///
    /// ```
    /// let mut input = taktsim_trace::Decoder::new(&[9]);
    /// let tag = input.u8()?;
    /// let e = input.invalid("Flow tag", tag.into(), "0 or 1");
    /// assert_eq!(e.to_string(), "invalid Flow tag before byte 1: found 9, allowed 0 or 1");
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    #[must_use]
    pub fn invalid(
        &self,
        what: &'static str,
        found: u64,
        allowed: impl fmt::Display,
    ) -> DecodeError {
        DecodeError::Invalid {
            what,
            offset: self.offset,
            found,
            allowed: allowed.to_string(),
        }
    }

    fn take<const N: usize>(&mut self) -> Result<[u8; N], DecodeError> {
        let bytes = self.slice(N)?;
        let mut array = [0; N];
        array.copy_from_slice(bytes);
        Ok(array)
    }

    pub(crate) fn slice(&mut self, len: usize) -> Result<&'a [u8], DecodeError> {
        let remaining = self.bytes.len() - self.offset;
        if len > remaining {
            return Err(DecodeError::UnexpectedEnd {
                offset: self.offset,
                needed: len,
                remaining,
            });
        }
        let start = self.offset;
        self.offset += len;
        Ok(&self.bytes[start..self.offset])
    }

    /// Reads a byte.
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnexpectedEnd`] if no byte is left.
    ///
    /// ```
    /// let mut input = taktsim_trace::Decoder::new(&[]);
    /// assert!(input.u8().is_err());
    /// ```
    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take::<1>()?[0])
    }

    /// Reads a little-endian `u16`.
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnexpectedEnd`] if fewer than 2 bytes are left.
    ///
    /// ```
    /// assert_eq!(taktsim_trace::Decoder::new(&[2, 1]).u16()?, 0x0102);
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        self.take().map(u16::from_le_bytes)
    }

    /// Reads a little-endian `u32`.
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnexpectedEnd`] if fewer than 4 bytes are left.
    ///
    /// ```
    /// assert_eq!(taktsim_trace::Decoder::new(&[1, 0, 0, 0]).u32()?, 1);
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        self.take().map(u32::from_le_bytes)
    }

    /// Reads a little-endian `u64`.
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnexpectedEnd`] if fewer than 8 bytes are left.
    ///
    /// ```
    /// assert_eq!(taktsim_trace::Decoder::new(&[1, 0, 0, 0, 0, 0, 0, 0]).u64()?, 1);
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        self.take().map(u64::from_le_bytes)
    }

    /// Reads a little-endian `i32`.
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnexpectedEnd`] if fewer than 4 bytes are left.
    ///
    /// ```
    /// assert_eq!(taktsim_trace::Decoder::new(&[0xfe, 0xff, 0xff, 0xff]).i32()?, -2);
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    pub fn i32(&mut self) -> Result<i32, DecodeError> {
        self.take().map(i32::from_le_bytes)
    }

    /// Reads a little-endian `i64`.
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnexpectedEnd`] if fewer than 8 bytes are left.
    ///
    /// ```
    /// assert_eq!(taktsim_trace::Decoder::new(&[0xff; 8]).i64()?, -1);
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    pub fn i64(&mut self) -> Result<i64, DecodeError> {
        self.take().map(i64::from_le_bytes)
    }

    /// Reads a `bool`: 0 or 1.
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnexpectedEnd`] if no byte is left,
    /// [`DecodeError::Invalid`] for any byte but 0 and 1.
    ///
    /// ```
    /// assert!(taktsim_trace::Decoder::new(&[1]).bool()?);
    /// assert!(taktsim_trace::Decoder::new(&[2]).bool().is_err());
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    pub fn bool(&mut self) -> Result<bool, DecodeError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            other => Err(self.invalid("bool", other.into(), "0 or 1")),
        }
    }

    /// Reads an `f64` from its bits.
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnexpectedEnd`] if fewer than 8 bytes are left.
    ///
    /// ```
    /// let bytes = 0.1_f64.to_bits().to_le_bytes();
    /// assert_eq!(taktsim_trace::Decoder::new(&bytes).f64()?, 0.1);
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    pub fn f64(&mut self) -> Result<f64, DecodeError> {
        self.u64().map(f64::from_bits)
    }

    /// Reads text written by [`Encoder::str`].
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnexpectedEnd`] if the text is cut off,
    /// [`DecodeError::InvalidUtf8`] if it is not UTF-8.
    ///
    /// ```
    /// assert_eq!(taktsim_trace::Decoder::new(&[2, 0, b'o', b'k']).str()?, "ok");
    /// assert!(taktsim_trace::Decoder::new(&[1, 0, 0xff]).str().is_err());
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    pub fn str(&mut self) -> Result<&'a str, DecodeError> {
        let length = self.u16()?;
        let offset = self.offset;
        let bytes = self.slice(usize::from(length))?;
        std::str::from_utf8(bytes).map_err(|_| DecodeError::InvalidUtf8 { offset })
    }

    /// Reads a simulation time.
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnexpectedEnd`] if fewer than 8 bytes are left.
    ///
    /// ```
    /// let bytes = 1_000_000_000_u64.to_le_bytes();
    /// assert_eq!(taktsim_trace::Decoder::new(&bytes).time()?.to_string(), "1s");
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    pub fn time(&mut self) -> Result<SimTime, DecodeError> {
        self.u64().map(SimTime::from_nanos)
    }

    /// Reads a duration.
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnexpectedEnd`] if fewer than 8 bytes are left.
    ///
    /// ```
    /// let bytes = 2_000_000_000_u64.to_le_bytes();
    /// assert_eq!(taktsim_trace::Decoder::new(&bytes).duration()?.to_string(), "2s");
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    pub fn duration(&mut self) -> Result<SimDuration, DecodeError> {
        self.u64().map(SimDuration::from_nanos)
    }

    /// Reads a component number.
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnexpectedEnd`] if fewer than 4 bytes are left.
    ///
    /// ```
    /// assert_eq!(taktsim_trace::Decoder::new(&[3, 0, 0, 0]).component()?.index(), 3);
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    pub fn component(&mut self) -> Result<ComponentId, DecodeError> {
        let index = self.u32()?;
        // Lossless on every supported (32- and 64-bit) target.
        usize::try_from(index)
            .map(ComponentId::from_index)
            .map_err(|_| {
                self.invalid(
                    "component",
                    index.into(),
                    "a number this platform can index",
                )
            })
    }

    /// Reads an item number.
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnexpectedEnd`] if fewer than 8 bytes are left.
    ///
    /// ```
    /// assert_eq!(taktsim_trace::Decoder::new(&5_u64.to_le_bytes()).item()?.as_u64(), 5);
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    pub fn item(&mut self) -> Result<ItemId, DecodeError> {
        self.u64().map(ItemId::from_raw)
    }

    /// Reads an event's sequence number.
    ///
    /// # Errors
    ///
    /// [`DecodeError::UnexpectedEnd`] if fewer than 8 bytes are left.
    ///
    /// ```
    /// assert_eq!(taktsim_trace::Decoder::new(&6_u64.to_le_bytes()).event()?.as_u64(), 6);
    /// # Ok::<(), taktsim_trace::DecodeError>(())
    /// ```
    pub fn event(&mut self) -> Result<EventId, DecodeError> {
        self.u64().map(EventId::from_raw)
    }
}

/// Bytes that are not a valid value. Offsets count from the start of the
/// message (or record) being read.
///
/// ```
/// use taktsim_trace::DecodeError;
///
/// let e = DecodeError::UnexpectedEnd { offset: 3, needed: 8, remaining: 1 };
/// assert_eq!(e.to_string(), "cut off at byte 3: 8 bytes needed, 1 left");
/// ```
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum DecodeError {
    /// The bytes end inside a value.
    #[error("cut off at byte {offset}: {needed} bytes needed, {remaining} left")]
    UnexpectedEnd {
        /// Where the value starts.
        offset: usize,
        /// Its size.
        needed: usize,
        /// The bytes that were left.
        remaining: usize,
    },
    /// A value that is not allowed, e.g. an unknown tag.
    #[error("invalid {what} before byte {offset}: found {found}, allowed {allowed}")]
    Invalid {
        /// What was read, e.g. `"record kind"`.
        what: &'static str,
        /// Where it ends.
        offset: usize,
        /// The value found.
        found: u64,
        /// What is allowed.
        allowed: String,
    },
    /// Text that is not UTF-8.
    #[error("text at byte {offset} is not UTF-8")]
    InvalidUtf8 {
        /// Where the text starts.
        offset: usize,
    },
    /// Bytes left after the value.
    #[error("{remaining} bytes left over after byte {offset}")]
    TrailingBytes {
        /// Where the value ends.
        offset: usize,
        /// How many bytes follow it.
        remaining: usize,
    },
}
