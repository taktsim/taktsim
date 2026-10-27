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

Three small changes, none with a branch:

- `taktsim_core::ENGINE_VERSION`: the engine version for provenance.
- `Engine::config()`: so the header takes the seed from the engine and
  replay can check it, instead of trusting the caller.
- `TraceRecord` is no longer `#[non_exhaustive]`: the writer must encode
  every kind, and with the attribute a new kind would compile and be
  dropped by a wildcard arm (CLAUDE.md: no wildcard arms).

And one module, `taktsim_core::digest` (below).

### Hashes and checksums in our own code

The first version of this PR used `sha2` 0.11.1 (with `digest`,
`block-buffer`, `crypto-common`, `hybrid-array`, `typenum`) and
`crc32fast` 1.5.3. The owner decided on 2026-10-27 to keep shipped
dependencies slim and replace both with our own code. It lives in
`taktsim_core::digest`: pure computation, which core may contain (no I/O,
no new dependency), and the model hash of M4 (ADR-0002) uses it too.

```rust
pub struct Sha256;           // Default, Clone, Debug
impl Sha256 {
    pub const fn new() -> Self;
    pub fn update(&mut self, bytes: &[u8]);
    pub fn finalize(self) -> [u8; 32];
}
pub fn sha256(bytes: &[u8]) -> [u8; 32];

pub struct Crc32;            // Default, Clone, Debug
impl Crc32 {
    pub const fn new() -> Self;
    pub fn update(&mut self, bytes: &[u8]);
    pub const fn finalize(self) -> u32;
}
pub fn crc32(bytes: &[u8]) -> u32;
```

- **SHA-256** follows FIPS 180-4: arithmetic modulo 2^32 with wrapping
  operations, the message length counted in a `u64` of bits, padding per
  5.1.1. Portable scalar code, no CPU-specific paths. It is checked
  against all 65 NIST CAVP short-message vectors (every length from 0 to
  64 bytes, `SHA256ShortMsg.rsp`), the first 8 long-message vectors
  (`SHA256LongMsg.rsp`), the FIPS 180-4 examples including one million
  `a`, padding edge cases at 55, 56, 63, 64, 65, 119, 120, 127 and 128
  bytes, and a property test that any split across `update` calls gives
  the one-shot digest.
- **CRC32** is CRC-32/ISO-HDLC (reflected polynomial `0xEDB88320`,
  initial value and final XOR `0xFFFFFFFF`), byte-wise with a table built
  by a `const fn`. Checked against the check value
  `crc32(b"123456789") == 0xCBF43926`, zlib values and the same split
  property.
- **Same bytes as before.** These are the functions `sha2` and
  `crc32fast` computed, so every trace, snapshot and hash is unchanged. A
  test in `tests/golden.rs` rebuilds the golden traces and compares their
  stream and file hashes with the values CI recorded on all four targets
  with the replaced crates.
- **Cost.** The table-driven CRC and scalar SHA-256 are slower than the
  replaced crates' hardware paths; the wall times below were measured
  with each (orientation only).

### Dependencies

| Crate | Version | License | Why |
| --- | --- | --- | --- |
| `insta` (dev) | 1.49.0 | Apache-2.0 | snapshots in tests; pulls in `similar` (Apache-2.0) |

No new crate is linked into a shipped binary or library.

## Measurements

The 10 M event measurement (ADR-0004, ADR-0007), on the CI proxy runner
(ADR-0010): `ci/trace-memory.sh`, run by the `trace-memory` workflow. A toy
line of 1,000 components (source, 998 single-server stations, sink) runs
10,000,000 events with checks on while the trace streams to a file; the
trace is then read and replayed. Peak memory is the process's resident
high-water mark (`VmHWM`); median of 10 runs per mode.

Two runs on GitHub-hosted `ubuntu-24.04` (x86-64, 4 CPUs, 15.6 GiB): the
first with `sha2` and `crc32fast` (2026-10-26, PR #21 head `47430a8`,
workflow run 40393351697), the second with `taktsim_core::digest`, as
shipped (2026-10-27, head `655ead5`, workflow run 40468566195).

| Mode | Peak memory, median (min–max), `digest` | Same, `sha2`/`crc32fast` |
| --- | --- | --- |
| run without trace (baseline) | 3.3 MiB (3.2–3.4) | 3.5 MiB (3.4–3.5) |
| run, trace written to a file | 3.4 MiB (3.3–3.4) | 3.6 MiB (3.6–3.6) |
| read and decode every record | 2.3 MiB (2.3–2.3) | 2.5 MiB (2.5–2.5) |
| replay from the trace | 3.5 MiB (3.5–3.5) | 3.7 MiB (3.7–3.8) |

- **File size:** 400,186,879 bytes (381.6 MiB) for 10,009,528 records
  (10,000,000 events, the rest items created and disposed of) in 6,107
  chunks: 40.0 bytes per record. The size follows from the format, so it
  is the same on every platform; so is the stream hash
  (`0eb3b0ef…f4bda0`), identical in both runs.
- **Memory:** writing the trace adds about 0.1 MiB to the run, the open
  chunk; reading needs 2.3 MiB in all. Neither grows with the length of
  the trace. The 2 GB target of spec §9 (1,000 components, 10 M events)
  is far away for the trace; the full check with real components is M13
  (ADR-0007).
- Wall times, median, orientation only (not evidence; the baseline alone
  varied from 4.0 s to 5.1 s between the runs): with `digest` 5.1 s
  without trace, 8.9 s with, 3.4 s to read, 8.7 s to replay; with
  `sha2`/`crc32fast` 4.0 s, 6.5 s, 0.8 s and 4.9 s. Scalar SHA-256 and
  byte-wise CRC32 cost about 2.5 s per 400 MB read here. If that matters
  later (e.g. for `trace query` on long traces), a faster portable CRC
  (slicing-by-8) and hashing on a second thread are options that keep the
  bytes the same.

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

## Owner's answers (2026-10-27)

The owner took the recommendations:

1. Yes: replay compares every record, the clock, the item counts and the
   components' invariants after every event. Digests of component-internal
   state come with the snapshots of v0.4.
2. Yes: ADR-0004 asks for the numbers in ADR-0007, so they are recorded
   there and here.
3. Yes: CSV export stays in M11, as ADR-0004 says ("CSV in v0.1 (M11)").
4. Yes: `trace hashes (identical on every target)` becomes a required check
   on `main`. The owner changes the ruleset; `trace-memory` stays a
   measurement.

Also confirmed: replay refuses a trace written by another engine version,
since the determinism guarantee holds per engine version (spec §2,
principle 3).
