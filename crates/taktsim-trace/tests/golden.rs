//! Golden traces of the toy models (spec §7 Layer 2, ADR-0021): each run
//! must reproduce its committed snapshot, stream hash included, on every
//! tier-1 target. CI also collects every case's hashes from every target and
//! fails if one differs (`TAKTSIM_TRACE_HASHES`, A4).
//!
//! A snapshot changes only with an intended engine change, and the commit
//! says why (CLAUDE.md). Reference models (M5, M7) add cases the same way.

mod support;

use taktsim_trace::golden::{self, Case};
use taktsim_trace::{ModelHash, SystemHash};

#[test]
fn single_server() {
    let case = Case {
        name: "toy_single_server",
        model_hash: ModelHash::sha256(b"toy single server: source 1min, station 45s, sink"),
        system_hash: SystemHash::sha256(b"toy single server"),
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
        system_hash: SystemHash::sha256(b"toy tandem"),
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
        system_hash: SystemHash::sha256(b"toy line 20"),
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
        system_hash: SystemHash::default(),
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
        system_hash: SystemHash::sha256(b"harness"),
        until: "30min".parse().unwrap(),
    };
    let golden = golden::record(&case, || support::single_server(9)).unwrap();
    assert_eq!(golden.header().seed, Some(9));
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

/// The stream and file hashes of the three cases, pinned for the layout of
/// ADR-0021 as decided on 2026-11-05 (record prefix, chunk first time,
/// record format and system hash in the header), then ADR-0022 (the record
/// format's name in the header; the stream hashes did not change), then
/// ADR-0024 §7 (`has seed` in the header, file format 3: the file hashes change,
/// the stream hashes do not, since the header is not in the stream), recorded
/// on Linux x86-64. The cross-platform job compares the same hashes from every
/// tier-1 target, so this test pins the bytes and that job proves they are
/// the same everywhere. The header's engine version is set to 0.0.0, so a
/// release does not change these values; a change to any record, the
/// framing or the header does, and the commit must say why.
#[test]
fn hashes_are_pinned_for_this_layout() {
    type Build = fn() -> Result<taktsim_core::Engine<support::Toy>, taktsim_core::EngineError>;
    let cases: [(Build, &str, &[u8], &str, &str); 3] = [
        (
            || support::single_server(42),
            "1h",
            b"toy single server: source 1min, station 45s, sink",
            "208cda594606f9c0962eaf11b248a4aeb5b50ae09093fba036f434c93c01adad",
            "463ed546cb6949d952081576d7c78d83e0a8a44b79fc2d4d45b5c1193155cbde",
        ),
        (
            || support::tandem(7),
            "4h",
            b"toy tandem: source 1min, cut 40s mtbf 20min mttr 3min, weld 50s scrap 1/7, sink",
            "744ef2b5754faccc360ca0bef9f3300a49163501a3fd7d3db34cc92ddd46d4f9",
            "7d82fef53196735fb1aeac61f4af7a0a749955edf0d8cc419da08049e8e4dc4e",
        ),
        (
            || support::line(2026, 20),
            "8h",
            b"toy line: source 1min, 20 stations 40s, sink",
            "1e4383e4188daeeb3fc7ca8f44a34b45c348c52d621f0039a40742399bd251f3",
            "17d1d56b084f0e6d632f93385ac81b912e39ac5426de4f86f6402e75bb3e15e0",
        ),
    ];
    for (build, until, model, stream, file) in cases {
        let mut engine = build().unwrap();
        let mut header = taktsim_trace::Header::for_engine(
            &engine,
            ModelHash::sha256(model),
            SystemHash::sha256(model),
        );
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

/// A case with no seed says so, and the stream is the same as with one: the
/// seed is in the header, not in the records.
#[test]
fn a_case_without_a_seed_says_none() {
    let case = golden::Case {
        name: "no_seed_check",
        model_hash: ModelHash::sha256(b"no seed"),
        system_hash: SystemHash::sha256(b"no seed"),
        until: "1h".parse().unwrap(),
    };
    let stated = golden::record(&case, || support::single_server(42)).unwrap();
    let none = golden::record_without_seed(&case, || support::single_server(42)).unwrap();
    assert_eq!(stated.header().seed, Some(42));
    assert_eq!(none.header().seed, None);
    assert_eq!(none.summary().hash, stated.summary().hash);
    assert_ne!(none.file_hash(), stated.file_hash());
    assert!(none.snapshot().contains("\nseed: none\n"));
    assert!(stated.snapshot().contains("\nseed: 42\n"));
}
