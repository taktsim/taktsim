# ADR-0021: Trace format, message encoding and replay

- Status: proposed (accepted when the M3 pull request is merged)
- Date: 2026-10-26
- Blocks: M3; used by M5 and M7 (golden traces of reference models), M11
  (CSV export), v0.2 (`trace query`, Arrow reader)
- Spec: §7 Layer 2 (golden traces with `insta`), Layer 3 (replay,
  provenance), §8 (trace outputs), §9 (reproducibility, model size);
  ADR-0004, ADR-0007, ADR-0010, ADR-0013, ADR-0018

## Context

ADR-0004 chose the native trace format: an append-only, versioned binary
stream, written by `taktsim-trace`, fixed-width little-endian records in
chunks of about 64 KiB with a CRC32 each, a header with format version,
engine version, model hash and seed, golden tests comparing the SHA-256
of the canonical stream with a text dump for `insta` diffs, and a
10 M event measurement. ADR-0018 made messages the model's type
(`Engine<M>`, `TraceRecord<'a, M>`): the engine never looks inside them.

ADR-0004 leaves open the details M3 needs: how a generic message becomes
bytes, the exact layout, framing and integrity checks, what the hash
covers, how versions are checked, what replay compares, how golden tests
and the cross-platform comparison work, and where CSV export goes. The
spec names `insta` as a *default*, which an ADR has to confirm. This ADR
settles these. None of it changes the spec or ADR-0004; two readings are
marked and listed in the questions.

## Options and trade-offs

### Message encoding

The trace must store `M` deterministically on every platform.

| | E1. Method on core's `Message` | E2. serde + a binary format crate | E3. `TraceMessage` trait in `taktsim-trace` |
| --- | --- | --- | --- |
| Core change | every `Message` (tests, docs, benches) must implement it; core learns an encoding | none | none |
| Bytes stable across dependency releases | yes | no: the bytes follow the format crate's releases (bincode 1 → 2 changed them), the reason ADR-0004 rejected Parquet as the native format | yes: the format is ours |
| Effort per message type | small | `#[derive]` | a short hand-written `encode`/`decode` |
| Platform-dependent sizes | possible | `usize` encodes as the platform width unless configured | impossible: the encoder has no `usize` method |
| Wrong reader detected | no | no | yes: a format id in the header |

**E3.** A component package implements `TraceMessage` for its message
enum (`FORMAT`, `encode`, `decode`), next to its priorities, which are
also part of the trace contract (ADR-0018). The `Encoder` writes only
fixed-width little-endian integers, `bool` as 0/1, `f64` as its bits,
text as a `u16` length and UTF-8, and the engine's ids and times; the
`Decoder` reads the same and rejects anything else (unknown tags, a
`bool` that is not 0 or 1, invalid UTF-8, left-over bytes) with a typed
`DecodeError`. `FORMAT` (e.g. `"taktsim-components/1"`) goes into the
header; a reader for another format refuses the trace. A derive macro can
generate the same calls later if hand-written code becomes tedious.

**NaN.** Arithmetic that produces a NaN gives different sign and payload
bits on x86-64 and ARM64, so writing a NaN's own bits would make traces
differ between tier-1 targets. Every NaN is written as
`0x7ff8000000000000`. All other values, `-0.0` included, keep their exact
bits.

### Layout, chunks and integrity

ADR-0004 says both "length-prefixed records" (option S2) and "fixed-width
little-endian encoding" (recommendation). *Reading:* every field has a
fixed width; each record starts with its kind, and an event's message,
the only variable part, has a `u16` length prefix, so a record's end is
found without decoding the message (needed by a reader that does not know
`M`, such as a future `trace query` index).

All integers little-endian; no field has a platform-dependent size.

| Block | Bytes |
| --- | --- |
| Header | magic `TAKT\r\n\x1a\n` (8; catches line-ending conversion, as PNG does), format version u16 = 1, engine version (u16 length + UTF-8), message format (same), model hash (32), seed u64, component count u32, each component id (u16 length + UTF-8), CRC32 u32 of all before it |
| Chunk | tag `C`, payload length u32 (1 to 65,536), records u32 (≥ 1), index of its first record u64, CRC32 u32 of the 17 bytes before it and the payload, payload (whole records) |
| End marker | tag `E`, records u64, events u64, chunks u64, stream SHA-256 (32), CRC32 u32 of all before it; nothing may follow |

