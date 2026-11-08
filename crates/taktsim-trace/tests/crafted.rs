//! Traces that no correct writer produces, built by hand with valid
//! checksums, and sources that fail: the reader rejects each with a typed
//! error naming what is wrong (ADR-0021).

mod support;

use std::io::{self, Read};

use sha2::{Digest, Sha256};
use support::Toy;
use taktsim_core::Message;
use taktsim_trace::{
    Block, DecodeError, Decoder, Encoder, Header, ModelHash, ReadError, Record, SystemHash,
    TraceMessage, TraceReader, TraceWriter, decode_message, encode_message,
};

/// The header of a trace with components `a` and `b`.
#[allow(clippy::unwrap_used)] // test helper
fn header_bytes() -> Vec<u8> {
    let header = Header::new::<Toy>(
        ModelHash::default(),
        SystemHash::default(),
        1,
        vec!["a".into(), "b".into()],
    );
    let (bytes, _) = TraceWriter::<_, Toy>::new(Vec::new(), &header)
        .unwrap()
        .finish()
        .unwrap();
    // Everything before the end marker (1 + 70 bytes).
    bytes[..bytes.len() - 71].to_vec()
}

/// A chunk with a correct CRC; its first time is that of the first record
/// in `payload` (7 ns in every hand-made record here), or 0 if empty.
#[allow(clippy::unwrap_used)] // test helper
fn chunk(payload: &[u8], records: u32, first: u64) -> Vec<u8> {
    let mut head = vec![b'C'];
    head.extend_from_slice(&u32::try_from(payload.len()).unwrap().to_le_bytes());
    head.extend_from_slice(&records.to_le_bytes());
    head.extend_from_slice(&first.to_le_bytes());
    let first_time: u64 = if payload.len() >= 11 {
        u64::from_le_bytes(payload[3..11].try_into().unwrap())
    } else {
        0
    };
    head.extend_from_slice(&first_time.to_le_bytes());
    let mut crc = crc32fast::Hasher::new();
    crc.update(&head);
    crc.update(payload);
    head.extend_from_slice(&crc.finalize().to_le_bytes());
    head.extend_from_slice(payload);
    head
}

/// An end marker with a correct CRC, stating no end of the run.
fn end(records: u64, events: u64, chunks: u64, hash: [u8; 32]) -> Vec<u8> {
    end_stating(records, events, chunks, hash, (0, 0, 0))
}

/// An end marker with a correct CRC and the given has-end, time and outcome.
fn end_stating(
    records: u64,
    events: u64,
    chunks: u64,
    hash: [u8; 32],
    (has_end, time, outcome): (u8, u64, u8),
) -> Vec<u8> {
    let mut end = vec![b'E'];
    for n in [records, events, chunks] {
        end.extend_from_slice(&n.to_le_bytes());
    }
    end.extend_from_slice(&hash);
    end.push(has_end);
    end.extend_from_slice(&time.to_le_bytes());
    end.push(outcome);
    let crc = crc32fast::hash(&end);
    end.extend_from_slice(&crc.to_le_bytes());
    end
}

/// Prefixes `body` with its kind and length: the record prefix every
/// kind shares (the time is the first field of `body`).
#[allow(clippy::unwrap_used)] // test helper
fn framed(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut r = vec![kind];
    r.extend_from_slice(&u16::try_from(body.len()).unwrap().to_le_bytes());
    r.extend_from_slice(body);
    r
}

/// An item-created record by component `by`, at 7 ns.
fn created(by: u32) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&7_u64.to_le_bytes());
    body.extend_from_slice(&3_u64.to_le_bytes());
    body.extend_from_slice(&by.to_le_bytes());
    framed(2, &body)
}

