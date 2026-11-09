# ADR-0025: KPIs and terminating experiments

- Status: proposed (needs the owner's explicit acceptance)
- Date: 2026-11-08
- Blocks: M8; used by M9 (warm-up, see "What this leaves ready for M9"), M10 (re-runs A2),
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
5. A short seed list needs no budget flag (§4). A reading of ADR-0019 item 4 and spec §5,
   "budgets are mandatory on long-running commands".
6. `metrics[].observable` of ADR-0024 §3 becomes the list `observables` (§2), since one metric
   can answer several declared observables. M7 writes neither, so no M7 byte changes.

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
// taktsim-stats: pure, plain ids, no factory word, and no end until the end.
pub enum Fact { Created { time, item, by }, Moved { time, item, from, to },
                Delivered { time, item, by }, Removed { time, item, by },
                State { time, resource, state: u16, in_progress: u32 },
                Class { item, dimension: u16, class: u16 } }
pub struct PlaceSpec { pub states: u16, pub creation: Creation }
pub struct KpiFold;   // new(&[PlaceSpec], start); push(Fact); finish(end) -> KpiReport

// taktsim-components: required, no default; `Flow` matches every message without `_`.
pub enum MessageFact<'a> { State { name: &'static str, in_progress: u32 },
                           Class { dimension: &'static str, value: u16 },
                           ClassName { dimension: &'static str, value: u16, name: &'a str } }