| Record | Kind | Fields after the kind byte |
| --- | --- | --- |
| event | 1 | seq u64, time u64 (ns), target u32, from u32, has item u8, item u64 (0 if none), message length u16, message |
| item created | 2 | time u64, item u64, by u32 |
| item disposed | 3 | time u64, item u64, by u32, how u8 (0 delivered, 1 scrapped, 2 consumed) |

An event takes 36 bytes plus its message; the toy line of the
measurement below averages 40 bytes per record.

- **Header additions.** Besides ADR-0004's four fields, the header holds
  the message format (so the right message type reads it) and the
  component ids (records refer to components by number; with the ids a
  trace is readable on its own, and readers and replay check every number
  against the list).
- **Chunks.** A record never spans chunks; the writer closes a chunk when
  the next record would pass 64 KiB, so a record is at most 64 KiB
  (`RecordTooLarge` otherwise). Writer and reader each hold one chunk.
  Lengths and counts read from a file are checked before use and never
  size an allocation, so damaged data cannot exhaust memory.
- **Integrity.** CRC32 (IEEE) per block detects damage; the first-record
  index detects a missing, repeated or reordered chunk; the end marker's
  counts and stream hash are compared with what was read. A trace without
  end marker is *truncated*: its writer failed or crashed. The reader
  returns the records of every intact chunk before the first error
  (ADR-0004: "readable up to the last complete chunk"), then one typed
  `ReadError`, which names the block, byte offset, what was found and
  what is allowed.
- **Writer errors.** Recording cannot fail a run (ADR-0018): the writer
  keeps the first error, drops later records and returns the error from
  `finish`, without writing an end marker.

### Versions and hashes

- **Format version.** A reader accepts exactly `FORMAT_VERSION`; the
  error says which version was found and which this build reads. A new
  layout is a new version, and readers of older versions are added only
  when needed.
- **Engine version.** `taktsim_core::ENGINE_VERSION` (the core crate's
  version) is written into the header. Replay refuses a trace of another
  engine version: the spec promises identical traces only for the same
  engine version.