/// An event record from `a` to `b` at 7 ns with `message` and the raw item
/// fields.
fn event(has_item: u8, item: u64, message: &[u8]) -> Vec<u8> {
    let mut body = Vec::new();
    body.extend_from_slice(&7_u64.to_le_bytes());
    body.extend_from_slice(&0_u64.to_le_bytes());
    body.extend_from_slice(&1_u32.to_le_bytes());
    body.extend_from_slice(&0_u32.to_le_bytes());
    body.push(has_item);
    body.extend_from_slice(&item.to_le_bytes());
    body.extend_from_slice(message);
    framed(1, &body)
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
    // A kind this build does not know, framed correctly: its end is found,
    // its decoding refused.
    let bad_kind = framed(9, &[7, 0, 0, 0, 0, 0, 0, 0, 1, 2]);
    let mut bad_disposal = created(0);
    bad_disposal[0] = 3;
    bad_disposal.push(7);
    bad_disposal[1..3].copy_from_slice(&21_u16.to_le_bytes());
    let cases: [(Vec<u8>, &str); 7] = [
        (
            bad_kind,
            "invalid record kind before byte 1: found 9, allowed 1, 2, 3 or 4 (taktsim-core/2)",
        ),
        (
            created(2),
            "invalid component before byte 23: found 2, allowed below 2, the number of components in the header",
        ),
        (
            bad_disposal,
            "invalid disposal before byte 24: found 7, allowed 0 (delivered), 1 (scrapped) or 2 (consumed)",
        ),
        (
            event(0, 4, &[0]),
            "invalid item number of an event without an item before byte 36: found 4, allowed 0",
        ),
        (
            event(2, 0, &[0]),
            "invalid bool before byte 28: found 2, allowed 0 or 1",
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
    // A record whose length runs past the end of its chunk, and one whose
    // length stops short of it: the chunk does not add up.
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
    empty.truncate(29);
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

#[test]
fn a_note_naming_an_unknown_component_is_refused() {
    // Kind 4: time, by u32, message.
    let mut body = 7_u64.to_le_bytes().to_vec();
    body.extend_from_slice(&5_u32.to_le_bytes());
    body.push(0);
    let e = first_error(&trace(&framed(4, &body), 1));
    let ReadError::Record { source, .. } = &e else {
        panic!("{e}");
    };
    assert_eq!(
        source.to_string(),
        "invalid component before byte 15: found 5, allowed below 2, the number of components in the header"
    );
}

#[test]
fn a_hand_made_note_reads() {
    let mut body = 7_u64.to_le_bytes().to_vec();
    body.extend_from_slice(&1_u32.to_le_bytes());
    body.push(0);
    let bytes = trace(&framed(4, &body), 1);
    let mut reader = TraceReader::<_, Toy>::new(bytes.as_slice()).unwrap();
    let record = reader.next_record().unwrap().unwrap();
    assert_eq!(
        record,
        Record::Note {
            time: taktsim_units::SimTime::from_nanos(7),
            by: taktsim_core::ComponentId::from_index(1),
            message: Toy::Next,
        }
    );
    assert_eq!(record.time(), taktsim_units::SimTime::from_nanos(7));
}

#[test]
fn the_stated_end_of_a_run_is_checked() {
    let empty: [u8; 32] = Sha256::digest([]).into();
    let read = |stated| {
        let mut bytes = header_bytes();
        bytes.extend(end_stating(0, 0, 0, empty, stated));
        let mut reader = TraceReader::<_, Toy>::new(bytes.as_slice()).unwrap();
        reader.next_record().map(|_| reader.summary().unwrap().end)
    };
    assert_eq!(read((0, 0, 0)).unwrap(), None);
    for (code, outcome) in [
        (0, taktsim_core::RunOutcome::Horizon),
        (1, taktsim_core::RunOutcome::Idle),
        (2, taktsim_core::RunOutcome::EventLimit),
        (3, taktsim_core::RunOutcome::Cancelled),
    ] {
        let time = taktsim_units::SimTime::from_nanos(9);
        assert_eq!(
            read((1, 9, code)).unwrap(),
            Some(taktsim_trace::RunEnd { time, outcome })
        );
    }
    for (stated, message) in [
        ((2, 0, 0), "invalid has end"),
        ((1, 0, 4), "invalid run outcome"),
        ((0, 5, 0), "invalid end of a trace that states none"),
        ((0, 0, 1), "invalid end of a trace that states none"),
    ] {
        let e = read(stated).unwrap_err();
        assert!(e.to_string().starts_with(message), "{e}");
    }
}