pub enum Creation { Never, HeldUntilFirstMove, InSystem }
pub enum Kpi { Deliveries, Occupancy, StateShares, InProgress, AwaitingRelease, Transit }
trait Domain { fn read_message(message: &Self::Message, facts: &mut dyn FnMut(MessageFact<'_>)); }
trait ComponentType<D> { const STATES: &'static [&'static str]; const CREATION: Creation;
                         fn places(&self) -> u32; }
impl Registry { fn with_kpi(self, name, signature, kpi: Kpi) -> Result<Self, CatalogError>; }
```

The bridge, `taktsim-run/src/facts.rs`, matches `Record` and `Disposal` exhaustively. M6's
new messages and states do not compile until they answer, so they cannot miscount. It maps
a state name to its position in that component type's `STATES`; a name the type did not
declare is exit 2, type `run.kpi_undeclared_state`. `read_message` calls back, so one message
can state several facts: a v0.2 item has a variant, a product and a material at once.

**The fold learns its end last.** A stored trace states its end in the last record, and a live
`TraceSink` does not know it until the run stops. So `new` takes the start only, and `finish(end)`
the end. Levels are integrated exactly, as integer item-nanoseconds in a `u128`, which cannot
overflow since a `u64` level times a `u64` span fits. `TimeWeightedAccumulator` needs its window
when built, so the fold does not use it (disagreement 11); a test requires their means to agree. An
empty window gives a report with no time-weighted value, so that path runs through the fold.
A trace with no end marker is refused, as ADR-0022 §9 says.

**States belong to a component type.** `MachineState` and `SourceState` already differ, and a
calendar's or an operator's will differ again. `STATES` sits next to `PORTS`, so a new state on a new
type adds no row, and no byte, to a line that does not have that type. Each `state_shares` row list
is its type's, in its order.

**A class has a dimension.** `{variant: a}` comes from `dimension: "variant"` on the fact, so per-product
and per-material rows need no second label and no change to this API.

**One table of levels.** `ItemCreated` adds one at `by`. An event with an item and
`from ≠ target` moves it. `ItemDisposed` removes it at `by`. A level below zero, or an event for
an item that is not at `from`, is exit 2, type `run.kpi_conservation`. So at every record
`Σ levels = created − disposed`, and at the end `Σ levels = items_alive`, which ties the new KPIs to
M7's counters.

**Release is what the creator's type says.** ADR-0022 §7 defines release as a part leaving the
Source. `Ports` decides nothing, because a type's ports do not say what its output means:

| `CREATION` | Types | In `held(<id>)` | Released, so WIP and lead time start |
| --- | --- | --- | --- |
| `Never` | Machine, Buffer, Sink | | creating an item here is exit 2, `run.kpi_conservation` |
| `HeldUntilFirstMove` | Source | from creation to its first move | at its first move |
| `InSystem` | none yet; Assembly and Split in v0.2 | never | at creation |

`taktsim-stats` has its own `Creation`, since neither crate depends on the other, and the bridge maps them
exhaustively. An item disposed before release leaves `held` and enters neither `wip` nor lead time, so system WIP
cannot go below zero. A chained assembly's lead time needs the lineage records of spec §4; the v0.2 ADR
for it decides that, and until then `InSystem` starts at creation, which no v0.1 model reaches.

## 2. The KPIs

| Metric | Unit | `over` | Rows | Definition |
| --- | --- | --- | --- | --- |
| `throughput(<id>)` | `items/s` | window | one; per dimension with two or more classes, one more per class, label `{<dimension>: <name>}` | delivered by it in the window ÷ window. The class is the one on the event that carried the item |
| `state_shares(<id>)` | `share` | window | one per name in its type's `STATES`, always all, labels `{resource, state}` | integer nanoseconds in that state ÷ window, from the start note to the stated end |
| `utilization(<id>)` | `share` | window | one | time-weighted units in a cycle ÷ `places`. ADR-0022 §5, and what ADR-0023's `lowest_utilization` ranks |
| `wip(<id>)` | `items` | window | one | time-weighted mean of its level |
| `wip()` | `items` | window | one | time-weighted mean of released and not yet disposed |
| `held(<id>)` | `items` | window | one, 0 if nothing was created | time-weighted mean of items created, not yet released. ADR-0022 §7's "reported apart" |
| `lead_time(<id>)` | `s` | window | §3 | release to `ItemDisposed` delivered |

**`busy` is not utilization.** ADR-0022 §5: any unit in a cycle makes a machine `busy`. At capacity 2
and ρ = 0.8, M/M/2 gives `state_shares{busy}` = P(N ≥ 1) = 8/9 and `utilization` = 0.8. So both are
listed; at capacity 1 they are equal, which a test requires. `units_in_cycle` is on the note today, 0 while
down; `in_progress` carries it through the fold, and `places` is `capacity`. `places` is required, and is 1 for a
type that does no work, because a wrong default would state a wrong number. Pools come after M6.

The metrics of M7 keep their names, order and bytes, and the new ones follow `items_alive`, sorted by
KPI name and then in model order. `items_alive` is not WIP: at the end `items_alive = wip_end + Σ held_end`.
A test pins M7's rows byte for byte. Every KPI's metric name is the canonical text of an observable
that answers it, which a test requires, so `wip()` and not a bare `wip`.

**Every row can be asked for.** `standard()` registers:

| Name | Arity | Accepts | `Kpi` |
| --- | --- | --- | --- |
| `throughput` | 1 | Sink | `Deliveries` |
| `lead_time` | 1 | Sink | `Transit` |
| `wip` | any number: `wip()` the system, `wip(a, b)` each | Buffer, Machine | `Occupancy` |
| `held` | 1 | Source | `AwaitingRelease` |
| `state_shares` | at least 1 | Machine, Source | `StateShares` |
| `utilization` | 1 | Machine | `InProgress` |

So a whole-line target loads: `precision: {wip(): 5%}`. `Arity::AtLeast(0)` exists and the parser takes
`wip()`, so the expression language does not change. `wip(sink)` stays refused. M5 registered `wip` for
Buffer only; ADR-0022 §9 already lists a machine's units as enough from the trace, and M8 is where
ADR-0015 answer 5 settles these signatures.

**Which are listed.** Every registered KPI for every component its signature accepts, whether
declared or not, so `Signature::accepts` decides and `taktsim-run` names no `Buffer` or
`Machine`. Listing only what is declared would give a smaller document, but an agent asking a new
question would need another run, because `observe` is in the model hash. The fold computes them anyway.

**A package brings its KPIs as data.** `Registry::with_kpi` replaces `with_observable`, so a registered
observable always has an answer, and `metrics.rs` takes names from the registry and matches `Kpi`
exhaustively; it holds no `throughput` or `wip`. `with_kpi("census", …, Kpi::Occupancy)` gives a hospital
package its row, under its word, and without `wip` or `held` rows it did not register. `Domain` cannot
instead map a `KpiReport` to rows, because `taktsim-components` does not depend on `taktsim-stats`, and
adding that would loosen the gate of §1. A new kind of computation, such as OEE, is a new `Kpi` variant and
a change in `taktsim-stats` and `taktsim-run`. That is by design: neither is the engine, and principle 7 is
about the engine.

**Several declarations, one metric.** The loader takes `state_shares(cut)`, `state_shares(cut, weld)` and
`state_shares(weld, cut)` as different texts, and `observe` and `precision` are checked apart. So a metric has
`observables`, a sorted list of every declared text it answers, left out when empty. It replaces ADR-0024 §3's
single `observable`, which M7 never writes; ADR-0024's rule then reads "each declared observable is in
some metric's `observables`, or in `unreported`". Refusing the overlap at load would narrow ADR-0015.

**Units** are the base ones, `s` and `items/s`, so the bytes need no conversion. Text
rendering may also show per hour. Inside the code they are typed (`Rate`, `SimDuration`,
`Ratio`), with `wip` and `held` the recorded exception of ADR-0014 §6.

**Types (ADR-0015 answer 5).** `throughput` a `Rate`; `lead_time` a time, the mean row;
`wip` and `held` a number, as M5 registered `wip`, because a time-weighted mean is not a whole count;
`utilization` a number; `state_shares` is not a scalar, so the v0.4 ADR that evaluates expressions
over observables refuses it there. M8 evaluates no expression, and neither a model nor a hash changes.

**Undefined values.** Only two things have no value. A run with no delivered unit has no lead time, and
a run cut at time zero has an empty window, so no time-weighted row. Those rows are left out of that run's
document, and a declared observable goes to `unreported` with `reason: undefined`. So `unreported` is
empty unless a value truly does not exist, and ADR-0024's rule still holds. Everything else has a true
value: `held` of a Source that created nothing is 0, and `throughput` of a Sink that received nothing
is 0.

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
| `--seed 1..20` or a repeated `--seed`, no precision, at most 1,000 seeds | fixed list | the list |
| the same, 1,001 to 100,000 seeds | fixed list | `--max-seconds` and `--max-runs` at least the list's length, both required; otherwise refused, exit 1, `run.no_budget` or `run.budget_too_small` |
| more than 100,000 seeds, or `--max-runs` above 100,000 | refused, exit 1, `run.budget_too_large`, before any allocation | |
| precision, `--seed s` or `model.seed`, `--max-runs n`, `--max-seconds t` | sequential over `s, s+1, …, s+n−1` | both, required (ADR-0019 item 4) |
| precision without either flag | refused, exit 1, `run.no_budget`, one line per missing flag | |
| precision and a seed list | refused, exit 1, `run.seed_list_with_precision` | |
| a model that draws nothing, with any of the above | one run, `no_random_draws`, each unused flag a warning | none |

`--max-seconds` also works on a fixed list. `--max-runs` below a list's length is exit 1,
`run.budget_too_small`, not a silent cut. Refusing list-plus-precision is the end that can
widen later without breaking anything.

**A list is a budget only while it is short.** Spec §5 wants no agent to start an unbounded loop, and
`--seed 1..1000000` on an 8 h model is close to one. A range's length comes from checked arithmetic on its
two ends before anything is expanded, so `--seed 0..9007199254740991` is exit 1 and never an allocation
failure. The two limits are round numbers, not measurements: 1,000 is far above A2's ten seeds and any list
written by hand, and 100,000 keeps a list at 800 KB and the value matrix of §8 at 800 KB per row. Both
can widen later without breaking anything. The reading is listed above and in disagreement 10.

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
back from its stored `runs/<id>/result.json`, fresh or reused, so both follow one path. That
document is a cache of the fold over `[0, end]`, and M8 needs no other window. Windows of counted
runs must be equal, or exit 2.

**The rows come from the model, not from a run.** The order and the set must not depend on what the
lowest seed happened to do. Each row has a key, (position of the component in the model, KPI, label), with M7's
counters and `items_alive` in M7's order and §2's order after them. The document's rows are those the registry
and the model determine, plus any `scrapped`, `consumed` or class row that some counted run has, each at its
key's place. Then, for a run without a row:

| Row | Value in that run |
| --- | --- |
| a count or a rate, including `throughput` and its class rows | 0, a true value |
| `wip`, `held`, `state_shares`, `utilization` | always present, or absent in all runs (windows are equal) |
| `lead_time` | undefined |

So with `interarrival: {dist: exponential, mean: 2h}`, `horizon: 8h` and `--seed 1..20`, an empty seed 1 removes
nothing: `held(source)` averages over 20 with a true 0, and `lead_time(sink)` keeps its place. A rare
`scrapped(qc)` is present with its zeros if any counted run scrapped. A model that declares `lead_time(sink)`
gets `unreported` only if no run at all defines it.

`Accuracy::from_replications(values, plan)`, over one `Option` per counted run, is the only way to a
replicated value:

| Case, over the defined values | `value` | Accuracy |
| --- | --- | --- |
| at least 2, not all equal | mean | `sampling` from `ConfidenceInterval`; `stopping: sequential` if the mode was |
| at least 2, all equal | that value | `none`, `no_variation_observed` (§6) |
| one, of one run | that value | `none`, `single_replication` |
| one, of several runs | that value | `none`, `one_defined_value`: how many ran, and why more seeds may not help |
| none | row left out | a declared observable is in `unreported`, `undefined`, with the count |
| a model that draws nothing | M7's path | `none`, `no_random_draws`; `target` stated, `reached` left out, since no interval exists to compare |

If k of n runs left a row undefined, the row carries a further entry `{kind: partly_undefined, undefined: k, of: n,
detail}`, whatever the first entry says. Its mean is over runs that delivered something, so it is conditional on
that and biased against an unconditional mean; the entry states it, in the way §3's `censored` does.

A **target row** is one that a declared `precision` covers, among: `throughput(<id>)` total, `wip()`,
`wip(<id>)`, `held(<id>)`, `utilization(<id>)`, the mean row of `lead_time`, and every row of `state_shares`. So a rare
state's share may never reach 2 %, and the result then says so through `budget_exhausted`. One
`ReplicationControl` runs per target row, and the experiment stops when none says `Continue` (ADR-0014 §3). A row
undefined in some replication never reaches its target.

**The tightest target governs.** `precision: {state_shares(cut): 2%, state_shares(cut, weld): 5%}` gives the rows of
`cut` 2 % and the rows of `weld` 5 %. A half-width within 2 % is also within 5 %, so `reached` on the tightest
answers both, while `target` states the one that governed and a reader can compare the half-width with the other.

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

**Measured** (`crates/taktsim-run/tests/coverage.rs`; the seeds are fixed, so the counts are the same on
every target): A2 covers 299/600 in 948 of 1,000 fixed-n experiments. The sequential procedure covers it
in 930 of 1,000, with a mean of 16.32 replications per experiment, near the estimate of 15. Beside
ADR-0014's 94.35 % on normal results at γ = 5 %, 93.0 % here at γ = 3 % is consistent with the
shortfall §7 expects from stopping soon after n₀.

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
  can drive it through every finishing order. It keeps the **value matrix**, one `Option<f64>` per
  committed seed and row. The stop decision is a function of that matrix, the targets and n₀. The
  `ReplicationControl`s are an incremental form of it, and `rebuild(&matrix)` replays them, which a
  property test requires to agree bit for bit.
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

`RunRequest` stays `Copy`: `seeds: &'a [u64]`, empty for the model's, expanded only after §4's length check.

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
  table: 639 parts, WIP 22, 16.5 min; and ADR-0022's capacity-2 check, utilization + down = 1
  (0.956 + 0.044), with `utilization_is_not_the_busy_share`.
- **Row sets:** `row_set_does_not_depend_on_the_lowest_seed` (the empty run first, in the middle, and last,
  giving rows in one order), `true_zero_is_averaged`, `partly_undefined_states_the_count`,
  `one_defined_value_of_several`, `undefined_in_all_goes_to_unreported`.
- **Declarations:** `overlapping_observables_are_all_answered`, `tightest_target_governs`,
  `system_wip_and_held_can_be_targeted`, `every_kpi_name_is_an_observable_text`.
- **Packages:** `a_package_gets_only_the_kpis_it_registers`, `state_rows_follow_the_type`,
  `a_new_state_changes_no_other_type`, `undeclared_state_is_refused`, `two_dimensions_two_row_sets`,
  `created_by_a_never_type_is_conservation`, `created_in_system_is_wip_at_once`,
  `disposed_before_release_touches_no_wip`.
- **The fold:** `one_pass_needs_no_end`, `cut_at_zero_goes_through_the_fold`,
  `fold_mean_agrees_with_time_weighted_accumulator`, `fold_from_start_equals_the_cut_window`.
- **Budgets:** `huge_range_is_refused_before_allocating`, `long_list_needs_both_budgets`,
  `max_runs_below_list_is_refused`.
- **Properties:** `Σ levels = created − disposed` at each record; state nanoseconds sum to the window;
  classes sum to the total in each dimension; on a drained run the WIP integral equals the sum of lead
  times, exact in integer nanoseconds (Little's law, finite form); `Sequencer` over every finishing order
  gives one decision and one committed set; `rebuild_equals_incremental`; a push order never reaches the
  document.
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

- `taktsim-stats` gains the KPI fold; `Domain` gains `read_message`, `ComponentType` gains `STATES`, `CREATION`
  and `places`, all required; `Registry::with_kpi` replaces `with_observable`; `standard()` registers `held` and
  `utilization` and widens `wip` and `state_shares`, so `describe` and the model schema regenerate;
  `taktsim-run` gains the coordinator. `Cargo.lock` gains `rayon` and its crates, and `ctrlc` and its crates, named in the PR.
- `ci/check-crates.sh` gains `allowed taktsim-stats "taktsim-units taktsim-core"` (tightens, the second
  for its dev-dependency); `gate-change`.
- Three existing tests change because ADR-0024 said they would, each saying so in its commit, and none
  loses strength: `declared_observable_is_answered_or_unreported` passes a budget and now requires
  answers, not `not_computed`; the `--seed` repeated case moves to a duplicate seed; CLI snapshots gain
  rows, while a new test pins M7's rows unchanged. `labels_and_observable_round_trip` round-trips the list.
- Coverage in A2 is on fixed n; users of `precision` get `stopping: sequential`.
- Not done, by intent: warm-up and steady state (M9), benchmark validation (M10), `compare` (M11), a
  histogram row, a bound on no-variation rows.

**After M6 lands** (in the PR, for the lead session): pool utilisation; per-variant and per-product KPIs
across routes, and a `Class` for items whose variant is not on the first `Arrive`; `read_message`, `STATES`,
`CREATION` and `places` for its additions, which will not compile until answered, with `places` meeting ADR-0023's
own count; `wip` for what its signature then accepts; `branch_merge`, `rework_loop`, `pool` in the reference
experiments; `Domain` and `ComponentType` items on both branches.

## What this leaves ready for M9

ADR-0014 leaves open which series MSER-5 reads, and its answer 3 prefers trying the mean over several
replications first. Then the warm-up end w is known only after several runs, and can move as seeds join. M8 does
not decide this, and does not close it off:

- **A run's document stays immutable, and over `[0, end]`.** It is a cache for w = 0 only. For w > 0 the experiment
  gets its values by running the fold over the stored trace with `KpiFold::new(places, w)`, which
  `fold_from_start_equals_the_cut_window` proves on hand-written traces.
- **The stop decision is a function of the value matrix** (§8). When w moves, M9 replaces the matrix and calls
  `rebuild`. The prefix rule stays: statistics depend only on the seeds so far.
- **Equal windows** (§5) remain M8's rule. If M9 detects per replication, windows differ and `metrics[].over`, which holds
  one, must change; that is a format question for M9's ADR. The mean-series reading needs no such change.

## What M8 redoes if earlier answers change

| If the owner answers otherwise | M8 redoes |
| --- | --- |
| ADR-0024 Q1: `none` is not a result | all of §5's table and §6; `run` with one seed reports nothing, so per-run documents go and the experiment reads the fold |
| Q2: `--seed` over `model.seed` refused | §4: no base seed over a stated `model.seed`; the model must drop it |
| Q3: no-draw models may take seeds | §5's guard moves from "no seeds" to the check in §6 |
| Q4, Q5, Q14: the header or the run id | golden results and pinned ids regenerate; no logic |
| Q8: result documents closed | `stopping`, `censored`, `partly_undefined` and `held` need a format bump |
| ADR-0024 keeps the single `observable` | overlapping declarations must be refused at load instead, which amends ADR-0015 |
| Q10: partial runs kept | a cut run could stay, under an id with its limit; §8's prefix rule is unchanged |
| Q12: stalls become results | a stalled replication joins `event_limit`'s row, not a failure |
| Q13: no check run | nothing here |
| ADR-0022 T1 declined (T2) | the bridge reads self-messages; the fold is unchanged |
| ADR-0022 §7: lead time from creation | one line in the bridge, every golden result |

## One-way doors

Before v0.1 ships each costs regenerated files; the cost below is after that.

| Door | Commits to | Cost to reverse | Test that proves it |
| --- | --- | --- | --- |
| KPI names, units, rows and labels of §2 and §3, including `held`, `utilization`, `wip()`, `share`, `items/s`, `statistic` | every reader, M11, later KPIs | a deprecation; semver major for a renamed field | `m7_rows_are_unchanged`, `every_kpi_name_is_an_observable_text`, `kpis.rs`, golden results |
| The observable signatures of §2: `wip` of any arity, `held`, `utilization`, what each accepts | every model that declares them | loading fails on models; accepting more later is free | `system_wip_and_held_can_be_targeted`, model schema freshness |
| `metrics[].observables` as a sorted list; tightest target governs a shared row | every reader of ADR-0024's rule, M11 | a format change | `overlapping_observables_are_all_answered`, `tightest_target_governs` |
| Row set and order from the model, with 0 for a missing count | every experiment document | every golden result | `row_set_does_not_depend_on_the_lowest_seed`, `true_zero_is_averaged` |
| KPI definitions: release per `CREATION`, nearest-rank quantiles, delivered only, `utilization` over `places` | every number, every comparison with a factory's | every result changes | `kpis.rs`, `disposed_before_release_touches_no_wip`, `wip_integral_equals_lead_times`, `page_1_table`, `utilization_is_not_the_busy_share` |
| Accuracy words `no_variation_observed`, `one_defined_value`, `partly_undefined`, `undefined`, `censored`, `stopping` | what a number claims | a deprecation | `identical_replications_state_no_interval`, `partly_undefined_states_the_count`, `censored_lead_time_says_so`, `sequential_says_so` |
| No zero-width `sampling`; `reached` on it | every reader of `reached` | a format change | `no_golden_result_has_a_zero_width_sampling` |
| Stop words, `complete` for each, exit codes in §5 | scripts and agents | semver major | `endings_table`, CLI contract |
| Flags `--seed` list and range form, `--max-runs`, `--max-seconds`, `--threads`, `--max-memory-mib`; both budgets required; limits of 1,000 and 100,000 seeds | every script | semver major; requiring less later, and wider limits, are free | `run_without_budget_names_the_flag`, `long_list_needs_both_budgets`, `huge_range_is_refused_before_allocating`, `walkthrough_uses_only_describe` |
| Seed sequence `s, s+1, …`, lists sorted, duplicates refused | which runs an experiment holds; the numbers | every experiment's numbers | `sequence_is_consecutive`, `seed_order_is_sorted`, `duplicate_seed_is_refused` |
| Prefix rule: strict seed order, nothing past the stop stored, no thread count in any byte | determinism of every experiment, and `calibrate` later | a determinism break | `threads_1_and_8_store_the_same_bytes`, `sequencer_any_finishing_order` |
| A2 model, seeds, count, window | the v0.1 claim | never move; a new test beside it | `a2_fixed_n_coverage_on_mm1` |
| `experiments/<id>/result.json` and what is kept | every store, M11 | a store migration | `kept_only_when_determined`, `interrupted_then_resumed_equals_uninterrupted` |
| `Domain::read_message`, `ComponentType::STATES`, `CREATION`, `places`, `Registry::with_kpi`, `Kpi`, `MessageFact`, `Fact`, `PlaceSpec`, `KpiFold::new(places, start)`, `finish(end)` | every later package | every package | doc tests, `flow_reads_every_message`, `a_package_gets_only_the_kpis_it_registers`, `one_pass_needs_no_end` |
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
| 10 | A seed list, unlike a precision run, has no flag budget, against "budgets are mandatory on long-running commands" | spec §5, ADR-0019 item 4 | §4: its length is the budget up to 1,000 seeds, and both flags beyond; ask (question 11) | no |
| 11 | "Keeps one `TimeWeightedAccumulator` per WIP signal" needs the window at the start, and a trace states its end last | ADR-0014 Consequences, `crates/taktsim-stats/src/time_weighted.rs` | §1: exact integer integrals in the fold, and a test that the two agree | no |
| 12 | One `observable` per metric cannot hold ADR-0024's rule when declarations overlap | ADR-0024 §3, `crates/taktsim-model/src/load.rs` | §2: a list, `observables`; the ADR-0024 row changes in this PR | no |
| 13 | "The share of time a station is working" reads as `busy`, and that is not utilization above capacity 1 | `docs/concepts/glossary.md`, ADR-0022 §5 | one added sentence in the glossary, in this PR | no |
| 14 | ADR-0022 §9 lists system WIP from "first `Arrive` from a Source" only; §1 adds `CREATION` for types that do not exist yet | ADR-0022 §7, §9 | §1: equal for every v0.1 model; the v0.2 ADR settles lineage | no |

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
11. **A short seed list without budget flags (§4).** *Recommended:* yes, up to 1,000 seeds: the list states how much
    work it asks for, and a signal still stops it. The alternative, both flags on every list, is stricter and can be
    loosened later, but costs every `--seed 1..20` two flags.
12. **Replacing `observable` with `observables` (§2).** *Recommended:* yes, while nothing has been released.