- **Stream hash** (ADR-0004's "SHA-256 of the canonical stream"): SHA-256
  of every record's bytes in order, i.e. the concatenated chunk payloads.
  It does not depend on chunk boundaries or the header, so two runs with
  the same records have the same hash whatever engine version or seed the
  header shows. The end marker stores it; the reader checks it.
- **File hash:** SHA-256 of the whole file. The cross-platform comparison
  checks it as well, so the header and framing are also byte-identical.

### Replay

Spec §7: "Replay mode re-executes a run from its trace and asserts
identical state at every event." *Reading:* `replay(reader, engine)`
checks the header against the engine (engine version, seed, component
ids), then runs the engine one event at a time up to the time of the
trace's next event and compares

1. every record the engine emits with the trace's next record, byte for
   byte: time, sequence number, target, sender, message and item of each
   event, and every item created or disposed of;
2. after the start and after every event, the engine's clock and item
   counts (created, delivered, scrapped, consumed, live) with those the
   trace implies.

The first difference stops the replay with `Diverged` or `State`, naming
the record and showing both sides as text. A component's internal state
(queue contents, machine state) is not in the trace; it is checked by the
component's invariants, which the replayed engine runs after every event
with checks on. Comparing it directly would need a state digest from
every component, a new `Component` method in core; the state snapshots
of spec §14 (v0.4) are the natural place for that (question 1).

### Golden traces and the cross-platform job

- **`insta` 1.49.0** (spec default, confirmed here), without default
  features: plain-text snapshots only.
- `taktsim-trace` has a `golden` feature with the harness:
  `golden::record(case, build)` runs a model into a trace, reads it back,
  replays it with a second engine from `build`, and returns a snapshot
  text: provenance without the engine version, counts, the stream hash,
  and one line per record (the first 400 and last 100). Tests compare it
  with `insta::assert_snapshot!`. Without the engine version and file
  size, a release does not change every snapshot; a changed record always
  changes the stream hash.
- If `TAKTSIM_TRACE_HASHES` names a directory, `record` writes
  `<case>.txt` there: `<case> records=<n> stream=<sha256> file=<sha256>`.
  The `test` job sets it on every tier-1 target and uploads the files;
  the `trace hashes` job (`ci/trace-hashes.sh`, tested by
  `ci/trace-hashes-test.sh`) fails unless all four targets report the
  same cases with the same hashes, and shows each target's set.
- Reference models plug in without changing the harness: M5 and M7 add a
  test in their crate with `taktsim-trace` (feature `golden`) as a
  dev-dependency, one `golden::record` call and one snapshot per case.
  Case names are unique across the workspace. Until then the cases are toy
  models built from test components (single server; tandem with
  breakdowns, cancelled timers and scrap; a 20-station line).

### CSV export

ADR-0004 puts CSV export in v0.1 at M11; ADR-0017 says trace exports use
its `CsvWriter`; the M3 roadmap entry does not list it. Placement: a `csv`
feature of `taktsim-trace` depending on `taktsim-adapters/csv`, so
`taktsim-trace` stays free of adapters by default (the WASM build) and
`taktsim-adapters` stays free of the engine. One row per record: record
index, time in ns, kind, seq, sender, target (component ids), item,
disposal, message (`Debug` text). Built in M11, not in this PR.

### Core changes

Kept to three, none with a branch:

- `taktsim_core::ENGINE_VERSION`: the engine version for provenance.
- `Engine::config()`: so the header takes the seed from the engine and
  replay can check it, instead of trusting the caller.
- `TraceRecord` is no longer `#[non_exhaustive]`: the writer must encode
  every kind, and with the attribute a new kind would compile and be
  dropped by a wildcard arm (CLAUDE.md: no wildcard arms).

### Dependencies

| Crate | Version | License | Why |
| --- | --- | --- | --- |
| `sha2` | 0.11.1 | MIT OR Apache-2.0 | SHA-256 (ADR-0002 and ADR-0004 name it); pulls in `digest`, `block-buffer`, `crypto-common`, `hybrid-array`, `typenum` (all MIT OR Apache-2.0); `cpufeatures` and `cfg-if` are already in the lock file |
| `crc32fast` | 1.5.3 | MIT OR Apache-2.0 | CRC32 per block |
| `insta` (dev) | 1.49.0 | Apache-2.0 | snapshots; pulls in `similar` (Apache-2.0) |

`sha2` and `crc32fast` use SHA and carry-less-multiply instructions when
the CPU has them, detected at run time, with portable fallbacks. Both
compute exact integer functions, so every path gives the same bits
(ADR-0010's rule for CPU-specific code).

## Measurements

The 10 M event measurement (ADR-0004, ADR-0007), on the CI proxy runner
(ADR-0010): `ci/trace-memory.sh`, run by the `trace-memory` workflow. A toy
line of 1,000 components (source, 998 single-server stations, sink) runs
10,000,000 events with checks on while the trace streams to a file; the
trace is then read and replayed. Peak memory is the process's resident
high-water mark; median of 10 runs per mode.

MEASUREMENTS

## Consequences

- Component packages depend on `taktsim-trace` to make their messages
  storable, and own their encoding and its `FORMAT` version.
- The trace's memory does not grow with the run: one 64 KiB chunk in the
  writer and one in the reader.
- A changed record, message encoding or priority changes the stream hash
  and fails a golden test on every target; a platform difference fails
  the cross-platform job even where no snapshot shows it.
- Golden snapshots survive releases that change no record.
- Replay checks events, item bookkeeping and invariants, not a direct
  comparison of component state (question 1).

## Questions for the owner

1. **Replay "state" (reading).** Replay compares every record, the clock
   and the item counts after every event, and runs the components'
   invariants; it does not compare component-internal state. A direct
   comparison needs a state digest method on `Component` (a core change).
   Is this reading right for v0.1, with state digests left to the
   snapshots of v0.4?
2. **Measurement location.** ADR-0004 says the 10 M event numbers go into
   ADR-0007; they are recorded here and appended to ADR-0007's
   measurements. Fine?
3. **CSV export timing.** ADR-0004 puts it in M11, ADR-0017 mentions it
   under M3. This PR leaves it to M11 (placement above). Fine?
4. **Required check.** The new `trace hashes (identical on every target)`
   job should become a required check on `main` for A4. The
   `trace-memory` workflow is a measurement, not a gate, until the target
   becomes required in v0.2 (ADR-0007).
