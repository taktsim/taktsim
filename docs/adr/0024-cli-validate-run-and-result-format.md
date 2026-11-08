# ADR-0024: CLI `validate` and `run`, the result document and the run store

- Status: proposed (needs the owner's explicit acceptance)
- Date: 2026-11-08
- Blocks: M7; used by M8 and M9 (they fill the result), M11 (`compare` reads
  it), M12 (Python calls the same API)
- Spec: §2 principles 1 to 4, §5, §6, §7 Layers 1 to 3, §9, §10; ADR-0002,
  ADR-0004, ADR-0005, ADR-0014, ADR-0019, ADR-0020, ADR-0021, ADR-0022

## Why explicit acceptance

Most of this settles details M7 needs. These points change the spec or an
accepted ADR, so the whole needs the owner's answer:

1. A value may state "no accuracy statement" (§4). This narrows spec §5's "every
   output that contains a number contains its uncertainty", spec §6's "refuses
   runs that leave it undefined", ADR-0019 item 9, and how principle 4 is read.
2. The trace header's seed can be absent: file format 3 (§7). Amends ADR-0021's
   table and ADR-0004's "the header carries … seed".
3. The run id also covers the engine name, the two record formats and the domain
   package's name and version (§6). Amends spec §5 and ADR-0019 item 5.
4. An error of exit code 2 or 7 has no "one valid example" (§9). Amends ADR-0019
   item 3 and spec §5.
5. The run store moves from `taktsim-cli` to a new library crate (§1). Amends
   spec §10.

## Context

M7 is where people and agents first run a real line and get a typed result.
M5 (ADR-0022, still proposed) gives `Registry::prepare`, `Prepared::run_traced`
and `Model::with_horizon`. M7 is written on ADR-0022's recommended answers and
starts before that ADR is accepted, on the owner's instruction. M6 (routing) is
built beside it.

The result document is the expensive door here: M8, M11 and M12 build on it, and
ADR-0019 makes removing or renaming a field a breaking change. The result and
trace formats must not assume a factory, and must not assume that every error is
sampling error. A deterministic model has no sampling error, and a later engine
may have an error of another kind. So nothing in them is shaped around a
confidence interval or a machine.

## 1. Where the logic lives

The Python bindings (M12) must call the same code as the CLI, so nothing but
argument parsing and rendering may stay in the binary.

| | L1. New crate `taktsim-run` | L2. In `taktsim-components` | L3. Library target of `taktsim-cli` |
| --- | --- | --- | --- |
| File I/O in the engine's package | no | yes: the run store would sit beside the behaviours | no |
| Python pulls in the parser | no | no | yes: `clap` |
| Fits spec §10 (run store in the CLI crate) | no: amends one line | no | yes |
| Crates | one more | none | none |

**L1.** The graph becomes units ← core ← model, trace ← components ← run ← cli,
python, and `taktsim-run` also uses `taktsim-stats` for its interval type. The
API is generic over ADR-0022's `Domain`, so it names no factory word:

```rust
pub struct Store;                         // open(path), discover(start_dir)
pub struct RunRequest<'a> { pub seed: Option<u64>, pub horizon: Option<SimDuration>,
                            pub max_events: Option<u64>, pub reuse: bool,
                            pub cancel: Option<&'a AtomicBool> }
pub enum ModelSource<'a> { Path(&'a Path), Text(&'a str, Format) }

pub fn validate<D: Domain>(registry: &Registry<D>, source: ModelSource<'_>) -> ValidationReport;
pub fn run<D: Domain>(registry: &Registry<D>, source: ModelSource<'_>, request: &RunRequest<'_>,
                      store: &Store) -> Result<RunResponse, Failure>;
pub fn replay_stored<D: Domain>(registry: &Registry<D>, store: &Store, run_id: &RunId)
                      -> Result<ReplaySummary, Failure>;

pub struct RunResponse { pub result: ResultDocument, pub reused: bool, pub warnings: Vec<Diagnostic> }
pub struct Failure { pub class: FailureClass, pub diagnostics: Vec<Diagnostic> }
pub enum FailureClass { Invalid, RunFailed, Io }   // exit 1, 2, 7
```

`replay_stored` is the library half of a later `taktsim replay`; M7 has no such
command, because the roadmap lists replay as a test, not a command. Beside
replaying, it checks the stored model against the header's model and system
hashes, which replay itself does not (ADR-0021 checks version, seed and
components), and the running package's version against `provenance`. A run with no
seed is replayed under a different internal seed, so a false claim of drawing
nothing diverges (§5).

## 2. Parser and how result types get a schema

**Parser: `clap` with derive**, already accepted by ADR-0019. Version 4.6.7,
already in `Cargo.lock` through `criterion`. Features: `std`, `derive`, `help`,
`usage`, `error-context`; default features off, so no colour crates and no
`strsim`. New in the lock: `clap_derive` (MIT OR Apache-2.0); `heck`, `syn`,
`quote` and `proc-macro2` are there already. The tests use
`std::process::Command`, as today, not `assert_cmd`. No signal crate yet (§9).

**Result types.**

| | J1. `serde`, `serde_json`, `schemars` | J2. `taktsim_model`'s `Record`, `Encode` and `Json` |
| --- | --- | --- |
| New crates in the shipped binary | at least five | none |
| One definition for reader and schema | no: derive reads serde attributes | yes: `Record::read` gives both (ADR-0020 §11) |
| Writer can drift from reader | no | yes: closed by a round-trip test |
| Byte stability | follows `serde_json` | ours, already pinned by the schema freshness check |

**J2.** ADR-0020 replaced `schemars` with own code for the same reasons; ADR-0019's
"`schemars`, already the schema generator" predates it (disagreement 1). Two
additions to `taktsim-model`:

- `Record::OPEN: bool = false`. An open record ignores unknown keys when read and
  leaves `additionalProperties` out of its schema. Model files stay closed. Result
  documents are open, so M8's new fields do not fail a v0.1 validator or reader.
- `pub fn record_schema<T: Record>() -> Json`: the schema of one type with its
  `$defs`.

## 3. The result document

One document for every run, stored and printed byte for byte the same. Keys sort
(`Json`), line ends are `\n` on every target, and it holds no time of day, host
name or absolute path, so the same model and seed on one platform gives the same
bytes. `provenance.platform` is the only field that differs across targets; the
cross-target gate compares traces.

```json
{
  "complete": true,
  "experiment_id": "exp-1f…",
  "format": "taktsim-result/1",
  "metrics": [
    {
      "name": "delivered(sink)",
      "over": { "from": "0s", "to": "8h" },
      "unit": "items",
      "values": [
        {
          "accuracy": [
            { "detail": "one replication has no interval; run several seeds",
              "kind": "none", "reason": "single_replication" }
          ],
          "value": 639
        }
      ]
    }
  ],
  "provenance": {
    "engine": "taktsim-core",
    "engine_version": "0.1.0",
    "model_hash": "56d6…",
    "package": "taktsim-components",
    "package_version": "0.1.0",
    "platform": "x86_64-linux",
    "seeds": [42],
    "system_hash": "18c3…"
  },
  "runs": [
    {
      "complete": true,
      "end_time": "8h",
      "events": 9585,
      "records": 11200,
      "run_id": "taktsim-core-9a…",
      "seed": 42,
      "stop_reason": "horizon",
      "trace": {
        "bytes": 482113,
        "message_format": "taktsim-components/1",
        "path": "runs/taktsim-core-9a…/trace.takt",
        "record_format": "taktsim-core/2",
        "stream_hash": "c4…",
        "trace_format": 3
      }
    }
  ],
  "stop_reason": "horizon",
  "unreported": [
    { "observable": "throughput(sink)", "reason": "not_computed" }
  ]
}
```

Every number above is illustrative, none a measurement.

| Field | Rule |
| --- | --- |
| `format` | `taktsim-result/1`. A new major number only for a breaking change. |
| `metrics[].name`, `unit` | Open strings. `unit` is required: ADR-0014 §6's one declared unit per KPI. |
| `metrics[].over` | `{from, to}` for a window, `{at}` for a reading at one time, as duration strings. M9's warm-up is a `from` above `0s`. |
| `metrics[].observable` | Optional: the declared text this metric answers, such as `state_shares(cut, weld)`. Several metrics may carry the same text. M7 writes none. |
| `metrics[].values` | A list, each with its own accuracy. One entry for a scalar. `state_shares` and a lead-time distribution (M8) are several entries, told apart by an optional `labels` mapping, such as `{resource: cut, state: busy}`, so no reader splits a string. `value` is a number or a boolean, since ADR-0015 lets an observable be declared yes or no; M7 writes numbers only and no `labels`. |
| `runs` | At least one. `seed` absent if the run has none. `run_id` and `trace` absent if the run was not kept (§8). |
| `experiment_id` | Present exactly when every entry of `runs` has a `run_id` (§8). |
| `complete`, `runs[].complete` | Required booleans, and **the only field a reader uses to decide.** `true`: the run, or the experiment, finished what was asked. `false`: it was cut short, the numbers hold only for the stated `over`, and they must not be compared with a complete result. |
| `provenance.seeds` | The seeds used, empty allowed (§5). |
| `stop_reason`, `runs[].stop_reason` | Open set, explanation only: it says why, and `complete` says what follows. At the top it is the experiment's reason. M8 adds `precision_reached` (with `complete: true`) and `budget_exhausted` (with `complete: false`) there, and a later clean ending adds its own word without a format change. |
| `unreported` | What the model declares in `observe` or `experiment.precision` that this result does not hold, with an open `reason`. Required, empty allowed, so a missing KPI is stated in the result, where an agent reads it, not only on stderr. **Each declared observable is either the `observable` of some metric or in `unreported`,** so a reader can check that it got what it asked for. |

Nested `provenance` follows spec §7 Layer 3, and `compare` (M11) can refuse a result
without it. **Reader rule for the whole document:** ignore unknown keys.

**How a run ends.** Core's `RunOutcome::Idle` says only that no events are left, and the
clock then jumps to the horizon. Whether that is fine depends on what is still in the
system, so `taktsim-run` reads it together with the item counts:

| Core's outcome | Items alive | `stop_reason` | `complete` | Exit |
| --- | --- | --- | --- | --- |
| `Horizon` | any | `horizon` | `true` | 0 |
| `Idle` | 0 | `drained` | `true` | 0 |
| `Idle` | above 0 | no result: error `run.stalled` | | 2 |
| `EventLimit` | any | `event_limit` | `false` | 3 |
| `Cancelled` | any | `cancelled` | `false` | 4 (not from M7's CLI, §9) |

`idle` is not a word of the result, because in the glossary it is a machine state that
means no demand. The trace's end marker keeps core's word (§7).

A stall is a failure in M7, not a finding. In the standard catalog a Source's interval, a
cycle or a repair is always pending, and ADR-0022's no-deadlock argument rules out
everything waiting, so items left with no event mean a lost wake-up or a broken type added
with `Registry::with`. The error gives the time of the last event, `items_alive` and
`recent`, read from the temporary trace, and nothing is stored, so the wrong numbers can
never be reused. v0.2 brings stalls that belong to the model, such as an Assembly waiting
on an empty Store. Whether those become a result with `complete: false` and `stop_reason:
stalled` is for the ADR of those components; the word is reserved for it.

**The horizon and `observe`.** `run --horizon` goes through `Model::with_horizon`
before hashing, as ADR-0022 §2 says, so the stored model, its hash and the header
all describe the run. A later `--observe` follows the same rule with a
`Model::with_observe`, so two runs with different observables cannot share a run
id. M7 has no `--observe`, because it computes no observable; adding a flag breaks
nothing.

### What M7 reports

KPIs are M8's. M7 reports only what the trace states directly, read back from the
stored file by one fold over core's records:

| Metric | From | Listed for | `over` |
| --- | --- | --- | --- |
| `created(<id>)` | `ItemCreated` by it | any component that created one; always one with `Ports::Outlet` | `0s` to the end |
| `delivered(<id>)` | `ItemDisposed` by it, `Disposal::Delivered` | any component that delivered one; always one with `Ports::Inlet` | `0s` to the end |
| `scrapped(<id>)`, `consumed(<id>)` | `ItemDisposed` by it, the other two `Disposal`s | any component with one (none in M7's catalog) | `0s` to the end |
| `items_alive` | created minus disposed | once | `{at: end}` |

The fold is keyed on each record's `by` and matches `Disposal` exhaustively. `Ports` only
decides which zero rows are shown, so `delivered(sink) = 0` is stated, not missing; it
never decides who may create. A type with `Ports::Both` or `Ports::None` that creates items
is counted like any other.

The unit is `items`: these are the engine's items, in any domain. A part is one item, and
a lot of a continuous material is one item (spec §4), so the word stays true for `kg` or
`l`. A quantity in a material's own unit is a KPI, whose unit the model declares (M8 and
v0.2), so the domain supplies nothing here.

`items_alive` is deliberately not called WIP: it includes parts a blocked Source
holds, which ADR-0022 §7 keeps out of WIP. The names do not collide with
`throughput`, `lead_time`, `wip` or `state_shares`, so M8 adds metrics without
changing these. Each of the four sums must equal its counter in the engine's `ItemCounts`;
a difference is exit 2.

## 4. Accuracy

Every value has a required `accuracy` list, at least one entry, each tagged:

| `kind` | Other fields | Meaning |
| --- | --- | --- |
| `sampling` | `interval`: `mean`, `half_width`, `std_dev`, `replications`, `level`, from `taktsim_stats::ConfidenceInterval` | a confidence interval over independent replications; M8 fills it |
| `none` | `reason` (open set), `detail` | no statement, and why |
| anything else | unknown | **Reader rule:** reads as no statement, never as an error |

`target` and `reached` are optional beside `kind`, on every kind. `target` is a
`{relative: 0.02}` object, so M8 or a later engine can add an absolute target
without changing a reader of `relative`.

| | A1. Tagged list | A2. One optional interval | A3. One tagged object |
| --- | --- | --- | --- |
| "No statement" can be said | yes | only by absence, which a reader can miss | yes |
| Non-sampling error | a new kind | a new field beside it, or a breaking change | a new kind |
| Two errors on one value | yes | no | breaking change |
| Cost now | a list of one | none | none |

**A1.** M7's reasons: `single_replication` for a run with a seed, and
`no_random_draws` for a model that draws nothing, where the number is exact for
that model and says nothing about how well the model fits reality. The claim behind
`no_random_draws` is checked before the result is stored (§5).

Principle 4 holds in this reading: the value carries a statement, and the statement
is that there is no error bound. No number looks better than it is.

For M8: replications of a model that draws nothing would give a zero-width interval
that looks perfect. §5 prevents that for one model, since such a model has no seeds
to replicate over, and M8 must keep it so.

## 5. Seeds

**Which seeds a run uses,** in the library, so every client gets the same answer:

1. The model draws nothing: **no seed.** `seeds: []`, the header says none, and a
   given `--seed` or `model.seed` is unused, with a `warning` line naming it.
2. Otherwise `--seed` if given, otherwise `model.seed`.
3. Otherwise **refused**, exit 1, type `run.no_seed`, path `model.seed`, example
   `--seed 42`. Never entropy and never an unstated default.

**How the library knows.** A new `ComponentType::draws_random(&self) -> bool`,
default `true`, so an unknown type must declare otherwise, and a required
`Domain::draws_random(document) -> bool` with no default, for what a domain draws
outside its components' parameters (M6's `split`). `Registry::draws_random(&document)`
is the union. `Source` is true with a non-constant `interarrival` or two or more
variants; `Machine` with any non-constant time; `Buffer` and `Sink` never.
`Distribution::Constant` draws nothing (`taktsim-core`'s `sample` returns before
touching the stream). Taking a stream is not drawing.

A false claim would be labelled exact while being one sample path, so it has three
guards, and the first two are not enough alone:

| Guard | What it catches | When |
| --- | --- | --- |
| Each `draws_random` destructures its parameters, and nested ones (`Setup`, its changes), without `..` | a new key nobody answered for: it does not compile | build |
| `no_draw_claim_holds_on_generated_models`: a property test over generated models of the standard catalog, each that claims to draw nothing run under two seeds | a wrong answer in an implementation | CI |
| **The check run:** a run that claims to draw nothing and reached its end is replayed, before it is stored, against an engine with a different internal seed | anything else, including a type added with `Registry::with` | every such run |

A difference in the check run is exit 2, type `run.no_draw_claim_false`, and nothing is
stored. `no_draw_claim_is_true` stays as the check on the reference models. The check run
costs at most one more run of that model, once, because the result is then reused; a model
that draws pays nothing. It needs no change to core and adds nothing to the path of a draw.
It is a check, not a proof: a very rare draw may not show under one other seed in a short
run, which is why the property test is there too. It asks what matters: whether the
trace depends on the seed.

`Registry::prepare` keeps ADR-0022's order, and `no_seed_is_an_error` stays as it
is. A new `RunOptions::no_seed()` asks for a run without a seed: `prepare` refuses
it for a model that draws, otherwise builds the engine with an internal seed of 0
and writes a header with no seed. `RunOptions::no_seed_check()` is the same with an
internal seed of 1, for the check run and `replay_stored`.

**`--seed` against a stated `model.seed`.**

| | K1. Refuse a different value | K2. Record it; `model.seed` is the default | K3. Write it into the model before hashing |
| --- | --- | --- | --- |
| M8's seed list over a model with `seed:` | impossible | works | every replication has another model hash, so "the same model" is lost |
| Fits `model.seed` as "the default seed" and ADR-0022's order | no | yes | no |
| Stored model alone reproduces the run | yes | no: `seeds` is needed too | yes |
| Same file, same flag: same id | yes | yes | yes |

**K2.** The seed that ran is in `provenance.seeds`, in `runs[].seed`, in the header
and in the run id, and it is what replay uses. The stored model keeps what the file
said. The price is that a reader who reruns must take the seed from the result, not
from the model, which spec §5's run id already implies by naming seed beside model.

In M7 `--seed` takes one value; repeating it is exit 1. M8 widens it. It takes at
most 2^53 − 1, as `model.seed` does (ADR-0020), so the run id has a canonical form.

## 6. Run id and experiment id

```text
run_id        = <engine> "-" hex(SHA-256(JCS({
                  "engine", "engine_version", "message_format",
                  "model_hash", "package", "package_version",
                  "record_format", "run_id_format": 1,
                  "seed": <integer or null> })))
experiment_id = "exp-" hex(SHA-256(JCS({
                  "experiment_id_format": 1, "runs": [sorted run ids] })))
```

Example: `taktsim-core-` followed by 64 hex digits. The engine name is
`taktsim_core::ENGINE_NAME`, new beside `ENGINE_VERSION`, and a test pins that
`RECORD_FORMAT` starts with it, so the name before `/` in a record format names the
engine.

The horizon is inside the model hash (ADR-0022 §2), so the spec's model, seed,
horizon and engine version are all covered. The name and the two formats are added
because a store can hold runs of several engines, and a version does not always
move with a format while the workspace is at `0.0.0`. The experiment id is over the
run ids, so members may differ in more than the seed. It adds no field when a later
experiment's members differ in something else.

**The package.** The behaviour of a run comes from the domain package as much as from
the engine, and the message format moves only when the encoding does. So a package that
fixes its failure logic must get a new id, or the store returns the old numbers. A new
`Domain::PACKAGE` and `Domain::PACKAGE_VERSION`, required with no default, give the name
and the crate's `env!("CARGO_PKG_VERSION")`; `Flow` gives `taktsim-components` and the
workspace version. Both go into the id and `provenance`. The test
`message_format_names_the_package` pins that the name before `/` in `Flow`'s message
format is its package.

They do not go into the trace header. A bare trace is still checked record by record on
replay, so a changed package shows as a divergence; `replay_stored` names the cause from
`provenance`. A later header field costs a format version, not every store. What
ADR-0022 says of a type added with `Registry::with` stays true: it is named by neither,
and `--no-reuse` is how to check it.

The id is opaque: compare it whole and do not parse it. Windows' path limit allows
it: `runs/<id>/trace.takt` is 93 characters below the store.

## 7. The trace header's seed

| | H1. Always a number, 0 if none | H2. `has seed u8`, `seed u64` (0 if none) | H3. A new record kind |
| --- | --- | --- | --- |
| A run with no seed says what is true | no: seeds 0 and 7 would look like two runs | yes | yes |
| Pattern already in the format | | events' `has item`, the end marker's `has end` | |
| Replay of a run with no seed | must invent a seed | no seed check; records still compared, under another internal seed (§5) | |
| Cost after v0.1 | | a format version | |

**H2,** before the format is public. `Header.seed` becomes `Option<u64>`; `FORMAT_VERSION`
becomes 3. M5's bump to 2 has not shipped either, but one number per change keeps every
intermediate trace identifiable. A byte other than 0 or 1 in `has seed`, or 0 with a
non-zero seed, is a corrupt header, as for `has end`. Files: `header.rs`, `read.rs`,
`replay.rs` (compares only a stated seed), `golden.rs` (prints `seed: none`, and can
record a case with no seed), and ADR-0021's table.

Pinned file hashes change, stream hashes do not, since the header is not in the
stream. No snapshot changes: each shows `seed: <n>` as before.

**The end marker.** ADR-0022 §9's `has end`, `end time` and `outcome` cover what the
result needs: `over.to` and `end_time` from the time, `stop_reason` and `complete` from the
outcome and the item counts (§3). The marker keeps core's words; only the result maps
them. `run` always writes through `run_traced`, so
a stored trace always states its end. `outcome` is core's vocabulary, under the record
format: another engine's words come with its own record format and reader. `events`
in the marker counts records of kind 1 under that same format (one sentence for
ADR-0021).

## 8. The run store

ADR-0019 item 5 fixes the place: nearest `.taktsim/` upward from the current
directory, else `.taktsim/` here, `--store` overrides. Layout below it:

```text
.taktsim/
  store.json             {"store_format": 1}
  .gitignore             "*"; runs are not models
  runs/<run_id>/
    model.json           the canonical normalized model: SHA-256 is the model hash
    result.json          the result document, same bytes as `run --format json`
    trace.takt           the trace, streamed (ADR-0004)
  tmp/<pid>-<n>/         a run in progress
```

- **Atomic.** A run writes in `tmp/`, closes its files, then renames the directory
  into `runs/`. If the target appeared meanwhile, another process did the same run,
  and the temporary one is dropped. No half-written run is ever visible.
- **Reuse.** If `runs/<id>/result.json` exists and reads, `run` returns it without
  running (ADR-0019 item 5). With `--max-events` too: nothing runs, so no limit is
  reached, and it is exit 0. `--no-reuse` runs anyway and, if the new run reached its
  end, compares the stream hash with the stored one, never overwriting: a difference is
  exit 2 and means the engine changed or determinism broke. This also matters while the
  version stays `0.0.0`. A `--no-reuse` run stopped by a limit compares nothing, since a
  shorter stream says nothing about the engine, and is exit 3.
- **Partial runs are not kept and have no id.** The run id does not cover the limit, and
  an id must name one document that later commands can find (spec §5, ADR-0019 item 5). So
  a run stopped by a limit has `complete: false`, `stop_reason: event_limit`, and no
  `run_id` and no `trace`, and its document has no `experiment_id`. It is still printed
  (exit 3), and `provenance` and `runs[].seed` say what it was. **`runs[].run_id` is present
  exactly when that run reached its end and is in the store; `experiment_id` exactly when
  every run has a `run_id`.** So M8's `budget_exhausted`, 14 finished runs out of 20, has an
  experiment id over those 14 and `complete: false` at the top.
- A failed run (exit 2) is not kept either; its error has the last events. That includes a
  stall (§3) and a false claim of drawing nothing (§5).
- `trace.path` is relative to the store and uses `/` on every target. The stored
  document holds no absolute path, so a `progress` line on stderr states where the
  store is and whether the run was reused. A `store.json` with an unknown
  `store_format` is exit 7.
- `experiments/` is reserved for M8's several-seed documents.

| | B1. `runs/<id>/` directories | B2. Flat files by hash | B3. An index file or database |
| --- | --- | --- | --- |
| A person can look inside | yes | hardly | no |
| Atomic with rename only | yes | per file | needs locking |
| Extra files later (M11's reports) | beside them | a new naming scheme | yes |

**B1.**

## 9. Errors and exit codes

The exit codes are ADR-0019's. Which appear in M7:

| Code | When, in M7 |
| --- | --- |
| 0 | success |
| 1 | a bad flag or value, a model that fails to load, `run.no_seed`, a zero horizon. `validate` on an invalid model |
| 2 | invariant or conservation violation, engine error, `--no-reuse` mismatch, `run.stalled` (§3), `run.no_draw_claim_false` (§5) |
| 3 | `--max-events` reached, with the partial result on stdout |
| 7 | the model file, an included file or the store cannot be read or written (`LoadErrorKind::is_io`), or stdout cannot be written (`io.stdout`) |
| 101 | the invariant-violation path of ADR-0007; a panic hook writes it as one JSON line first when `--format json` was asked for |

Codes 4, 5 and 6 are in the table of `describe` but on no M7 command. Code 4 needs a
signal handler, which ADR-0019 brings with the first long-running command, M8.
`describe` lists per command only the codes it can give; adding one is not a
breaking change. `run` is not long-running under ADR-0019 item 4, so no budget is
mandatory. `--max-events` is there for an agent that wants a bound.

An error line on stderr, with `--format json`, is one JSON object per line:

```json
{"allowed":"a duration such as `42s`","column":14,"example":"8h","file":"m.yaml",
 "found":"the number 10","kind":"error","line":5,"message":"…",
 "path":"components[0].cycle_time","type":"model.wrong_type"}
```

- `type` is a stable word in a namespace: `args.*`, `model.*`, `run.*`, `store.*`,
  `io.*`, `internal.*`. `io.stdout` is a consumer that went away, on any command, so it
  sends no reader to look at a store. Model errors take it from a new `LoadErrorKind::name()`, an exhaustive
  match.
- `path` is a model path, or the flag or argument (`--horizon`, `<model>`). A value that
  came from a flag is reported at the flag, as is a value with a leading hyphen
  (`--seed -1`), since the file it would otherwise point at is not wrong. Parser
  errors go through `Cli::try_parse_from` and clap's error context into the same type,
  and exit 1, not clap's 2. The format is found by scanning the raw arguments, since a
  failed parse has no value.
- `found`, `allowed` and `example` are required for exit 1. **For exit 2 and 7 `example`
  is absent,** as there is no valid value of a broken invariant. Exit 2 carries instead:
  `path` of `components[i]` where a component is known, the simulation `time`, and
  `recent`, the last events. This amends ADR-0019 item 3.
- Text mode renders the same typed `Diagnostic`.

**`validate`.** Its result is a report, as ADR-0005 wants: `{format:
"taktsim-validation/1", valid, errors, model_hash, system_hash}`, the hashes present
only when valid. On an invalid model the report goes to stdout and each error also
to stderr, then exit 1: ADR-0019 item 2 asks for one result document on stdout, and
item 3 for errors on stderr. `validate` reads only. Other commands that fail print
nothing on stdout.

## 10. `describe` and `schema`

`describe` walks `Cli::command()` for commands, arguments, types, defaults and allowed
values. What clap does not hold comes from one function with an exhaustive `match` on
the `Command` enum: purpose, what it writes, `long_running`, `needs_person`, input
schemas, result schema, exit codes, one example. Adding a command does not compile
until it has these. A closed table over clap's `value_name` gives each argument a type
(`DURATION`, `SEED`, `PATH`, …), and a test fails on an unknown one.

The document carries `format` (`taktsim-describe/1`), `package_version`, the exit code
table, the JSON Schema of an error line, and its own schema. It is committed as
`schemas/cli/describe.json`. A freshness test and `ci/check-schemas.sh` compare it,
and `ci/schemas-test.sh` tests that comparison. Result schemas sit inside it, since
ADR-0019 asks for the schema of each result there. Input schemas are by reference, as
`taktsim schema model`.

`taktsim schema <type|all>`: `all` and `model` print the bytes of
`schemas/model/1.0.json`; a component type prints its entry from `$defs`; `result`,
`validation`, `error` and `describe` print those. An unknown name is exit 1 with the
list in `allowed`.

Every command has `--format text|json` as a global flag, default `text`, chosen only
by the flag (ADR-0019 answer 2). `describe` and `schema` print JSON either way.
`--version` and `--help` keep clap's rendering, exit 0.

| Command | Flags in M7 |
| --- | --- |
| `describe` | `--format` |
| `schema <type\|all>` | `--format` |
| `validate <model>` | `--format` |
| `run <model>` | `--format`, `--seed <n>`, `--horizon <duration>`, `--store <dir>`, `--max-events <n>`, `--no-reuse` |

Not in M7: `--fast`, because spec §7 allows it only after one checked run for
calibration loops, and `render`, which waits for the owner.

## 11. Reference models and the harness

`benchmarks/models/*.yaml`, one case per file, named by its stem in lowercase, digits
and `_`, each with its `seed:` and horizon in the file:

- `chain.yaml`: Source, Machine, Buffer, Machine, Sink.
- `chain_failures.yaml`: the same with failures and a capacity-limited buffer that does
  fill.

`crates/taktsim-run/tests/reference_models.rs` lists the directory, sorted, and has no
list of its own. For each model it: runs through `taktsim_run::run` into a temporary
store; records the same model with `golden::record` and requires the same stream hash;
compares the `reference_<stem>` snapshot, which also feeds `TAKTSIM_TRACE_HASHES` and so
`ci/trace-hashes.sh`; runs `replay_stored`; and checks `no_draw_claim_is_true`, where it
applies. A new file with no committed snapshot fails, so it joins with a snapshot and no
code. `branch_merge.yaml`, `rework_loop.yaml` and `pool.yaml` wait for M6, listed for
the lead session in the pull request. `no_draw_claim_holds_on_generated_models` (§5) sits
beside it, because a directory of chosen models says little about keys they do not use.

Tests of the CLI, in `crates/taktsim-cli/tests/`, through the real binary: snapshots of
stdout, stderr and the exit code, with platform and temporary paths masked; the examples
in `describe`; the YAML blocks of the docs through `validate`, using the reader of
`docs_examples.rs`; `crates/taktsim-components/tests/data/malformed/` through `validate`,
each with a path-level JSON error and exit 1; a table of malformed invocations; the
scripted walk-through of ADR-0019 answer 6, which takes every command and flag from
`describe`'s output and fails if it needs a word not printed there; and two runs in two
stores with byte-identical `result.json` and `trace.takt`.

## Consequences

- A person or an agent gets a typed result from a real line, with provenance, and is told
  there is no error bound until M8.
- `Cargo.lock` gains `clap_derive`. The workspace `Cargo.toml`, `ci/check-crates.sh` (a
  direction line for `taktsim-run` and `taktsim-cli`) and `ci/check-schemas.sh` change; all
  tighten, and the pull request carries `gate-change`.
- `ci/check-crates.sh` must allow `taktsim-run` above components. M12's Python crate depends
  on `taktsim-run`.
- M8 fills `sampling`, adds metrics, empties `unreported`, widens `--seed`, adds the signal
  handler and `experiments/`. Nothing here changes for that.
- M6 must answer for its draws in the right place (ADR-0023 §6, §7, §11). `Source::draws_random`:
  `due_in` when not constant, and `priority_mix`. `Machine::draws_random`: `cycle_time_by_variant`
  with any non-constant entry, drawn from the `cycle_time` stream. `Flow`'s `Domain::draws_random`:
  a `split` on any edge, on the `route` stream. `when` and the `choose` rules draw nothing. The
  first two fail to compile until answered, since the parameters are destructured; `split` does
  not, so the generator of `no_draw_claim_holds_on_generated_models` must add it, and until then
  the check run (§5) refuses such a model instead of storing it. The lead session has this on its
  list.
- `Domain` gains three required items in M5's API (`PACKAGE`, `PACKAGE_VERSION`, `draws_random`),
  so M6's branch needs them when the two merge.
- A model that declares `experiment.precision` runs once in M7, with the KPI under
  `unreported`. From M8 it needs a budget (ADR-0019 item 4). Both ship in v0.1, so no
  released behaviour changes.
- Not done, by intent: a generated CLI reference page (ADR-0019 item 1), for the v0.1
  documentation pass.

## One-way doors

Before v0.1 ships, each costs regenerated files. The cost below is after that.

| Door | Commits to | Cost to reverse | Test that proves it |
| --- | --- | --- | --- |
| Commands and flags | every script and agent | semver major (ADR-0019) | `describe_is_fresh`, `walkthrough_uses_only_describe`, CLI snapshots |
| Exit code use per command | every caller's error handling | semver major | `documented_exit_codes`, `malformed_models_exit_1_with_a_path` |
| Error line (`kind`, `type`, `path`, `found`, `allowed`, `example`; none for exit 2 and 7) | every agent's error handling | semver major | `error_lines_match_their_schema`, CLI snapshots |
| Result document: fields and `taktsim-result/1`, including `labels` and `observable` | M8, M11, M12, every reader | a new format; every stored result | `result_round_trips`, `emitted_results_match_the_schema`, `unknown_keys_are_ignored`, `labels_and_observable_round_trip`, `declared_observable_is_answered_or_unreported`, CLI snapshots |
| Accuracy: required list, tags, reader rule, `none` | what every number claims | a new format | `every_value_has_accuracy`, `unknown_kind_reads_as_no_statement`, `single_run_states_no_interval`, `sampling_wraps_the_interval` |
| `complete` as the reader's only test, `stop_reason` as an open explanation | every reader of partial results | a new format | `complete_decides_and_stop_reason_only_explains`, `event_limit_is_partial_and_exits_3` |
| A stall is exit 2 in M7; `drained` for an empty system; `stalled` reserved | every reader's idea of a finished run; v0.2's stalls | an exit code's use (semver major) | `stall_is_exit_2_and_not_kept`, `drained_is_complete` |
| Metric names `created`, `delivered`, `scrapped`, `consumed`, `items_alive`, unit `items` | scripts reading M7's result | a deprecation | `metrics_equal_item_counts`, `any_component_may_create_items`, CLI snapshots |
| Run id and experiment id definition, ids only on stored runs | every stored run, every later reference | all stores go stale | `run_id_is_pinned`, `horizon_changes_the_run_id`, `run_id_names_the_engine`, `package_version_changes_the_run_id`, `experiment_id_is_over_sorted_runs`, `partial_document_has_no_ids` |
| Store layout, `store_format`, reuse rule | every store, `trace query`, `compare` | a store migration | `store_layout_is_pinned`, `second_run_is_reused`, `no_reuse_detects_a_different_trace`, `truncated_no_reuse_exits_3_without_comparing`, `partial_run_is_not_kept`, `model_json_hashes_to_the_model_hash`, `unknown_store_format_is_refused` |
| Trace header: `has seed`, file format 3 | every trace reader | a format version | `header_bytes_are_pinned`, `absent_seed_round_trips`, `run_without_seed_replays`, the unchanged stream hashes in the snapshots |
| Seed rules: no-draw models have none, K2, refusal | which runs count as the same; replication counts | different ids and refusals | `no_seed_refused_when_the_model_draws`, `draws_nothing_runs_with_empty_seeds`, `seed_option_is_recorded_over_model_seed`, `no_draw_claim_is_true`, `no_draw_claim_holds_on_generated_models`, `false_no_draw_claim_exits_2_and_is_not_kept` |
| `ComponentType::draws_random`, `Domain::draws_random`, `Domain::PACKAGE`, `Domain::PACKAGE_VERSION`, `RunOptions::no_seed`, `RunOptions::no_seed_check`, `ENGINE_NAME` | every later package; core's public API | every package | `no_draw_claim_is_true`, doc tests, `record_format_names_the_engine`, `message_format_names_the_package` |
| `taktsim-run` API and direction | M12, every later client | every client | doc tests, `ci/check-crates.sh`, `cargo deny check` |
| No serde in the shipped binary | bill of materials | adding is easy; removing is not | `cargo deny check`, `Cargo.lock` review |

## Spec and code disagreements

| # | What | Where | Recommendation |
| --- | --- | --- | --- |
| 1 | ADR-0019 calls `schemars` "already the schema generator"; ADR-0020 replaced it with own code | ADR-0019 item 1, ADR-0020 §11 | own code (§2); a note in ADR-0019 |
| 2 | "Every output that contains a number contains … confidence interval" and "metrics with confidence intervals"; one run has none | spec §5, ADR-0019 item 9 | "states its accuracy, or that it has none" (§4) |
| 3 | "Refuses runs that leave it undefined" | spec §6 | the type stays required; a one-seed run states `none` |
| 4 | Run store listed in the CLI crate; the logic must be shared with Python | spec §10 | `taktsim-run` (§1); one line |
| 5 | Run id named from model, seed, horizon, engine version | spec §5, ADR-0019 item 5 | add engine name and record formats (§6) |
| 6 | Header always holds a seed; `RunOptions` has no way to run without one | ADR-0021, ADR-0004, ADR-0022 §2 | §7; `RunOptions::no_seed()`, which leaves the existing order alone |
| 7 | Every error carries one valid example | ADR-0019 item 3, spec §5 | none for exit 2 and 7 (§9) |
| 8 | Today's binary exits 2 on a bad argument, and `unknown_arguments_fail_with_usage_on_stderr` pins it; ADR-0019's code is 1 | `crates/taktsim-cli/tests/cli.rs` | that test pins the placeholder, so it is replaced by stricter ones that assert exit 1, an empty stdout and a typed error; the commit says why |
| 9 | Roadmap M7 lists branch/merge, rework and pool models, which need M6 | roadmap M7, M6 | they join from the directory after M6 (§11) |
| 10 | CLAUDE.md: a milestone does not start while an ADR it depends on is proposed | ADR-0022 | the owner's instruction this time; M7 merges after M5 |
| 11 | "A registry with a type from outside is not named by the header or the run id" still holds for `Registry::with`, but a whole `Domain` is now named by the run id | ADR-0022 §11 | a note there when M7 merges (§6) |
| 12 | §9 requires `example` for exit 1, but `LoadErrorKind::example` returns none for about twenty kinds (a syntax error, a duplicate, a self-loop), and has since ADR-0020 | §9, ADR-0020, `crates/taktsim-model/src/error.rs` | the error schema requires `example` for `args.*` only; a `model.*` error has one where the loader has one. Giving every kind an example is a later change that adds a key, so it breaks nothing |
| 13 | A docs-only pull request skips the gates (`ci/changes.sh`), but `every_yaml_block_of_the_docs_validates`, like the older `docs_examples.rs`, reads `docs/` and `README.md`, so a broken example in a page fails only the next full run | `ci/changes.sh`, `crates/taktsim-cli/tests/examples.rs` | decide separately; narrowing what may skip is a gate change of its own |
| 14 | §9 did not list a stdout that cannot be written, and had no namespace for it; `store.write` blamed a store on commands that touch none | §9 | `io.stdout`, still exit 7 |

## Questions for the owner

Implementation proceeds on the recommendations; each can change before the M7 pull request
merges.

1. **No accuracy statement (§4).** Is a value with `kind: none` and a reason a result under
   principle 4? *Recommended:* yes. The alternative is that `run` with one seed reports
   nothing until M8.
2. **`--seed` over a stated `model.seed` (§5).** Record it (K2)? *Recommended:* yes. Refusing
   would stop M8 from replicating a model that states a seed.
3. **A model that draws nothing has no seed, even if one is given (§5).** *Recommended:* yes:
   it keeps one run from looking like two, and rules out replications of identical runs.
4. **Trace header (§7).** `has seed`, file format 3? *Recommended:* yes.
5. **Run id (§6).** Add the engine name and both formats, and show the name in the id?
   *Recommended:* yes.
6. **Exit 2 and 7 errors without an example (§9).** *Recommended:* yes, with `time` and
   `recent` for exit 2.
7. **New crate `taktsim-run` (§1)?** *Recommended:* yes.
8. **`Record::OPEN` (§2).** Result documents open to new keys, model files closed?
   *Recommended:* yes.
9. **`--no-reuse` (§8).** Rerun and compare, never overwrite? *Recommended:* yes. Without it a
   store can hide an engine change while the version stays `0.0.0`.
10. **Partial runs not kept, and without an id (§8).** *Recommended:* yes. Keeping them needs the
    limit in the id, which can be added later.
11. **`validate` on an invalid model prints the report on stdout and the errors on stderr
    (§9).** *Recommended:* yes.
12. **A stall in M7 (§3).** Items left with no event is exit 2, with nothing stored? *Recommended:*
    yes, since the standard catalog cannot reach it unless something is broken. The alternative is
    a stored result with `complete: false`, which is what v0.2's own stalls may need; its ADR decides.
13. **The check run (§5).** A model that claims to draw nothing runs twice the first time?
    *Recommended:* yes. "Exact" is a strong word, and a declaration alone should not earn it.
14. **The domain package in the run id (§6).** *Recommended:* yes. It costs two constants now and
    every store later.
