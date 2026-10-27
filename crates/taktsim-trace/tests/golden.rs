//! Golden traces of the toy models (spec §7 Layer 2, ADR-0021): each run
//! must reproduce its committed snapshot, stream hash included, on every
//! tier-1 target. CI also collects every case's hashes from every target and
//! fails if one differs (`TAKTSIM_TRACE_HASHES`, A4).
//!
//! A snapshot changes only with an intended engine change, and the commit
//! says why (CLAUDE.md). Reference models (M5, M7) add cases the same way.

mod support;

use taktsim_trace::ModelHash;
use taktsim_trace::golden::{self, Case};

#[test]
fn single_server() {
    let case = Case {
        name: "toy_single_server",
        model_hash: ModelHash::sha256(b"toy single server: source 1min, station 45s, sink"),
        until: "1h".parse().unwrap(),
    };
    let golden = golden::record(&case, || support::single_server(42)).unwrap();
    insta::assert_snapshot!(case.name, golden.snapshot());
}

#[test]
fn tandem_with_breakdowns_and_scrap() {
    let case = Case {
        name: "toy_tandem",
        model_hash: ModelHash::sha256(
            b"toy tandem: source 1min, cut 40s mtbf 20min mttr 3min, weld 50s scrap 1/7, sink",
        ),
        until: "4h".parse().unwrap(),
    };
    let golden = golden::record(&case, || support::tandem(7)).unwrap();
    insta::assert_snapshot!(case.name, golden.snapshot());
}

#[test]
fn long_line() {
    // Longer than the dump shows, and several chunks.
    let case = Case {
        name: "toy_line_20",
        model_hash: ModelHash::sha256(b"toy line: source 1min, 20 stations 40s, sink"),
        until: "8h".parse().unwrap(),
    };
    let golden = golden::record(&case, || support::line(2026, 20)).unwrap();
    assert!(golden.summary().chunks > 1);
    insta::assert_snapshot!(case.name, golden.snapshot());
}

#[test]
fn the_harness_checks_names_and_writes_hash_files() {
    let bad = Case {
        name: "Toy-Line",
        model_hash: ModelHash::default(),
        until: "1h".parse().unwrap(),
    };
    let Err(e) = golden::record(&bad, || support::single_server(1)) else {
        panic!("a name with capitals and `-` was accepted");
    };
    assert!(matches!(e, golden::GoldenError::Name { .. }), "{e}");

    // Not a golden case of its own: no snapshot, and written to a
    // temporary directory only.
    let case = Case {
        name: "harness_check",
        model_hash: ModelHash::sha256(b"harness"),
        until: "30min".parse().unwrap(),
    };
    let golden = golden::record(&case, || support::single_server(9)).unwrap();
    assert_eq!(golden.header().seed, 9);
    let dir = std::env::temp_dir().join(format!("taktsim-golden-{}", std::process::id()));
    golden.write_hashes(&dir).unwrap();
    let line = std::fs::read_to_string(dir.join("harness_check.txt")).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert_eq!(line, golden.hash_line());
    assert_eq!(
        line,
        format!(
            "harness_check records={} stream={} file={}\n",
            golden.summary().records,
            golden.summary().hash,
            golden.file_hash()
        )
    );
    // A file where the directory should be.
    let blocked = std::env::temp_dir().join(format!("taktsim-golden-file-{}", std::process::id()));
    std::fs::write(&blocked, "").unwrap();
    let result = golden.write_hashes(&blocked);
    std::fs::remove_file(&blocked).unwrap();
    assert!(matches!(result, Err(golden::GoldenError::Hashes { .. })));
}

/// The hashes CI recorded on all four tier-1 targets when traces were
/// hashed with `sha2` 0.11.1 and checksummed with `crc32fast` 1.5.3 (PR #21,
/// workflow run 40393351617, engine version 0.0.0). The `taktsim_core::digest`
/// implementations that replaced them must give the same bytes: the stream
/// hash covers the records, the file hash also every CRC32 and the header.
/// The header's engine version is set to 0.0.0, so a release does not
/// change these values.
#[test]
fn hashes_are_those_of_the_replaced_implementations() {
    type Build = fn() -> Result<taktsim_core::Engine<support::Toy>, taktsim_core::EngineError>;
    let cases: [(Build, &str, &[u8], &str, &str); 3] = [
        (
            || support::single_server(42),
            "1h",
            b"toy single server: source 1min, station 45s, sink",
            "c86ca63bf7f7872c94dad43b55172f708429a3e8cd0f4d782b40e6cbcf1011fc",
            "c50bc48124633c669b4cba5d42c01d7d1235552aa245ac86005741f378d63386",
        ),
        (
            || support::tandem(7),
            "4h",
            b"toy tandem: source 1min, cut 40s mtbf 20min mttr 3min, weld 50s scrap 1/7, sink",
            "e1ce08bd264dd183ae810a4cbe84200525dced7c477463ea0f53549bcf87148c",
            "ff2ddaa9478deaac2fce455e024431c912827ce0d2ffa3eb2965125982972ed7",
        ),
        (
            || support::line(2026, 20),
            "8h",
            b"toy line: source 1min, 20 stations 40s, sink",
            "ea72b70a24c61ddb15b0fc153dd666b6b08391dde3991e7edcbece5fd84ea655",
            "51c94cad52d0a86d9a95639cedf4563b013d6d0089115454f3a2ba2cd54383ed",
        ),
    ];
    for (build, until, model, stream, file) in cases {
        let mut engine = build().unwrap();
        let mut header = taktsim_trace::Header::for_engine(&engine, ModelHash::sha256(model));
        header.engine_version = "0.0.0".to_owned();
        let mut writer = taktsim_trace::TraceWriter::new(Vec::new(), &header).unwrap();
        engine
            .run(
                taktsim_core::RunLimits::until(until.parse().unwrap()),
                &mut writer,
            )
            .unwrap();
        let (bytes, summary) = writer.finish().unwrap();
        assert_eq!(summary.hash.to_string(), stream);
        assert_eq!(taktsim_trace::TraceHash::sha256(&bytes).to_string(), file);
    }
}
