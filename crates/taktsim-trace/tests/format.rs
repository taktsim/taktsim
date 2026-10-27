//! The trace format: write then read is the identity, and damaged, cut-off
//! or foreign data is reported with a typed error (ADR-0021).

mod support;

use std::io::{self, Write};
use std::sync::{Arc, Mutex};

use proptest::prelude::*;
use support::Toy;
use taktsim_core::{ComponentId, Disposal, EventId, ItemId, RunLimits, TraceSink};
use taktsim_trace::{
    Block, CHUNK_CAPACITY, DecodeError, Decoder, EncodeError, Encoder, FORMAT_VERSION, Header,
    MAGIC, ModelHash, ReadError, Record, TraceHash, TraceMessage, TraceReader, TraceWriter,
    WriteError,
};
use taktsim_units::SimTime;

/// The components every hand-written trace names.
const COMPONENTS: usize = 4;

fn header(seed: u64) -> Header {
    let names = (0..COMPONENTS).map(|i| format!("c{i}")).collect();
    Header::new::<Toy>(ModelHash::sha256(b"format tests"), seed, names)
}

#[allow(clippy::unwrap_used)] // test helper
fn write(header: &Header, records: &[Record<Toy>]) -> (Vec<u8>, taktsim_trace::Summary) {
    let mut writer = TraceWriter::new(Vec::new(), header).unwrap();
    for record in records {
        writer.record(&record.as_trace());
    }
    writer.finish().unwrap()
}

/// Reads every record, then the error that stopped reading, if any.
fn read_all(bytes: &[u8]) -> (Vec<Record<Toy>>, Option<ReadError>) {
    let mut reader = match TraceReader::<_, Toy>::new(bytes) {
        Ok(reader) => reader,
        Err(e) => return (Vec::new(), Some(e)),
    };
    let mut records = Vec::new();
    loop {
        match reader.next_record() {
            Ok(Some(record)) => records.push(record),
            Ok(None) => return (records, None),
            Err(e) => return (records, Some(e)),
        }
    }
}

/// Records compare by `Debug` text, so NaN equals NaN and -0 differs from 0.
fn text(records: &[Record<Toy>]) -> Vec<String> {
    records.iter().map(|r| format!("{r:?}")).collect()
}

fn c(i: usize) -> ComponentId {
    ComponentId::from_index(i)
}

fn event(seq: u64, message: Toy, item: Option<u64>) -> Record<Toy> {
    Record::Event {
        seq,
        time: SimTime::from_nanos(seq * 1_000),
        target: c(1),
        from: c(0),
        message,
        item: item.map(ItemId::from_raw),
    }
}

/// About 6,000 records, so several chunks.
#[allow(clippy::unwrap_used)] // test helper
fn many_records() -> Vec<Record<Toy>> {
    (0..2_000_u64)
        .flat_map(|i| {
            [
                Record::ItemCreated {
                    time: SimTime::from_nanos(i),
                    item: ItemId::from_raw(i),
                    by: c(0),
                },
                event(
                    i,
                    Toy::Done {
                        lot: u32::try_from(i).unwrap(),
                    },
                    Some(i),
                ),
                Record::ItemDisposed {
                    time: SimTime::from_nanos(i),
                    item: ItemId::from_raw(i),
                    by: c(3),
                    how: Disposal::Delivered,
                },
            ]
        })
        .collect()
}

/// Where each block starts: the header's length, then each chunk, then the
/// end marker.
#[allow(clippy::unwrap_used)] // test helper
fn blocks(bytes: &[u8]) -> Vec<usize> {
    let mut reader = TraceReader::<_, Toy>::new(bytes).unwrap();
    let h = reader.header().clone();
    let header_len = 8
        + 2
        + 2
        + h.engine_version.len()
        + 2
        + h.message_format.len()
        + 32
        + 8
        + 4
        + h.components.iter().map(|n| 2 + n.len()).sum::<usize>()
        + 4;
    while reader.next_record().unwrap().is_some() {}
    let mut starts = vec![header_len];
    let mut at = header_len;
    while bytes[at] == b'C' {
        let len = u32::from_le_bytes(bytes[at + 1..at + 5].try_into().unwrap()) as usize;
        at += 1 + 20 + len;
        starts.push(at);
    }
    assert_eq!(bytes[at], b'E');
    starts
}

