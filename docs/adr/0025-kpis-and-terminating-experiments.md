# ADR-0025: KPIs and terminating experiments

- Status: proposed (needs the owner's explicit acceptance)
- Date: 2026-11-08
- Blocks: M8; used by M9 (warm-up cuts the same windows), M10 (re-runs A2),
  M11 (`compare` reads experiments), M12 (Python calls the same API)
- Spec: §2 principles 2 to 4, §4 Observables and KPIs, §5, §6 Output analysis, §7,
  §15; ADR-0007, ADR-0010, ADR-0013, ADR-0014, ADR-0015, ADR-0019, ADR-0022, ADR-0024

## Why explicit acceptance

Most of this settles what M8 needs. These points read or narrow the spec or an
accepted ADR, so the whole needs the owner's answer:

1. A KPI with identical replications states no half-width (§5). Narrows spec §6, "the
   report states the achieved confidence interval half-width", and spec §5.
2. A signal cuts running replications at once, and only finished ones count (§9). A
   reading of ADR-0019 item 4, "stop at the next replication boundary".
3. The wall clock can shorten an experiment (§9). A reading of principle 3, since
   ADR-0019 item 4 requires that budget.
4. ADR-0015 answer 5 leaves the observables' types to M8; `wip` stays a number, which
   spec §6's `wip(buf1) <= 30 parts` does not type-check against (§2).

ADR-0022 and ADR-0024 are still proposed, and M8 is built on them at the owner's
instruction. "What M8 redoes" below lists the cost if their answers change.

## Context

M7 gives one run a typed result that says it has no error bound. M8 fills it: the KPIs of
spec §4 and experiments over several seeds, with `taktsim_stats::ConfidenceInterval` and
`ReplicationControl`, which exist already and are not rewritten. ADR-0024 fixes the
document, its accuracy tags, the ids and the store. This ADR decides only what they leave
open. Times marked "estimate" have not been measured; the implementation states the
measurements in the pull request.

## 1. Where the KPI fold lives

KPIs come from core's records, and state shares also from notes, whose message belongs to
the domain. `ci/check-crates.sh` does not let `taktsim-components` depend on `taktsim-stats`,
and stats is pure numbers (ADR-0014), so it should not depend on components either.

| | F1. Fold in `taktsim-stats`, fed through a bridge in `taktsim-run` | F2. Fold in `taktsim-components` | F3. Fold in `taktsim-run` |
| --- | --- | --- | --- |
| Gate change | tightens: a new line pins stats | loosens: components gains stats | none |
| Roadmap and spec §10 (KPIs in stats) | yes | no | no |
| Tested without an engine or a domain | yes | no | no |
| Factory words in the fold | none | yes | none |

**F1.** ADR-0022 §9 recommended the facts, with the fold in components. The neutral
vocabulary stays and the bridge moves one crate up, for the gate.

```rust
// taktsim-stats: pure, plain ids, no factory word.
pub enum Fact { Created { time, item, by }, Moved { time, item, from, to },
                Delivered { time, item, by }, Removed { time, item, by },
                State { time, resource, state: u16 }, Class { item, class: u16 } }
pub struct KpiFold;   // new(places, state_count, Window); push(Fact); finish() -> KpiReport

// taktsim-components: required, no default, `Flow` matches every message without `_`.
pub enum MessageFact<'a> { Nothing, State(&'static str), Class(u16), ClassName(u16, &'a str) }
trait Domain { const STATE_NAMES: &'static [&'static str]; const CLASS_LABEL: &'static str;
               fn read_message(message: &Self::Message) -> MessageFact<'_>; }
```

The bridge, `taktsim-run/src/facts.rs`, matches `Record` and `Disposal` exhaustively. M6's
new messages and states do not compile until they answer, so they cannot miscount. The fold
reads the stored trace and takes its end from the trace's end marker; a trace with no end
marker is refused, as ADR-0022 §9 says. A live `TraceSink` can feed the same fold later.

**One table of levels.** `ItemCreated` adds one at `by`. An event with an item and
`from ≠ target` moves it. `ItemDisposed` removes it at `by`. An item's first move is its
release, by whichever component created it: `Ports` decides nothing, as in M7. A level below
zero, or an event for an item that is not at `from`, is exit 2, type `run.kpi_conservation`.
So at every record `Σ levels = created − disposed`, and at the end `Σ levels = items_alive`,
which ties the new KPIs to M7's counters.

## 2. The KPIs

| Metric | Unit | `over` | Rows | Definition |
| --- | --- | --- | --- | --- |
| `throughput(<id>)` | `items/s` | window | one; with two or more classes, one more per class, label from `CLASS_LABEL` | delivered by it in the window ÷ window. The class is the one on the event that carried the item |
| `state_shares(<id>)` | `share` | window | one per name in `STATE_NAMES`, always all, labels `{resource, state}` | integer nanoseconds in that state ÷ window, from the start note to the stated end |
| `wip(<id>)` | `items` | window | one | time-weighted mean of its level (`TimeWeightedAccumulator`) |
| `wip` | `items` | window | one | time-weighted mean of released minus disposed |
| `held(<id>)` | `items` | window | one, for a component that created items | time-weighted mean of items created, not yet released. ADR-0022 §7's "reported apart" |
| `lead_time(<id>)` | `s` | window | §3 | first move to `ItemDisposed` delivered |

The metrics of M7 keep their names, order and bytes, and the new ones follow `items_alive`.
`items_alive` is not WIP: at the end `items_alive = wip_end + Σ held_end`. A test pins M7's
rows byte for byte.

**Which are listed.** Every KPI for every component its catalog signature accepts, whether
declared or not, so `Signature::accepts` decides and `taktsim-run` names no `Buffer` or
`Machine`. A declared observable tags its metrics in `observable`: `state_shares(cut, weld)`
tags `state_shares(cut)` and `state_shares(weld)`. Listing only what is declared would give
a smaller document, but an agent asking a new question would need another run, because
`observe` is in the model hash. The fold computes them anyway.

**Units** are the base ones, `s` and `items/s`, so the bytes need no conversion. Text
rendering may also show per hour. Inside the code they are typed (`Rate`, `SimDuration`,
`Ratio`), with `wip` and `held` the recorded exception of ADR-0014 §6.

**Types (ADR-0015 answer 5).** `throughput` a `Rate`; `lead_time` a time, the mean row;
`wip` a number, as M5 registered it, because a time-weighted mean is not a whole count;
`state_shares` is not a scalar, so the v0.4 ADR that evaluates expressions over observables
refuses it there. M8 evaluates no expression, and neither a model nor a hash changes.

**Undefined values.** A run with no delivered unit has no lead time. A run cut at time zero
has an empty window and no time-weighted row. Their rows are left out, and a declared
observable goes to `unreported` with `reason: undefined`. So `unreported` is empty unless a
value truly does not exist, and ADR-0024's rule still holds.

## 3. Lead time as a distribution

| | L1. Exact: keep every lead time as integer nanoseconds | L2. Log-spaced histogram | L3. P² estimators |
| --- | --- | --- | --- |
| Memory per run | 8 bytes per delivered unit | fixed | fixed |
| Value is the exact one, hand-checkable | yes | within a bin | no |
| Depends on record order | no | no | yes |
| Stated error bound | none needed | yes | no |

**L1.** 8 bytes per unit is 80 MB for ten million delivered units, a size no v0.1 target
reaches, during the fold only. The trace still streams to disk. L2 can replace it behind
the same rows if a model needs it, at the price of new golden results.

Rows, labels `{statistic: …}`: `mean`, `min`, `p50`, `p90`, `p95`, `p99`, `max`. Quantiles
are nearest rank, `sorted[⌈p·n⌉ − 1]` in integer arithmetic, so each is a value that occurred
and every target gets the same one. Across replications each row has its own interval: an
interval for the expected p90 of a run, not a pooled p90. A distribution over units is
not independent; replications are.

**Censoring.** Lead time covers delivered units only, which understates it while a line
fills (ADR-0022 §9 says the report must state this). If units were inside at the end, each
row carries a second accuracy entry: `{kind: censored, units_inside: <mean>, detail}`. This is
the extension ADR-0024 §4 made room for: a reader that does not know the kind ignores the
entry, and the interval is still read.

## 4. Seeds and budget: what the flags ask for

| Asked | Mode | Budget |
| --- | --- | --- |
| one seed, no `experiment.precision` | one run, M7's document | none |
| `--seed 1..20` or a repeated `--seed`, no precision | fixed list | the list |
| precision, `--seed s` or `model.seed`, `--max-runs n`, `--max-seconds t` | sequential over `s, s+1, …, s+n−1` | both, required (ADR-0019 item 4) |
| precision without either flag | refused, exit 1, `run.no_budget`, one line per missing flag | |
| precision and a seed list | refused, exit 1, `run.seed_list_with_precision` | |
| a model that draws nothing, with any of the above | one run, `no_random_draws`, each unused flag a warning | none |

`--max-seconds` also works on a fixed list. Refusing list-plus-precision is the end that can
widen later without breaking anything.

**Seed order.** A list is sorted before anything uses it, and a duplicate is exit 1. The
experiment id is over sorted run ids, but Welford's mean depends in its last bits on push
order, so without sorting two orders of one set would give one id and two documents.

| | S1. `s, s+1, …` | S2. Hash of `(s, i)` | S3. A stream of seeds from `s` |
| --- | --- | --- | --- |
| A person can read `provenance.seeds` | yes | no | no |
| Experiments share runs through the store | yes | no | no |
| Independent streams | yes: Philox keys on the seed (ADR-0013) | yes | yes |
| Experiments from nearby `s` overlap | yes | no | no |

**S1.** The overlap matters only when comparing designs, which is for `compare` (M11) and
v0.4 to refuse or use. An upper end above 2^53 − 1 is exit 1, as for one seed.

`n₀` is `DEFAULT_MIN_REPLICATIONS` (10), with no flag. `--max-runs` below it is exit 1,
`run.budget_too_small`. The level is 95 %, with no flag. `--max-events` holds per replication.

## 5. The experiment document

Same format, `taktsim-result/1`. `runs` has one entry per counted replication, in seed
order. `provenance.seeds` lists exactly those seeds. Each counted run's metrics are read
back from its stored `runs/<id>/result.json`, always, fresh or reused, so both follow one
path. Rows are matched by name and labels, in the first run's order; a row missing from a
run is zero for M7's counters, which show only non-zero rows, and "undefined" otherwise (§2).
Windows of counted runs must be equal, or exit 2.

`Accuracy::from_replications(values, plan)` is the only way to a replicated value:

| Case | `value` | Accuracy |
| --- | --- | --- |
| at least 2 values, not all equal | mean | `sampling` from `ConfidenceInterval`; `stopping: sequential` if the mode was |
| at least 2 values, all equal | that value | `none`, `no_variation_observed` (§6) |
| one value | that value | `none`, `single_replication` |
| a model that draws nothing | M7's path | `none`, `no_random_draws`; `target` stated, `reached` left out, since no interval exists to compare |

`target` and `reached` are on target rows: `throughput(<id>)` total, `wip`, `held`, the mean
row of `lead_time`, and every row of `state_shares`. So a rare state's share may never
reach 2 %, and the result then says so through `budget_exhausted`. One `ReplicationControl`
runs per target row, and the experiment stops when none says `Continue` (ADR-0014 §3). A row
undefined in some replication never reaches its target.

| Ending, decided in seed order | `complete` | Top `stop_reason` | Exit | In `experiments/` |
| --- | --- | --- | --- | --- |
| the list finished | `true` | `seeds_finished` | 0 | yes |
| every target reached | `true` | `precision_reached` | 0 | yes |
| `--max-runs` used up, targets not reached | `false` | `budget_exhausted` | 3 | yes |
| `--max-seconds`, under targets | `false` | `budget_exhausted` | 3 | yes |
| `--max-seconds`, on a list | `false` | `budget_exhausted` | 3 | no |
| a replication stopped by `--max-events` | `false` | `event_limit` | 3 | no |
| a signal | `false` | `cancelled` | 4 | no |
| a replication failed | no document | | 2 | no; the counted runs stay |

The stderr `progress` line says which budget ended it and how many of the maximum ran, so
the document stays a function of the counted runs. The library returns a typed
`Ended`, so the exit code is never parsed from the open word `stop_reason`.

**Fourteen out of twenty.** Fourteen `runs` entries, `seeds` of fourteen, an experiment id
over those fourteen, `complete: false`, `budget_exhausted`, each interval over n = 14, and
`reached` per target. Run again with a larger budget and the fourteen are reused.

**A cut run in the document.** Partial runs enter no statistic and no list, or ADR-0024's "an
experiment id exactly when every run has a `run_id`" would remove the id. The one exception
is no finished replication at all: `runs` must hold at least one, so the document is the
lowest seed's partial one, M7's shape, with no ids.

## 6. A KPI that does not vary, in a model that draws

Example: a downstream machine's `down` share in a line where only arrivals are random, or a
count that happens to agree. `half_width = 0` would read as certainty, like `35.5 ± 0.000000`.

| | D1. `sampling` with width 0 | D2. `none`, `no_variation_observed` | D3. As D2, and keep running to the budget |
| --- | --- | --- | --- |
| Reads as certainty | yes | no | no |
| Spec §6 "states the half-width" | yes | no: narrowed, and listed above | no |
| Agrees with ADR-0014's stop rule | yes | yes | no: amends it |
| Budget spent on a KPI that cannot vary | none | none | all of it, always |

**D2.** The detail says that all n replications gave this value and that a zero-width
interval would not be certainty, since a rarer outcome may not have shown. A target row
states `reached: true`, because ADR-0014's rule is met, and its accuracy says what that is
worth. "All equal" compares the values, not `std_dev`. Text prints `12 (same in all 20
replications, no interval)`. A bound like `1 − 0.05^(1/n)` can join as an additive field if
readers want a number.

Two guards: no other code builds a `sampling` entry for a result, and a test walks every
golden document for a `sampling` entry with width zero. A model that draws nothing never
reaches this case, because it has no seeds to replicate over (ADR-0024 §4, §5).

## 7. What sequential stopping does to coverage

The t interval assumes n is fixed. Under a precision rule, n depends on the sample: a run of
low-variance results stops early, so short experiments tend to have understated half-widths.
Coverage falls below nominal. ADR-0014 measured 94.35 % ± 0.05 % on normal results at n₀ = 10
and γ = 5 %. It shrinks as γ falls and n₀ rises, and is worst when the rule stops soon after
n₀. Stopping also affects KPIs without a target, since they are read at a stopping time
chosen by another KPI.

So the document does not claim a plain 95 %. Every `sampling` entry of a sequential experiment
has `stopping: sequential`, including `budget_exhausted` and rows with no target, and text
prints "nominal". `level` stays 0.95, what was asked for.

**A2 is proven on fixed-n intervals** (ADR-0014 answer 2): 1,000 experiments, each a list of
10 seeds.

- Model `benchmarks/models/mm1.yaml`: Source exponential mean 2 s, Buffer of capacity 1,000,
  Machine exponential mean 1 s, Sink, horizon 600 s, starting empty. So λ = 0.5, μ = 1, ρ = 0.5.
- True value. Arrivals in [0, T] are Poisson, since the first part appears after the first
  interval (ADR-0022) and the Source is never blocked (the probability of 1,000 waiting is
  about 2⁻¹⁰⁰⁰). Delivered is arrivals minus those inside. The relaxation time is
  1/(μ(1 − √ρ)²) ≈ 11.7 s, so at T = 600 s the transient in E[L(T)] is below e⁻⁵⁰. Thus
  E[delivered(T)] = λT − ρ/(1 − ρ) = 299 and the true throughput is 299/600 items/s. This is a
  terminating result, with no warm-up, so M9 is not needed.
- Seeds: experiment `i` uses `100·i` to `100·i + 9`. Fixed here, never changed. A count outside
  [930, 970] is a finding to report.
- Pass probability for a correct implementation: about 99.7 % (binomial, as in ADR-0014 §7).

**The sequential procedure** is tested separately on the same model and seeds per block, γ = 3 %,
budget 100, because at about 5.8 % relative spread that stops near n = 15 (estimate), so the
rule really acts. Window for the count: [900, 970], fixed here. It is no A2 claim: the lower
edge says that coverage must not collapse, and the measured count joins this ADR beside
ADR-0014's. It does not move the window.

**Where they run.** Each A2 experiment is 10 runs of about 300 parts: about 3·10⁷ events in all
(estimate). They go through `prepare`, `run_traced` to memory, the fold and
`Accuracy::from_replications`, so 10,000 store directories are not created, and a smaller test
requires the store path to give the same bytes. rayon runs them. Estimate: a minute or two per
test on four CPUs. Proposal: both stay in the required `test` job on every tier-1 target while the
job stays inside its 30 minutes. If not, they move to a new required job on the same targets,
in the same PR, labelled `gate-change`, with the count unchanged. The measurement goes in the PR.

## 8. Parallel replications with the same bytes

The rule that gives everything else: **an experiment is a prefix of its seed order.** Runs enter
the store, the statistics and the stopping decision strictly in that order. A later seed that
finished first waits, and a run past the stopping point never reaches the store.

- **Workers** run each replication single-threaded into `tmp/`, fold it and write its
  `result.json`, then hand over the temp directory. The **coordinator**, one thread, commits in
  seed order, pushes into the controls and decides.
- **Speculation** is at most `threads` seeds past the decision point, so waste is bounded. A
  speculative run past the stop is cut and its temp directory removed. Otherwise extra `runs/`
  entries would depend on the thread count.
- **A pure `Sequencer`** holds the decision logic, with no threads and no clock, so a property test
  can drive it through every finishing order.
- **The lowest seed wins.** A failure or an event limit at seed k cuts only above k. Lower seeds
  finish, so which failure is reported does not depend on timing. Each replication therefore has
  its own cancel flag, which the coordinator sets, polling the outer flag and the deadline every
  100 ms.
- Stored runs are not rerun, and take no thread. `--no-reuse` reruns and compares, as in M7.

| | R1. `rayon` thread pool | R2. `std::thread::scope` and a channel |
| --- | --- | --- |
| New crates | `rayon`, `rayon-core`, `crossbeam-deque`, `crossbeam-epoch`, `crossbeam-utils` | none |
| Named by | spec §9 *default*, roadmap M8 | |
| Work stealing used | barely: tasks are coarse | |
| Later parallel loops (calibration, A2 test) | ready | each its own |
| Panic reaches ADR-0007's path | yes, at scope end | by joining |

**R1,** and this ADR accepts it as the spec requires. R2 would do for M8 alone, which the owner
may prefer (question 7). `rayon` goes in `taktsim-run` only, with `num_threads` always explicit,
so `RAYON_NUM_THREADS` changes nothing and no environment reaches logic. Trace streaming is
unchanged: traces go to disk, and only counts and lead times are in memory.

**Memory (ADR-0010).** The first replication that actually runs goes alone, as ADR-0010's checked
warm-up. Its fold gives an estimate per replication: `16 MiB + 8 B · delivered + 128 B · peak items
alive`. The parallel count is the smallest of the cores, `--threads`, and `--max-memory-mib` (default
2048, spec's target) ÷ estimate, and at least 1. A global-allocator measurement needs `unsafe`,
and reading free memory needs a platform-specific call or a crate. A wrong estimate costs only
speed or memory, never a byte of the result, since the thread count cannot change it. The count
used goes on a `progress` line.

## 9. Signals, the clock and exit 4

`taktsim-cli/src/signal.rs` installs a handler for `run` that sets one `AtomicBool`, passed as
`RunRequest.cancel`. Only that lives in the binary, so Python can set the same flag. `ctrlc` with
its `termination` feature covers SIGINT, SIGTERM, Ctrl+C and Ctrl+Break. An own handler would need
`unsafe`, and `signal-hook` has no Windows console. `cargo deny` has `multiple-versions = "deny"`,
and this PR adds no skip: if a version brings a duplicate crate, the implementation picks another,
or stops and says so.

| | C1. Finish running replications, then stop | C2. Cut them through the engine's hook; count only finished ones |
| --- | --- | --- |
| Time until exit | up to one whole replication, possibly hours | one event plus 100 ms |
| ADR-0019 "stop at the next replication boundary" | to the letter | for the statistics, which end on a boundary |
| ADR-0019 "the stop reaches it through the cancellation hook" | no use for it | yes |
| A single long run | cannot be stopped | stops, partial, as ADR-0024 §3's table says |

**C2.** A cut run is dropped from `tmp/` and only the counted prefix stays. The store is valid at any
moment, because each run is committed by rename. A killed process leaves only `tmp/` debris, which no
reader looks at. Running the same command again reuses the prefix and gives the same bytes as if it
had never been interrupted, which a test requires.

**The clock.** `--max-seconds` is measured by the coordinator with `Instant`, in `taktsim-run`,
never in core, and is in no id. It can only shorten the prefix, and the outcome says so. Under
targets, a clock stop at 14 gives the same bytes as `--max-runs 14` would. A test requires that by
handing the `Sequencer` a passed deadline after 14 results. So the nondeterminism is labelled and confined: the
length of the prefix, never its content.

`describe`: `run` becomes `long_running: true`, gains a `budget` entry naming the flags and when
they are required, and lists exit 4. ADR-0024 §9's "`run` is not long-running" held for M7.

## 10. The store: an id, one document

ADR-0024 §8 wants an id to name one document, and §6 defines the experiment id over run ids alone.
The set `{1..14}` can then come from a finished list (`complete: true`), or from a cancelled list of
twenty (`complete: false`).

| | X1. Store only what the model and run set determine | X2. Add the stop to the id | X3. First writer wins |
| --- | --- | --- | --- |
| The same id, the same bytes | yes | yes | no: depends on history |
| Fourteen of twenty has an id over fourteen (ADR-0024) | yes | no | yes |
| A cancelled result can be found again | no: rerun, and it is reused | yes | sometimes |

**X1**: the last column of §5's ending table. `.taktsim/experiments/<id>/result.json`, written in
`tmp/` and renamed, `store_format` stays 1 (ADR-0024 reserved the name). A cancelled document still
carries `experiment_id`, which names the set, and every run in it is in the store. Later commands
treat "no document here" as "run it again, cheaply". If the document exists it is never overwritten;
with `--no-reuse`, bytes other than `provenance.platform` must match, or exit 2.

`RunRequest` stays `Copy`: `seeds: &'a [u64]`, empty for the model's.

```rust
pub struct Budget { pub max_runs: NonZeroU32, pub max_seconds: NonZeroU64 }
pub enum Ended { Complete, BudgetExhausted, EventLimit, Cancelled }   // RunResponse.ended; exit 0, 3, 3, 4
```

## 11. Same bytes on every target

Statistics use IEEE operations, `sqrt` and `libm` (ADR-0014), in seed order. Lead times are integers.
`Json` prints shortest round-trip numbers and sorted keys. No `HashMap` is iterated. The golden results
are `insta` snapshots with only `provenance.platform` masked, and the `test` job runs them on all four
targets, so one snapshot file proves the bytes equal. No new job is needed.

## 12. Tests

- **KPIs by hand:** `crates/taktsim-run/tests/kpis.rs` writes traces record by record with
  `TraceWriter`, no engine, with expected numbers worked out in comments from the times. Includes
  units inside at the end, a blocked Source's held part, a repair, simultaneous moves. Plus page 1's
  table: 639 parts, WIP 22, 16.5 min.
- **Properties:** `Σ levels = created − disposed` at each record; state nanoseconds sum to the window;
  variants sum to the total; on a drained run the WIP integral equals the sum of lead times, exact in
  integer nanoseconds (Little's law, finite form); `Sequencer` over every finishing order gives one
  decision and one committed set; a push order never reaches the document.
- **A2 and sequential:** `a2_fixed_n_coverage_on_mm1`, `sequential_coverage_on_mm1` (§7).
- **Guards:** `no_draw_model_never_gets_an_interval` (over `--seed 1..5`, also in text: no `±`),
  `identical_replications_state_no_interval`, `no_golden_result_has_a_zero_width_sampling`.
- **Determinism:** `threads_1_and_8_store_the_same_bytes` walks the whole store tree;
  `interrupted_then_resumed_equals_uninterrupted`; `clock_stop_equals_max_runs`.
- **Contract (real binary):** each new flag and error; exit 3 and 4 (a real signal under `cfg(unix)`;
  on Windows the library flag, and the gap stated in the PR); the walk-through from `describe`.
- **Golden results:** `benchmarks/experiments/*.yaml`, a list and a sequential case; a new directory,
  because the reference harness runs each `benchmarks/models` file without a budget.

## Consequences

- `taktsim-stats` gains the KPI fold; `Domain` gains three required items; `taktsim-run` gains the
  coordinator. `Cargo.lock` gains `rayon` and its crates, and `ctrlc` and its crates, named in the PR.
- `ci/check-crates.sh` gains `allowed taktsim-stats "taktsim-units taktsim-core"` (tightens, the second
  for its dev-dependency); `gate-change`.
- Three existing tests change because ADR-0024 said they would, each saying so in its commit, and none
  loses strength: `declared_observable_is_answered_or_unreported` passes a budget and now requires
  answers, not `not_computed`; the `--seed` repeated case moves to a duplicate seed; CLI snapshots gain
  rows, while a new test pins M7's rows unchanged.
- Coverage in A2 is on fixed n; users of `precision` get `stopping: sequential`.
- Not done, by intent: warm-up and steady state (M9), benchmark validation (M10), `compare` (M11), a
  histogram row, a bound on no-variation rows.

**After M6 lands** (in the PR, for the lead session): pool utilisation; per-variant and per-product KPIs
across routes, and a `Class` for items whose variant is not on the first `Arrive`; `read_message` and
`STATE_NAMES` for its additions, which will not compile until answered; `wip` for what its signature
then accepts; `branch_merge`, `rework_loop`, `pool` in the reference experiments; `Domain` items on both
branches.

## What M8 redoes if earlier answers change

| If the owner answers otherwise | M8 redoes |
| --- | --- |
| ADR-0024 Q1: `none` is not a result | all of §5's table and §6; `run` with one seed reports nothing, so per-run documents go and the experiment reads the fold |
| Q2: `--seed` over `model.seed` refused | §4: no base seed over a stated `model.seed`; the model must drop it |
| Q3: no-draw models may take seeds | §5's guard moves from "no seeds" to the check in §6 |
| Q4, Q5, Q14: the header or the run id | golden results and pinned ids regenerate; no logic |
| Q8: result documents closed | `stopping`, `censored` and `held` need a format bump |
| Q10: partial runs kept | a cut run could stay, under an id with its limit; §8's prefix rule is unchanged |
| Q12: stalls become results | a stalled replication joins `event_limit`'s row, not a failure |
| Q13: no check run | nothing here |
| ADR-0022 T1 declined (T2) | the bridge reads self-messages; the fold is unchanged |
| ADR-0022 §7: lead time from creation | one line in the bridge, every golden result |

## One-way doors

Before v0.1 ships each costs regenerated files; the cost below is after that.

| Door | Commits to | Cost to reverse | Test that proves it |
| --- | --- | --- | --- |
| KPI names, units, rows and labels of §2 and §3, including `held`, bare `wip`, `share`, `items/s`, `statistic` | every reader, M11, later KPIs | a deprecation; semver major for a renamed field | `m7_rows_are_unchanged`, `kpis.rs`, golden results |
| KPI definitions: release as first move, nearest-rank quantiles, delivered only | every number, every comparison with a factory's | every result changes | `kpis.rs`, `wip_integral_equals_lead_times`, `page_1_table` |
| Accuracy words `no_variation_observed`, `undefined`, `censored`, `stopping` | what a number claims | a deprecation | `identical_replications_state_no_interval`, `censored_lead_time_says_so`, `sequential_says_so` |
| No zero-width `sampling`; `reached` on it | every reader of `reached` | a format change | `no_golden_result_has_a_zero_width_sampling` |
| Stop words, `complete` for each, exit codes in §5 | scripts and agents | semver major | `endings_table`, CLI contract |
| Flags `--seed` list and range form, `--max-runs`, `--max-seconds`, `--threads`, `--max-memory-mib`; both budgets required | every script | semver major; requiring less later is free | `run_without_budget_names_the_flag`, `walkthrough_uses_only_describe` |
| Seed sequence `s, s+1, …`, lists sorted, duplicates refused | which runs an experiment holds; the numbers | every experiment's numbers | `sequence_is_consecutive`, `seed_order_is_sorted`, `duplicate_seed_is_refused` |
| Prefix rule: strict seed order, nothing past the stop stored, no thread count in any byte | determinism of every experiment, and `calibrate` later | a determinism break | `threads_1_and_8_store_the_same_bytes`, `sequencer_any_finishing_order` |
| A2 model, seeds, count, window | the v0.1 claim | never move; a new test beside it | `a2_fixed_n_coverage_on_mm1` |
| `experiments/<id>/result.json` and what is kept | every store, M11 | a store migration | `kept_only_when_determined`, `interrupted_then_resumed_equals_uninterrupted` |
| `Domain::read_message`, `STATE_NAMES`, `CLASS_LABEL`, `MessageFact`, `Fact`, `KpiFold` | every later package | every package | doc tests, `flow_reads_every_message` |
| `RunRequest`, `Budget`, `Ended` | M12, every client | every client | doc tests |
| `rayon` and `ctrlc` | bill of materials | removing is easy: both are behind one module | `cargo deny check` |

## Spec and code disagreements

| # | What | Where | Recommendation | Blocks |
| --- | --- | --- | --- | --- |
| 1 | An interval with width zero for identical replications would satisfy "states the half-width" | spec §5, §6 | §6: `none`; recorded above | no |
| 2 | "Stop at the next replication boundary" and "through the cancellation hook" pull apart for a run in progress | ADR-0019 item 4 | §9: cut; the statistics stop on a boundary | no |
| 3 | A mandatory clock budget against "same bytes for the same seeds" | ADR-0019 item 4, principle 3 | §9: only the prefix length, labelled | no |
| 4 | "An id must name one document" against an id over run ids alone | ADR-0024 §6, §8 | §10, X1; a note in ADR-0024 | no |
| 5 | "`runs`: at least one" with nothing finished | ADR-0024 §3 | §5: the lowest seed's partial document | no |
| 6 | The fold in components, stats consuming it, against the crate direction | ADR-0022 §9, `ci/check-crates.sh` | §1: the bridge in `taktsim-run` | no |
| 7 | `wip(buf1) <= 30 parts` compares a number with a count | spec §6, ADR-0015 answer 5 | §2: number; the v0.4 ADR settles expressions | no |
| 8 | `taktsim-stats` has no line in `ci/check-crates.sh`, though its direction is in ADR-0014 | `ci/check-crates.sh` | add it | no |
| 9 | Changing a precision target changes the model hash, so every run id: tightening 2 % to 1 % reruns everything, and nothing is reused | ADR-0024 §6, ADR-0002 | leave for M8, and ask (question 8) | no |

## Questions for the owner

Implementation proceeds on the recommendations; each can change before the M8 pull request merges.

1. **Identical replications (§6).** State `no_variation_observed` and no half-width? *Recommended:* yes.
   The alternative is printing `± 0`, which reads as certainty.
2. **`reached: true` with no interval (§6).** ADR-0014's rule is met, so stop? *Recommended:* yes. Running to
   the budget instead spends all of it whenever one target cannot vary.
3. **Both budgets required (§4).** ADR-0019 item 4 read literally: `--max-runs` and `--max-seconds`?
   *Recommended:* yes. Requiring only `--max-runs` later breaks nothing, while adding a requirement would.
4. **A signal cuts running replications (§9).** *Recommended:* yes. An agent or a person should not wait hours.
5. **Cancelled experiments not stored as documents (§10).** *Recommended:* yes. Their runs are kept, and rerunning
   is cheap.
6. **Report every KPI, or only what is declared (§2)?** *Recommended:* every KPI, tagged when declared. It costs a
   longer document, and saves a rerun, since `observe` is in the run id.
7. **`rayon` (§8).** `std::thread::scope` would do for M8 with no new crates. *Recommended:* `rayon`, as the spec and the
   roadmap say and calibration will want.
8. **Precision targets in the run id (disagreement 9).** Should a future run id use a hash without
   `experiment.precision` and `observe`, so tightening a target reuses runs? *Recommended:* yes, as its own change
   before v0.1, because ids can still change cheaply; M8 does not depend on it. A per-run document would then drop its
   `observable` tags and `unreported`, leaving those to the experiment document, and it amends ADR-0024 §3's "two runs
   with different observables cannot share a run id".
9. **Where A2 runs (§7).** In the `test` job, moving to a required job of its own if too slow? *Recommended:* yes,
   count unchanged either way.
10. **`share` and `items/s` as unit words (§2).** *Recommended:* yes: base units, with text showing per hour.
