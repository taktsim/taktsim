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