fn toy() -> impl Strategy<Value = Toy> {
    prop_oneof![
        Just(Toy::Next),
        Just(Toy::Arrive),
        any::<u32>().prop_map(|lot| Toy::Done { lot }),
        Just(Toy::Breakdown),
        Just(Toy::Repaired),
        (".{0,40}", any::<f64>()).prop_map(|(text, share)| Toy::Note { text, share }),
    ]
}

fn record() -> impl Strategy<Value = Record<Toy>> {
    let component = (0..COMPONENTS).prop_map(c);
    let time = any::<u64>().prop_map(SimTime::from_nanos);
    let how = prop_oneof![
        Just(Disposal::Delivered),
        Just(Disposal::Scrapped),
        Just(Disposal::Consumed)
    ];
    prop_oneof![
        (
            any::<u64>(),
            time.clone(),
            component.clone(),
            component.clone(),
            toy(),
            any::<Option<u64>>()
        )
            .prop_map(|(seq, time, target, from, message, item)| Record::Event {
                seq,
                time,
                target,
                from,
                message,
                item: item.map(ItemId::from_raw),
            }),
        (time.clone(), any::<u64>(), component.clone()).prop_map(|(time, item, by)| {
            Record::ItemCreated {
                time,
                item: ItemId::from_raw(item),
                by,
            }
        }),
        (time, any::<u64>(), component, how).prop_map(|(time, item, by, how)| {
            Record::ItemDisposed {
                time,
                item: ItemId::from_raw(item),
                by,
                how,
            }
        }),
    ]
}

proptest! {
    // Up to about two chunks of records per case; fewer cases than the
    // default, since each writes and reads thousands of records.
    #![proptest_config(ProptestConfig::with_cases(64))]

    #[test]
    fn write_then_read_is_the_identity(
        records in prop::collection::vec(record(), 0..3_000),
        seed: u64,
    ) {
        let header = header(seed);
        let (bytes, written) = write(&header, &records);
        prop_assert_eq!(written.bytes, bytes.len() as u64);
        prop_assert_eq!(written.records, records.len() as u64);
        let events = records.iter().filter(|r| matches!(r, Record::Event { .. })).count();
        prop_assert_eq!(written.events, events as u64);

        let mut reader = TraceReader::<_, Toy>::new(bytes.as_slice()).unwrap();
        prop_assert_eq!(reader.header(), &header);
        let read: Vec<_> = reader.by_ref().collect::<Result<_, _>>().unwrap();
        prop_assert_eq!(text(&read), text(&records));
        prop_assert_eq!(reader.summary(), Some(written));
    }

    #[test]
    fn the_stream_hash_depends_on_the_records_only(
        records in prop::collection::vec(record(), 0..200),
        a: u64,
        b: u64,
    ) {
        let (bytes_a, summary_a) = write(&header(a), &records);
        let (bytes_b, summary_b) = write(&header(b), &records);
        prop_assert_eq!(summary_a.hash, summary_b.hash);
        prop_assert_eq!(a == b, bytes_a == bytes_b);
    }

    #[test]
    fn any_changed_byte_is_detected(at in any::<prop::sample::Index>(), mask in 1_u8..) {
        let records = many_records();
        let (mut bytes, _) = write(&header(1), &records);
        let at = at.index(bytes.len());
        bytes[at] ^= mask;
        let (read, error) = read_all(&bytes);
        prop_assert!(error.is_some(), "byte {at} changed, trace read without error");
        // Whatever was returned before the error is intact.
        prop_assert_eq!(text(&read), text(&records[..read.len()]));
    }

    #[test]
    fn any_cut_is_detected(cut in any::<prop::sample::Index>()) {
        let records = many_records();
        let (bytes, _) = write(&header(1), &records);
        let cut = cut.index(bytes.len());
        let (read, error) = read_all(&bytes[..cut]);
        prop_assert!(
            matches!(error, Some(ReadError::Truncated { .. })),
            "cut at {cut}: {error:?}"
        );
        prop_assert_eq!(text(&read), text(&records[..read.len()]));
    }
}

