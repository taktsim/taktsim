//! Traces that no correct writer produces, built by hand with valid
//! checksums, and sources that fail: the reader rejects each with a typed
//! error naming what is wrong (ADR-0021).

mod support;

use std::io::{self, Read};

use sha2::{Digest, Sha256};
use support::Toy;
use taktsim_core::Message;
use taktsim_trace::{
    Block, DecodeError, Decoder, Encoder, Header, ModelHash, ReadError, Record, TraceMessage,
    TraceReader, TraceWriter, decode_message, encode_message,
};

/// The header of a trace with components `a` and `b`.
#[allow(clippy::unwrap_used)] // test helper
fn header_bytes() -> Vec<u8> {
    let header = Header::new::<Toy>(ModelHash::default(), 1, vec!["a".into(), "b".into()]);
    let (bytes, _) = TraceWriter::<_, Toy>::new(Vec::new(), &header)
        .unwrap()
        .finish()
        .unwrap();
    // Everything before the end marker (1 + 60 bytes).
    bytes[..bytes.len() - 61].to_vec()
}

/// A chunk with a correct CRC.
#[allow(clippy::unwrap_used)] // test helper
fn chunk(payload: &[u8], records: u32, first: u64) -> Vec<u8> {
    let mut head = vec![b'C'];
    head.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
    head.extend_from_slice(&records.to_le_bytes());
    head.extend_from_slice(&first.to_le_bytes());
    let mut crc = crc32fast::Hasher::new();
    crc.update(&head);
    crc.update(payload);
    head.extend_from_slice(&crc.finalize().to_le_bytes());
    head.extend_from_slice(payload);
    head
}

/// An end marker with a correct CRC.
fn end(records: u64, events: u64, chunks: u64, hash: [u8; 32]) -> Vec<u8> {
    let mut end = vec![b'E'];
    for n in [records, events, chunks] {
        end.extend_from_slice(&n.to_le_bytes());
    }
    end.extend_from_slice(&hash);
    let crc = crc32fast::hash(&end);
    end.extend_from_slice(&crc.to_le_bytes());
    end
}

/// An item-created record by component `by`.
fn created(by: u32) -> Vec<u8> {
    let mut r = vec![2];
    r.extend_from_slice(&7_u64.to_le_bytes());
    r.extend_from_slice(&3_u64.to_le_bytes());
    r.extend_from_slice(&by.to_le_bytes());
    r
}

/// An event record from `a` to `b` with `message` and the raw item fields.
#[allow(clippy::unwrap_used)] // test helper
fn event(has_item: u8, item: u64, message: &[u8]) -> Vec<u8> {
    let mut r = vec![1];
    r.extend_from_slice(&0_u64.to_le_bytes());
    r.extend_from_slice(&5_u64.to_le_bytes());
    r.extend_from_slice(&1_u32.to_le_bytes());
    r.extend_from_slice(&0_u32.to_le_bytes());
    r.push(has_item);
    r.extend_from_slice(&item.to_le_bytes());
    r.extend_from_slice(&u16::try_from(message.len()).unwrap().to_le_bytes());
    r.extend_from_slice(message);
    r
}

/// A whole trace of one chunk holding `payload`.
fn trace(payload: &[u8], records: u32) -> Vec<u8> {
    let hash: [u8; 32] = Sha256::digest(payload).into();
    [
        header_bytes(),
        chunk(payload, records, 0),
        end(u64::from(records), 0, 1, hash),
    ]
    .concat()
}

#[allow(clippy::unwrap_used, clippy::panic)] // test helper
fn first_error(bytes: &[u8]) -> ReadError {
    let mut reader = TraceReader::<_, Toy>::new(bytes).unwrap();
    loop {
        match reader.next_record() {
            Ok(Some(_)) => {}
            Ok(None) => panic!("read without error"),
            Err(e) => return e,
        }
    }
}

#[test]
fn a_hand_made_trace_reads() {
    let payload = created(1);
    let bytes = trace(&payload, 1);
    let records: Vec<Record<Toy>> = TraceReader::new(bytes.as_slice())
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].time().as_nanos(), 7);
}

