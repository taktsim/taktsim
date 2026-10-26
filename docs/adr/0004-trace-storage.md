# ADR-0004: Trace storage

- Status: accepted (2026-10-25)
- Date: 2026-10-25
- Blocks: M3 (trace and replay)
- Spec: §15 "Trace storage", §7 golden traces and replay, §8 Out formats,
  §9 Model size

## Context

The trace is the record of every event and control decision in a run. It
backs golden-trace tests (byte for byte), replay mode, `trace.query`
(v0.2), `explain` (v0.4), and viewer scrubbing (snapshots plus events,
v0.4). Constraints:

- `taktsim-core` has no I/O and depends only on `taktsim-units`. Core
  emits trace records; another crate writes them.
- 1,000 components and 10 M events in a run within 2 GB (initial target);
  traces stream to disk and are never fully in memory.
- Spec §8 lists Parquet and CSV as v0.1 output formats for traces.
- The engine runs in WebAssembly from v0.2; the trace format must be
  readable there without a large dependency.

## Options

- **S1. Arrow in memory, Parquet on disk via `arrow-rs`** as the native
  trace format.
- **S2. Append-only binary format** (versioned header, length-prefixed
  records in fixed-size chunks with a checksum each), plus a reader that
  exposes records as Arrow batches and an exporter to Parquet and CSV.

## Trade-offs

| | S1 Parquet native | S2 append-only + Arrow reader |
| --- | --- | --- |
| Streaming write, bounded memory | yes, but the writer buffers a row group (size must be tuned) | yes, buffer of one chunk |
| Byte-for-byte golden traces | weak: Parquet files embed writer version, encoding and layout choices that change across `arrow-rs` releases | strong: the format is ours and versioned with the engine |
| Replay and snapshots | sequential scan of columnar data | natural fit, records in event order |
| Query and interop | best (Polars, DuckDB, pandas read it directly) | through the Arrow reader or Parquet export |
| Dependency weight | `arrow` + `parquet`: large compile time and WASM size | small; Arrow only in the reader crate behind a feature |
| Partial file after crash | footer missing → unreadable | readable up to the last complete chunk |

## Recommendation

**S2.** The native trace is an append-only, versioned binary stream:

- Core defines the trace record types and a `TraceSink` trait; a writer
  outside core (new small crate `taktsim-trace`, or a module in
  `taktsim-adapters`; I recommend the crate, so the CLI, MCP server
  (dropped by ADR-0019) and WASM build share it without pulling in
  adapters) implements it.
- Records in event order, fixed-width little-endian encoding, chunks of
  e.g. 64 KiB with CRC32; the header carries format version, engine
  version, model hash and seed.
- Golden traces and the cross-platform test compare the SHA-256 of the
  canonical stream, with a readable text dump for `insta` diffs.
- Reader: iterator over records (used by replay) and, behind a feature,
  Arrow record batches.
- Export: CSV in v0.1 (M11). Parquet export arrives with the Parquet
  adapter in v0.2, unless you want it in v0.1 (see question 1).
- Validate in M3 by writing 10 M synthetic events and recording peak
  memory and file size; numbers go into ADR-0007.

## Consequences

- One more format to specify and version. It is internal; the published
  interfaces are the Arrow reader and the exports.
- Golden traces are independent of third-party file-format releases.
- The WASM viewer can read traces and snapshots without `arrow-rs`.

## Resolved questions

The owner accepted every recommendation in this ADR on 2026-10-25. The
questions as they were asked:

1. Spec §8 lists Parquet output for traces in v0.1, but the v0.1 tier
   lists only the CSV adapter. Recommended: CSV export in v0.1, Parquet in
   v0.2. Or do you want Parquet export in v0.1?
2. OK to add a `taktsim-trace` crate that is not in the spec's §10 layout?

Decided: CSV export in v0.1, Parquet in v0.2; `taktsim-trace` is added to
the workspace.