#[test]
fn every_cut_of_a_small_trace_is_truncated_and_keeps_complete_chunks() {
    let records: Vec<_> = (0..3).map(|i| event(i, Toy::Arrive, None)).collect();
    let (bytes, _) = write(&header(1), &records);
    for cut in 0..bytes.len() {
        let (read, error) = read_all(&bytes[..cut]);
        assert!(
            matches!(error, Some(ReadError::Truncated { .. })),
            "cut at {cut}: {error:?}"
        );
        // One chunk: its records come only once it is complete.
        assert!(
            read.is_empty() || read.len() == records.len(),
            "cut at {cut}"
        );
    }
}

#[test]
fn a_trace_spans_chunks_and_the_records_of_complete_chunks_survive_a_cut() {
    let records = many_records();
    let (bytes, summary) = write(&header(1), &records);
    let starts = blocks(&bytes);
    assert!(summary.chunks >= 3, "{summary:?}");
    assert_eq!(starts.len() as u64, summary.chunks + 1);
    for chunk in starts.windows(2) {
        assert!(chunk[1] - chunk[0] <= 1 + 20 + CHUNK_CAPACITY);
    }
    // Cut inside the third chunk: the first two chunks' records are read.
    let (read, error) = read_all(&bytes[..starts[2] + 30]);
    let Some(ReadError::Truncated {
        offset,
        records: complete,
    }) = error
    else {
        panic!("{error:?}");
    };
    assert_eq!(offset, starts[2] as u64);
    assert_eq!(complete, read.len() as u64);
    assert!(complete > 0);
    let message = ReadError::Truncated {
        offset,
        records: complete,
    }
    .to_string();
    assert!(message.contains("has no end marker"), "{message}");
}

#[test]
fn a_damaged_chunk_is_named_with_its_checksums() {
    let records = many_records();
    let (mut bytes, _) = write(&header(1), &records);
    let starts = blocks(&bytes);
    bytes[starts[1] + 40] ^= 0x10;
    let (read, error) = read_all(&bytes);
    let Some(ReadError::Checksum {
        block,
        offset,
        stored,
        computed,
    }) = error
    else {
        panic!("{error:?}");
    };
    assert_eq!(block, Block::Chunk(1));
    assert_eq!(offset, starts[1] as u64);
    assert_ne!(stored, computed);
    // Chunk 0 was returned whole before the error, and nothing of chunk 1.
    let in_chunk_0 = u32::from_le_bytes(bytes[starts[0] + 5..starts[0] + 9].try_into().unwrap());
    assert_eq!(read.len(), in_chunk_0 as usize);
    assert_eq!(text(&read), text(&records[..read.len()]));
}

#[test]
fn a_damaged_header_or_end_marker_is_named() {
    let (bytes, _) = write(&header(1), &many_records());
    let starts = blocks(&bytes);
    let mut damaged = bytes.clone();
    damaged[starts[0] - 6] ^= 1; // a byte of the last component id
    assert!(matches!(
        TraceReader::<_, Toy>::new(damaged.as_slice()),
        Err(ReadError::Checksum {
            block: Block::Header,
            ..
        })
    ));
    let mut damaged = bytes.clone();
    let end = *starts.last().unwrap();
    damaged[end + 3] ^= 1;
    let (_, error) = read_all(&damaged);
    assert!(
        matches!(
            error,
            Some(ReadError::Checksum {
                block: Block::End,
                ..
            })
        ),
        "{error:?}"
    );
}

#[test]
fn a_missing_or_repeated_chunk_is_out_of_sequence() {
    let records = many_records();
    let (bytes, _) = write(&header(1), &records);
    let s = blocks(&bytes);
    // Chunk 1 removed.
    let removed = [&bytes[..s[1]], &bytes[s[2]..]].concat();
    let (_, error) = read_all(&removed);
    assert!(
        matches!(error, Some(ReadError::OutOfSequence { chunk: 1, .. })),
        "{error:?}"
    );
    // Chunk 0 repeated.
    let repeated = [&bytes[..s[1]], &bytes[s[0]..]].concat();
    let (_, error) = read_all(&repeated);
    let Some(
        e @ ReadError::OutOfSequence {
            expected, found: 0, ..
        },
    ) = error
    else {
        panic!("{error:?}");
    };
    assert!(expected > 0);
    assert!(e.to_string().contains("out of sequence"), "{e}");
    // The last chunk removed: the end marker's counts do not match.
    let n = s.len();
    let last_removed = [&bytes[..s[n - 2]], &bytes[s[n - 1]..]].concat();
    let (_, error) = read_all(&last_removed);
    let Some(
        e @ ReadError::EndMismatch {
            what: "records", ..
        },
    ) = error
    else {
        panic!("{error:?}");
    };
    assert!(
        e.to_string()
            .starts_with("the end marker states 6000 records, the trace has"),
        "{e}"
    );
}