#[test]
fn invalid_records_in_an_intact_chunk_are_named() {
    let bad_kind = vec![9; 10];
    let mut bad_disposal = created(0);
    bad_disposal[0] = 3;
    bad_disposal.push(7);
    let cases: [(Vec<u8>, &str); 7] = [
        (
            bad_kind,
            "invalid record kind before byte 1: found 9, allowed 1, 2 or 3",
        ),
        (
            created(2),
            "invalid component before byte 21: found 2, allowed below 2, the number of components in the header",
        ),
        (
            bad_disposal,
            "invalid disposal before byte 22: found 7, allowed 0 (delivered), 1 (scrapped) or 2 (consumed)",
        ),
        (
            event(0, 4, &[0]),
            "invalid item number of an event without an item before byte 34: found 4, allowed 0",
        ),
        (
            event(2, 0, &[0]),
            "invalid bool before byte 26: found 2, allowed 0 or 1",
        ),
        (
            event(0, 0, &[9]),
            "invalid Toy tag before byte 1: found 9, allowed 0 to 5",
        ),
        (
            event(0, 0, &[2, 1]),
            "cut off at byte 1: 4 bytes needed, 1 left",
        ),
    ];
    for (record, expected) in cases {
        let e = first_error(&trace(&record, 1));
        let ReadError::Record { record: 0, source } = &e else {
            panic!("{e}");
        };
        assert_eq!(source.to_string(), expected);
        assert!(e.to_string().starts_with("record 0 is invalid: "));
    }
    // A record cut off by the end of its chunk.
    let e = first_error(&trace(&created(0)[..10], 1));
    assert!(
        matches!(
            e,
            ReadError::Record {
                source: DecodeError::UnexpectedEnd { .. },
                ..
            }
        ),
        "{e}"
    );
}

#[test]
fn a_chunk_must_hold_exactly_the_records_it_counts() {
    let two = [created(0), created(1)].concat();
    let e = first_error(&trace(&two, 1));
    assert!(
        e.to_string().ends_with(
            "found more records than its header says, allowed exactly the records its header counts"
        ),
        "{e}"
    );
    let e = first_error(&trace(&created(0), 2));
    assert!(
        e.to_string()
            .contains("found 1 records fewer than its header says"),
        "{e}"
    );
    let e = first_error(&trace(&created(0), 0));
    assert!(
        matches!(e, ReadError::OutOfSequence { records: 0, .. }),
        "{e}"
    );
}

#[test]
fn chunk_lengths_and_block_tags_are_checked() {
    let header = header_bytes();
    let mut empty = chunk(&[], 1, 0);
    empty.truncate(21);
    let e = first_error(&[header.clone(), empty].concat());
    assert_eq!(
        e.to_string(),
        format!(
            "invalid chunk length at byte {}: found 0, allowed 1 to 65536 bytes",
            header.len() + 1
        )
    );
    let mut huge = chunk(&[0], 1, 0);
    huge[1..5].copy_from_slice(&70_000_u32.to_le_bytes());
    let e = first_error(&[header.clone(), huge].concat());
    assert!(
        e.to_string()
            .contains("found 70000, allowed 1 to 65536 bytes"),
        "{e}"
    );
    let e = first_error(&[header.clone(), vec![b'X']].concat());
    assert_eq!(
        e.to_string(),
        format!(
            "invalid block tag at byte {}: found 0x58, allowed 0x43 (chunk) or 0x45 (end marker)",
            header.len()
        )
    );
}

#[test]
fn the_end_marker_must_match_the_stream() {
    let payload = created(0);
    let wrong_hash = [header_bytes(), chunk(&payload, 1, 0), end(1, 0, 1, [0; 32])].concat();
    let e = first_error(&wrong_hash);
    assert!(
        matches!(
            e,
            ReadError::EndMismatch {
                what: "stream hash",
                ..
            }
        ),
        "{e}"
    );
    let hash: [u8; 32] = Sha256::digest(&payload).into();
    let wrong_events = [header_bytes(), chunk(&payload, 1, 0), end(1, 5, 1, hash)].concat();
    assert_eq!(
        first_error(&wrong_events).to_string(),
        "the end marker states 5 events, the trace has 0"
    );
}

#[test]
fn header_text_must_be_utf8() {
    let mut header = header_bytes();
    let n = header.len();
    // The last component id, `b`, just before the CRC.
    header[n - 5] = 0xff;
    let crc = crc32fast::hash(&header[..n - 4]);
    header[n - 4..].copy_from_slice(&crc.to_le_bytes());
    let Err(e) = TraceReader::<_, Toy>::new(header.as_slice()) else {
        panic!("invalid UTF-8 was read");
    };
    assert!(
        e.to_string().starts_with("invalid component id at byte "),
        "{e}"
    );
    assert!(
        e.to_string()
            .ends_with("found bytes that are not UTF-8, allowed UTF-8 text"),
        "{e}"
    );
}

