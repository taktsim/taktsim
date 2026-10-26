//! The trace measurement of roadmap M3 (ADR-0021): a toy line of 1,000
//! components (a source, 998 stations, a sink) runs for a number of events
//! while its trace streams to a file; the trace is then read and replayed.
//! `ci/trace-memory.sh` runs it on the CI proxy runner (ADR-0010) and
//! records peak memory and file size.
//!
//! ```text
//! trace_memory write <events> <path>   run and write the trace to <path>
//! trace_memory notrace <events>        run without a trace (the baseline)
//! trace_memory read <path>             read and decode every record
//! trace_memory replay <path>           replay the run from its trace
//! ```
//!
//! Prints one line of `key=value` pairs, ending with the process's peak
//! resident memory (`VmHWM` from `/proc/self/status`, Linux only) and its
//! wall time. Wall-clock times are for orientation only, never evidence for
//! a target.

#[path = "../tests/support/mod.rs"]
mod support;

use std::error::Error;
use std::fs::File;
use std::io::{self, BufReader, Write};
use std::time::Instant;

use support::Toy;
use taktsim_core::{Engine, EngineError, NoTrace, RunLimits};
use taktsim_trace::{Header, ModelHash, TraceReader, TraceWriter, replay};
use taktsim_units::SimTime;

const SEED: u64 = 2026;
const STATIONS: usize = 998;

fn model() -> Result<Engine<Toy>, EngineError> {
    support::line(SEED, STATIONS)
}

fn usage() -> Box<dyn Error> {
    "usage: trace_memory write <events> <path> | notrace <events> | read <path> | replay <path>"
        .into()
}

fn main() -> Result<(), Box<dyn Error>> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let start = Instant::now();
    let report = match args
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>()
        .as_slice()
    {
        ["write", events, path] => {
            let mut engine = model()?;
            let header = Header::for_engine(&engine, ModelHash::sha256(b"trace_memory line"));
            let mut writer = TraceWriter::new(File::create(path)?, &header)?;
            let limits =
                RunLimits::until(SimTime::from_nanos(u64::MAX)).max_events(events.parse()?);
            let run = engine.run(limits, &mut writer)?;
            let (_, s) = writer.finish()?;
            format!(
                "mode=write components={} events={} records={} chunks={} bytes={} stream_sha256={}",
                header.components.len(),
                run.events,
                s.records,
                s.chunks,
                s.bytes,
                s.hash
            )
        }
        ["notrace", events] => {
            let mut engine = model()?;
            let limits =
                RunLimits::until(SimTime::from_nanos(u64::MAX)).max_events(events.parse()?);
            let run = engine.run(limits, &mut NoTrace)?;
            format!("mode=notrace events={}", run.events)
        }
        ["read", path] => {
            let mut reader = TraceReader::<_, Toy>::new(BufReader::new(File::open(path)?))?;
            let mut records = 0_u64;
            while reader.next_record()?.is_some() {
                records += 1;
            }
            let s = reader.summary().ok_or("no end marker")?;
            format!("mode=read records={records} stream_sha256={}", s.hash)
        }
        ["replay", path] => {
            let reader = TraceReader::<_, Toy>::new(BufReader::new(File::open(path)?))?;
            let r = replay(reader, &mut model()?)?;
            format!("mode=replay records={} events={}", r.records, r.events)
        }
        _ => return Err(usage()),
    };
    let seconds = start.elapsed().as_secs_f64();
    let peak = peak_rss_kib().map_or_else(|| "unknown".to_owned(), |kib| kib.to_string());
    writeln!(
        io::stdout(),
        "{report} peak_rss_kib={peak} wall_seconds={seconds:.2}"
    )?;
    Ok(())
}

/// The process's peak resident set size in KiB, where the OS reports it.
fn peak_rss_kib() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|l| l.starts_with("VmHWM:"))?;
    line.split_whitespace().nth(1)?.parse().ok()
}