#[test]
fn data_after_the_end_marker_is_rejected() {
    let (mut bytes, _) = write(&header(1), &many_records());
    let len = bytes.len() as u64;
    bytes.push(0);
    let (_, error) = read_all(&bytes);
    assert!(
        matches!(error, Some(ReadError::TrailingData { offset }) if offset == len),
        "{error:?}"
    );
}

#[test]
fn another_format_version_says_what_was_found_and_what_is_allowed() {
    let (mut bytes, _) = write(&header(1), &[]);
    bytes[8..10].copy_from_slice(&2_u16.to_le_bytes());
    let Err(e) = TraceReader::<_, Toy>::new(bytes.as_slice()) else {
        panic!("version 2 was read");
    };
    assert!(matches!(
        e,
        ReadError::UnsupportedVersion {
            found: 2,
            supported: FORMAT_VERSION
        }
    ));
    assert_eq!(
        e.to_string(),
        "trace format version 2 is not supported: this build reads version 1 only; \
         read the trace with the taktsim release that wrote it"
    );
}

#[test]
fn other_data_is_not_a_trace() {
    let Err(e) = TraceReader::<_, Toy>::new(&b"timestamp,entity,signal\n"[..]) else {
        panic!("CSV was read as a trace");
    };
    assert!(matches!(&e, ReadError::NotATrace { found } if found == b"timestam"));
    assert!(e.to_string().starts_with("not a taktsim trace"), "{e}");
    // Line endings converted in transfer (CR LF → LF) are caught by the magic.
    let mut converted = MAGIC.to_vec();
    converted.remove(4);
    converted.extend_from_slice(&[0; 32]);
    assert!(matches!(
        TraceReader::<_, Toy>::new(converted.as_slice()),
        Err(ReadError::NotATrace { .. })
    ));
    // Empty data is a trace whose writer never got going.
    assert!(matches!(
        TraceReader::<_, Toy>::new(&[][..]),
        Err(ReadError::Truncated {
            offset: 0,
            records: 0
        })
    ));
}