/// Returns `data`, interrupted before every read, then fails after `fail_at`
/// bytes if set.
struct Flaky<'a> {
    data: &'a [u8],
    interrupt: bool,
    fail_at: Option<usize>,
    read: usize,
}

impl Read for Flaky<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.interrupt = !self.interrupt;
        if self.interrupt {
            return Err(io::ErrorKind::Interrupted.into());
        }
        if self.fail_at.is_some_and(|at| self.read >= at) {
            return Err(io::Error::other("device gone"));
        }
        let n = buf.len().min(self.data.len()).min(7);
        buf[..n].copy_from_slice(&self.data[..n]);
        self.data = &self.data[n..];
        self.read += n;
        Ok(n)
    }
}

#[test]
fn interrupted_reads_are_retried_and_failing_sources_reported() {
    let bytes = trace(&created(1), 1);
    let flaky = Flaky {
        data: &bytes,
        interrupt: false,
        fail_at: None,
        read: 0,
    };
    let records: Vec<_> = TraceReader::<_, Toy>::new(flaky).unwrap().collect();
    assert_eq!(records.len(), 1);
    for fail_at in [0, 3, 30, bytes.len() - 1, bytes.len()] {
        let flaky = Flaky {
            data: &bytes,
            interrupt: false,
            fail_at: Some(fail_at),
            read: 0,
        };
        let error = match TraceReader::<_, Toy>::new(flaky) {
            Err(e) => e,
            Ok(mut reader) => loop {
                match reader.next_record() {
                    Ok(Some(_)) => {}
                    Ok(None) => panic!("read past a failing source at {fail_at}"),
                    Err(e) => break e,
                }
            },
        };
        assert!(matches!(error, ReadError::Io { .. }), "{fail_at}: {error}");
        assert!(error.to_string().contains("device gone"), "{error}");
    }
}

#[test]
fn blocks_are_named() {
    assert_eq!(Block::Header.to_string(), "the header");
    assert_eq!(Block::End.to_string(), "the end marker");
    let e = ReadError::Checksum {
        block: Block::Chunk(2),
        offset: 9,
        stored: 1,
        computed: 2,
    };
    assert_eq!(
        e.to_string(),
        "chunk 2 at byte 9 is damaged: its CRC32 is 0x00000001, its bytes give 0x00000002"
    );
    assert_eq!(
        ReadError::AlreadyFailed.to_string(),
        "the trace reader already failed"
    );
}

#[test]
fn message_helpers_round_trip_and_reject() {
    let note = Toy::Note {
        text: "über".into(),
        share: -1.5,
    };
    let bytes = encode_message(&note).unwrap();
    assert_eq!(decode_message::<Toy>(&bytes).unwrap(), note);
    let mut longer = bytes.clone();
    longer.push(0);
    assert_eq!(
        decode_message::<Toy>(&longer).unwrap_err(),
        DecodeError::TrailingBytes {
            offset: bytes.len(),
            remaining: 1
        }
    );
    let huge = Toy::Note {
        text: "x".repeat(70_000),
        share: 0.0,
    };
    assert!(encode_message(&huge).is_err());
    assert_eq!(Toy::Breakdown.priority(), 0);

    let mut out = Vec::new();
    let mut encoder = Encoder::new(&mut out);
    encoder.i64(-3);
    encoder.bool(false);
    encoder.finish().unwrap();
    let mut input = Decoder::new(&out);
    assert_eq!(input.i64().unwrap(), -3);
    assert_eq!(input.offset(), 8);
    assert!(!input.bool().unwrap());
    input.finish().unwrap();
}

/// A message type whose encoding is broken: it writes a component number
/// that does not fit.
#[derive(Debug)]
struct Far;
impl Message for Far {}
impl TraceMessage for Far {
    const FORMAT: &'static str = "far/1";
    fn encode(&self, out: &mut Encoder<'_>) {
        out.component(taktsim_core::ComponentId::from_index(usize::MAX));
    }
    fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(Far)
    }
}

#[test]
fn values_that_do_not_fit_are_refused() {
    let Err(e) = encode_message(&Far) else {
        panic!("a component number above u32::MAX was encoded");
    };
    assert!(e.to_string().starts_with("component number "), "{e}");
}