/// Messages of another component package.
#[derive(Debug)]
struct Other;
impl taktsim_core::Message for Other {}
impl TraceMessage for Other {
    const FORMAT: &'static str = "other/1";
    fn encode(&self, _: &mut Encoder<'_>) {}
    fn decode(_: &mut Decoder<'_>) -> Result<Self, DecodeError> {
        Ok(Other)
    }
}

#[test]
fn a_trace_is_read_only_with_its_own_message_type() {
    let (bytes, _) = write(&header(1), &[]);
    let Err(e) = TraceReader::<_, Other>::new(bytes.as_slice()) else {
        panic!("read with the wrong message type");
    };
    assert_eq!(
        e.to_string(),
        "the trace's messages are encoded as `taktsim-trace.toy/1`, but they are read as `other/1`; \
         read it with the message type of the component package that wrote it"
    );
    let result = TraceWriter::<_, Other>::new(Vec::new(), &header(1));
    assert!(matches!(result, Err(WriteError::MessageFormat { .. })));
}

#[test]
fn the_reader_reports_a_failure_once_then_stops() {
    let (bytes, _) = write(&header(1), &many_records());
    let cut = &bytes[..bytes.len() - 1];
    let mut reader = TraceReader::<_, Toy>::new(cut).unwrap();
    let results: Vec<_> = reader.by_ref().collect();
    assert!(matches!(
        results.last(),
        Some(Err(ReadError::Truncated { .. }))
    ));
    assert_eq!(results.iter().filter(|r| r.is_err()).count(), 1);
    assert!(matches!(
        reader.next_record(),
        Err(ReadError::AlreadyFailed)
    ));
    assert_eq!(reader.summary(), None);
}

#[test]
fn every_nan_is_written_as_the_same_bits() {
    let note = |share| {
        event(
            0,
            Toy::Note {
                text: String::new(),
                share,
            },
            None,
        )
    };
    let (a, _) = write(&header(1), &[note(f64::NAN)]);
    let (b, _) = write(&header(1), &[note(-f64::NAN)]);
    let (c, _) = write(&header(1), &[note(f64::from_bits(0x7ff0_0000_0000_0001))]);
    assert_eq!(a, b);
    assert_eq!(a, c);
    let (zero, _) = write(&header(1), &[note(0.0)]);
    let (minus_zero, _) = write(&header(1), &[note(-0.0)]);
    assert_ne!(zero, minus_zero);
}

/// A writer that accepts `limit` bytes, then fails.
#[derive(Clone)]
struct Failing {
    written: Arc<Mutex<Vec<u8>>>,
    limit: usize,
}

impl Write for Failing {
    #[allow(clippy::unwrap_used)] // test helper
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let mut written = self.written.lock().unwrap();
        if written.len() + buf.len() > self.limit {
            return Err(io::Error::other("disk full"));
        }
        written.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

#[test]
fn a_failed_write_is_reported_at_finish_and_leaves_a_truncated_trace() {
    let out = Failing {
        written: Arc::default(),
        limit: 100_000,
    };
    let mut writer = TraceWriter::new(out.clone(), &header(1)).unwrap();
    for record in many_records() {
        writer.record(&record.as_trace());
    }
    let Err(e) = writer.finish() else {
        panic!("a full disk went unnoticed");
    };
    assert!(matches!(e, WriteError::Io { .. }));
    assert!(e.to_string().contains("disk full"), "{e}");
    let bytes = out.written.lock().unwrap().clone();
    let (read, error) = read_all(&bytes);
    assert!(
        matches!(error, Some(ReadError::Truncated { .. })),
        "{error:?}"
    );
    assert!(read.len() > 1);

    // The header itself cannot be written.
    let out = Failing {
        written: Arc::default(),
        limit: 10,
    };
    assert!(matches!(
        TraceWriter::<_, Toy>::new(out, &header(1)),
        Err(WriteError::Io { offset: 0, .. })
    ));
}

#[test]
fn records_that_do_not_fit_are_reported_at_finish() {
    let cases = [
        (
            event(
                0,
                Toy::Note {
                    text: "x".repeat(70_000),
                    share: 0.0,
                },
                None,
            ),
            "does not fit the format: text of 70000 bytes does not fit a trace: at most 65535 bytes are allowed",
        ),
        (
            event(
                0,
                Toy::Note {
                    text: "x".repeat(65_500),
                    share: 0.0,
                },
                None,
            ),
            "does not fit the format: a record of 65547 bytes does not fit a trace: at most 65536 bytes are allowed",
        ),
        (
            Record::ItemCreated {
                time: SimTime::START,
                item: ItemId::from_raw(0),
                by: c(COMPONENTS),
            },
            "names component #4, but the header lists 4 components",
        ),
    ];
    for (bad, expected) in cases {
        let mut writer = TraceWriter::new(Vec::new(), &header(1)).unwrap();
        writer.record(&event(0, Toy::Arrive, None).as_trace());
        writer.record(&bad.as_trace());
        // Later records are dropped.
        writer.record(&event(1, Toy::Arrive, None).as_trace());
        let Err(e) = writer.finish() else {
            panic!("{bad:?} was written");
        };
        assert_eq!(e.to_string(), format!("record 1 {expected}"));
    }
    // The largest record that fits is written.
    let largest = event(
        0,
        Toy::Note {
            text: "x".repeat(CHUNK_CAPACITY - 36 - 1 - 2 - 8),
            share: 0.0,
        },
        None,
    );
    let (bytes, summary) = write(
        &header(1),
        &[event(0, Toy::Arrive, None), largest.clone(), largest],
    );
    assert_eq!(summary.chunks, 3);
    assert!(read_all(&bytes).1.is_none());
}

#[test]
fn header_values_that_do_not_fit_are_rejected() {
    let mut long = header(1);
    long.engine_version = "1".repeat(70_000);
    assert!(matches!(
        TraceWriter::<_, Toy>::new(Vec::new(), &long),
        Err(WriteError::Header(EncodeError::TooLong { bytes: 70_000 }))
    ));
}

#[test]
fn an_engine_run_streams_into_the_writer() {
    let mut engine = support::single_server(3).unwrap();
    let header = Header::for_engine(&engine, ModelHash::sha256(b"single server"));
    assert_eq!(header.components, ["source", "station", "sink"]);
    let mut writer = TraceWriter::new(Vec::new(), &header).unwrap();
    engine
        .run(RunLimits::until("8h".parse().unwrap()), &mut writer)
        .unwrap();
    let (bytes, summary) = writer.finish().unwrap();
    let (read, error) = read_all(&bytes);
    assert!(error.is_none());
    assert_eq!(read.len() as u64, summary.records);
    let counts = engine.item_counts();
    let created = read
        .iter()
        .filter(|r| matches!(r, Record::ItemCreated { .. }))
        .count();
    assert_eq!(created as u64, counts.created.get());
    // Same run, same bytes.
    let mut again = support::single_server(3).unwrap();
    let mut writer = TraceWriter::new(Vec::new(), &header).unwrap();
    again
        .run(RunLimits::until("8h".parse().unwrap()), &mut writer)
        .unwrap();
    assert_eq!(writer.finish().unwrap().0, bytes);
}

#[test]
fn records_display_as_one_line_each() {
    let names: Vec<String> = ["source", "sink"].map(String::from).to_vec();
    let lines: Vec<String> = [
        Record::Event {
            seq: 4,
            time: "12s".parse().unwrap(),
            target: c(1),
            from: c(0),
            message: Toy::Arrive,
            item: Some(ItemId::from_raw(0)),
        },
        Record::ItemDisposed {
            time: "1.5s".parse().unwrap(),
            item: ItemId::from_raw(2),
            by: c(1),
            how: Disposal::Scrapped,
        },
        Record::ItemDisposed {
            time: "2s".parse().unwrap(),
            item: ItemId::from_raw(3),
            by: c(7),
            how: Disposal::Consumed,
        },
        Record::ItemDisposed {
            time: "3s".parse().unwrap(),
            item: ItemId::from_raw(4),
            by: c(1),
            how: Disposal::Delivered,
        },
    ]
    .iter()
    .map(|r| r.display(&names).to_string())
    .collect();
    assert_eq!(
        lines,
        [
            "12s event #4 source -> sink Arrive, item 0",
            "1.5s item 2 scrapped by sink",
            "2s item 3 consumed by #7",
            "3s item 4 delivered by sink",
        ]
    );
    assert_eq!(lines.len(), 4);
    let _ = (TraceHash::default(), EventId::from_raw(0));
}

#[test]
fn decode_errors_name_the_value_and_position() {
    let mut input = Decoder::new(&[1, 2]);
    assert_eq!(
        input.u32().unwrap_err().to_string(),
        "cut off at byte 0: 4 bytes needed, 2 left"
    );
    let mut input = Decoder::new(&[2, 0, 0xc3, 0x28]);
    assert_eq!(
        input.str().unwrap_err(),
        DecodeError::InvalidUtf8 { offset: 2 }
    );
    assert_eq!(
        DecodeError::InvalidUtf8 { offset: 2 }.to_string(),
        "text at byte 2 is not UTF-8"
    );
    assert_eq!(
        DecodeError::TrailingBytes {
            offset: 1,
            remaining: 2
        }
        .to_string(),
        "2 bytes left over after byte 1"
    );
    let mut input = Decoder::new(&[0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x7f]);
    assert_eq!(input.i64().unwrap(), i64::MAX);
    let mut bytes = Vec::new();
    let mut out = Encoder::new(&mut bytes);
    out.i32(i32::MIN);
    out.duration("1s".parse().unwrap());
    out.event(EventId::from_raw(9));
    out.finish().unwrap();
    let mut input = Decoder::new(&bytes);
    assert_eq!(input.i32().unwrap(), i32::MIN);
    assert_eq!(input.duration().unwrap().to_string(), "1s");
    assert_eq!(input.event().unwrap(), EventId::from_raw(9));
    assert_eq!(
        EncodeError::ComponentOutOfRange { index: 1 << 33 }.to_string(),
        "component number 8589934592 does not fit a trace: at most 4294967295 is allowed"
    );
}
