# ADR-0023: Routing and control

- Status: proposed (needs the owner's explicit acceptance)
- Date: 2026-11-08
- Blocks: M6; used by M7 (`taktsim run`), M8 (KPIs from traces), the later
  `explain` command, and every later component package with an outlet
- Spec: §2 principles 2, 3, 5 and 7, §4 "Routing and control logic", §7; roadmap
  M6; ADR-0009, ADR-0013, ADR-0015, ADR-0018, ADR-0020, ADR-0021, ADR-0022

## Why explicit acceptance

Most of this ADR settles details M6 needs. Three points change the spec or an
accepted ADR:

1. `control` joins the system hash (§2). ADR-0020 §8 lists exactly `format`,
   `components` and `flow`.
2. Spec §4 says every control decision is in the trace with its candidates.
   §9 narrows that twice: orders nobody declared (a Buffer's discipline, the
   oldest offer at a merge) write no record, and a queue above 1,024 units lists
   its best 1,024 and the total.
3. Spec §4's Machine row gains an optional parameter, `cycle_time_by_variant`
   (§6), without which shortest processing time cannot differ from FIFO.

This ADR builds on ADR-0022's recommended answers, which are themselves proposed.

## Context

ADR-0022 left M6 a flow with one edge per source, no loops, and first free as
the only branch rule. Its §3 worked through what the rules need: attributes on
offers, the taker's variant, a status message from target to upstream. Its
disagreement 9 asks which record kind a control decision uses, and its question 16
makes separate edges first-match. The expression language exists
(`taktsim_model::expr`, ADR-0009, ADR-0015); nothing uses it for edges yet.
`Edge` holds only `from` and `to`, and the document has no `control` section.

## The vocabulary

This extends spec §4's example, whose edges and `control` load as written once
`paint` has an edge leaving it (disagreement 6).

```yaml
components:
  - id: source
    type: Source
    interarrival: { dist: exponential, mean: 40s }
    variants: { basic: 80%, premium: 20% }
    due_in: { dist: uniform, min: 2h, max: 8h }   # from creation; absent: no due date
    priority_mix:                                  # instead of `priority`
      - { priority: 1, share: 20% }
      - { priority: 0, share: 80% }
  - id: weld-1
    type: Machine
    cycle_time: 45s
    cycle_time_by_variant: { premium: 60s }        # absent: `cycle_time` for all
flow:
  - { from: source, to: cut }
  - { from: cut, to: [weld-1, weld-2], choose: shortest_queue }
  - { from: [weld-1, weld-2], to: qc }
  - { from: qc, to: paint, when: "variant == 'premium'" }
  - { from: qc, split: { cut: 5%, pack: 95% } }
  - [paint, pack, sink]
control:
  dispatch: { weld-1: earliest_due_date, weld-2: earliest_due_date }
```

| Key | Values | Default |
| --- | --- | --- |
| `when` on an edge | a boolean expression over `variant` and `priority` | the edge always matches |
| `choose` on an edge | `first_free`, `shortest_queue`, `lowest_utilization`, `preferred` | `first_free` |
| `split` on an edge, instead of `to` and `choose` | a mapping from target to share | |
| `control.dispatch` | a mapping from station to `fifo`, `earliest_due_date`, `shortest_processing_time`, `minimum_setup`, `priority_class` | the upstream's own order |
| `due_in`, `priority_mix` on a Source | see §7 | |
| `cycle_time_by_variant` on a Machine | a mapping from variant to duration or distribution | |

Words follow the spec's `shortest_queue` and `earliest_due_date`, and the existing
lowercase words. `preferred` means preferred with fallback: the list order is the
preference. Each enum is matched exhaustively, with no wildcard arm.

## Decisions

### 1. Rules checked at load time

Each is an error with a field path and an example, reported through
`Catalog::with_check`, so `taktsim validate` sees it.

- `when`: parses, has type bool in the environment of §2, at most 4,096 bytes.
- Edges leaving one component are tried in the order written. The last has no
  `when`, so every part has a way out; an edge after one without `when` can never
  be used, so it is an error. M5's two unconditional edges stay rejected for that
  reason, and `second_edge_from_a_component_is_rejected` stands.
- `choose` needs at least two targets; `to` and `split` have at most 1,024.
- `split`: at least two targets, known ids, each share above 0 %, summing to 100 %
  within 10⁻⁹ percentage points, as `variants` does. `split` with `to` or `choose`
  is an error.
- `lowest_utilization`: every target is a station (`STATION`, ADR-0022 §2).
- `control.dispatch`: known ids, each a station. `control` is in the model file
  only, like `experiment`. `release` stays an unknown key until v0.2.
- Loops: §8.

ADR-0020 says an edge declared twice is an error, "relaxable in M6". It becomes:
a repeat is an error unless the earlier one has a `when`, since
`{from: qc, to: pack, when: …}, {from: qc, to: paint, when: …}, {from: qc, to: pack}`
is a sensible way to write first-match.

### 2. Where the vocabulary lives, and the hash

| | V1. Typed in `taktsim-model`, behaviour in the package | V2. Each catalog declares its edge keys | V3. Text in the model, checked by the domain |
| --- | --- | --- | --- |
| Rule names are typed enums (roadmap) | yes | yes, with a new generic layer | no |
| Schema generated from the types | yes, as now | yes, new machinery | enum lost from the schema |
| Model crate knows a component type (ADR-0020 §6) | no: rule words, not types | no | no |
| Later domain without routing | format 1.0's `flow` and `control` stay this engine's; ADR-0020 §11 already says so of `flow` | cleaner | clean |

**V1.** `ChoiceRule` and `DispatchRule` are public enums in `taktsim-model`; the
envelope test extends to `Control`, so `Model`, `Experiment` and the rest do not
reach it. `when` is parsed by the loader, which reports syntax errors. It is type-checked in
`Flow::check`, because the variant names come from the document:
`TypeEnv::new().with_enum("variant", declared)`, `with_path("variant", …)`,
`with_path("priority", Type::Int)`, no functions and no components. So conditions
are about the unit, never about observables.

`variant` is an enum and `priority` an int, so no condition can compare a float,
and the rules of ADR-0009 hold by construction. Spec §4 also names product and
quality state; nothing sets either before v0.2, and adding a path later breaks no
model. ADR-0022 §7 expected quality state in M6; see disagreement 4.

**Normalized form.** `when` becomes `Expr`'s canonical text, so spelling changes no
hash. `split` is a mapping, so its keys sort. A written `choose: first_free` stays
written (N1, ADR-0022 §1). Absent keys and an absent `control` are left out, so no
pinned hash moves.

**System hash.** A dispatching rule changes what a line produces, so a calibration
must not survive it: `SYSTEM_SECTIONS` gains `control`, left out when absent. That
changes ADR-0020 §8's list and no existing hash. The alternative, `dispatch` as a
Machine key, would keep the list but contradict spec §4's example.

### 3. From a part to a route

Two steps, in this order, so that each mechanism has one meaning:

1. **Which edge:** the first whose `when` holds. Attributes do not change, so this
   is decided once and never again.
2. **Which target on that edge:** one target, or `split`, or a `choose` rule.

Step 1 and a `split` happen when the part becomes ready to leave: created at a
Source, entered in a Buffer, finished at a Machine. Each holder keeps the result
beside the part. A `choose` rule acts until the part leaves (§5).

**When a part's target is full, it waits.** It never takes another edge or another
branch of a `split`.

**Can other parts pass it?**

| | H1. Head of line: only the next part in the holder's order has a route | H2. Each part has its route; a target takes the first part meant for it |
| --- | --- | --- |
| Paint full, a part for pack behind one for paint | pack starves behind paint | pack's part leaves |
| Fits "a Buffer chooses at `Accept`" (ADR-0022 §7) | changes it | yes |
| New deadlock | yes, a head waiting on a loop | no |
| Cost | O(1) | O(n) per `Accept` on a holder with routed parts |

**H2.** A Machine's finished units leave in finish order among those meant for the
target asking. This extends ADR-0022 §7 and changes nothing for one edge. A plain
edge (one target, no `when`, no rule) keeps M5's path, so chains cost what they did.

### 4. Split ratios

| | One edge, `split: {a: 5%, b: 95%}` | `share` on each edge | Parallel lists `to` and `shares` |
| --- | --- | --- | --- |
| Fits first-match of separate edges (ADR-0022 question 16) | yes | no: two meanings for two edges | yes |
| Combines with `when` | yes, on that edge | unclear | yes |
| Adds up, checked in one place, with one path | yes | across edges | yes |
| Same idiom as `variants` | yes | no | no |

**One edge.** The draw is `next_open01` on the component's `route` stream against
cumulative fractions over sorted target ids, the last taking what rounding leaves:
the rule for `variants`, with only exact float operations (ADR-0013). One draw per
part, at step 2 of §3, in the order parts become ready. A part whose branch is full
waits there, with an offer to that target only. No other stream shifts.

### 5. Pools and choice rules

A pool is an edge with several targets and a rule, as in spec §4's example. No pool
object exists; `group:` stays a viewer word.

| Rule | Protocol | Rank (lower wins), last term always position in the list |
| --- | --- | --- |
| `first_free` | M5's: offers to every target, the first `Accept` takes the part. When room opens at several at one instant, event order decides (ADR-0022 §3) | none |
| `preferred` | status | has room, then position |
| `shortest_queue` | status | has room, then queue, then position |
| `lowest_utilization` | status | has room, then work per place, then position |

**Why `preferred` differs from `first_free`.** When room opens at several targets at
one instant, `preferred` gives the part to the first listed, always; `first_free`
follows event order. A golden case pins each.

**Options for knowing about the targets** (components see each other only through
messages, ADR-0018):

| | Status on change (ADR-0022 §3) | Ask each target per part | Commit at readiness, never move |
| --- | --- | --- | --- |
| Pool without buffers, as in spec §4: a part behind busy `weld-1`, `weld-2` frees | moves | moves | waits behind `weld-1` while `weld-2` is empty |
| States while a question is open | none | new, with failures crossing | none |
| Messages | per change, only where a rule asks | 2 per target per part | per change |

**Status on change,** and a part may move while unaccepted. A `Status` goes from a
target to each upstream that uses a rule on it, or to each upstream if the target's
rule is `minimum_setup` (§6), at the end of any event that changed it:

- **`room`:** a place is free, the target is not down and its return lane is empty.
- **`queue`:** units at the target (waiting to start, in cycle, finished, returning,
  or in a Buffer's places), reserved places, and waiting offers. The asker takes off
  its own offer before comparing. A Sink is `room`, queue 0, for ever, and sends
  nothing.
- **`worked`, `running`, `setup_variant`:** below, and §6.

Upstream assumes room and queue 0 at the start, which is true then. A part with no
room anywhere stands as one offer, at the best-ranked target, so it keeps its turn
at a merge.

**A part moves only when a `Status` says a target gained room,** and only to a target
with room that outranks where it stands (`Withdraw`, then `Offer`). Changes of queue
alone update the ranks and move nothing. So an instant ends: every move follows a
room gained, a place can be gained only a finite number of times at one instant, and
the second part to reach a place stands in that queue without moving. An `Accept`
crossing a move gets the part, and the other offer is withdrawn (ADR-0022 question
18).

**Utilization without floats.** Each station adds `units_in_cycle × elapsed ns` to an
exact `worked` (u128), which `Status` carries with `running`, the units now in
cycle. Upstream works out `worked + running × (now − status time)`, divided by the
target's `capacity`, rounded down: nanoseconds worked per place since the run began.
Dividing again by elapsed time gives the utilization M8 reports, and elapsed time is
common to all targets, so this integer ranks them the same. Setup and down time do
not count as work. No warm-up reset; a pool does not know about warm-up.

**Costs.** A model without a status rule sends no `Status`, so its events do not
change.

### 6. Dispatching rules

Lower key wins; ties go to FIFO order. FIFO order is the order of offers at a
station's inlet, and entry order in a Buffer.

| Rule | Key of a candidate | Hand-built test |
| --- | --- | --- |
| `fifo` | its position in FIFO order | three units, oldest taken, also behind a `lifo` Buffer |
| `earliest_due_date` | due date in ns; no due date is `u64::MAX`, so after all others | due 3h, none, 1h: takes 1h; two with none: older |
| `shortest_processing_time` | the unit's mean time on this machine, in ns | two variants, shorter taken |
| `minimum_setup` | `2 × mean setup ns`, plus 1 if the variant changes | same variant first, then the cheapest change; a free `matrix` change after the same variant; first unit: FIFO |
| `priority_class` | `255 − priority` | classes 0, 2, 1: takes 2; ties older |

`minimum_setup` keeps the spec's "same variant first" and also uses `matrix` costs,
which the spec's parenthesis does not mention. A machine with no `setup` is FIFO.

**Mean times.** `Distribution` gains an exact mean: the declared `mean`, or `min + max`
over 2, or `min + mode + max` over 3, rounded down in integer ns, or for `weibull`
`scale × Γ(1 + 1/shape)` by `libm`, rounded down. All are the same on every platform
(ADR-0013). A drawn time does not exist before the cycle starts.

**Shortest processing time needs times per variant.** With one `cycle_time` per
machine the rule would equal FIFO (ADR-0022 §3). `cycle_time_by_variant` overrides it
per declared variant, drawn from the same `cycle_time` stream, so no draw shifts when
it is absent. A model that declares the rule without it loads and behaves as FIFO.

**Who decides.** A rule on station T acts in two places, so that every waiting unit
can win:

1. **At T's inlet,** when a place frees and two or more offers wait: T picks an
   offer by the key of the unit each describes.
2. **In a Buffer upstream,** when T's `Accept` arrives: the Buffer applies T's rule
   to the parts meant for T. T's rule replaces the Buffer's `discipline` while it
   is declared, as ADR-0022 recommended. Without a rule, the discipline holds.

A Machine upstream gives its first finished unit meant for T. A Source's batch is
one variant, priority and due date, so its order does not matter.

| | O1. Bare `Offer` stays; new `OfferOf(unit)` only to a station with a rule | O2. Every `Offer` carries attributes (ADR-0022 §3's wording) |
| --- | --- | --- |
| Bytes of a model with no rule | unchanged, so M5's traces prove M5's meaning | 21 more per offer |
| Enum tags | one more | none |

| | A1. `Accept` stays bare; a Buffer reads T's rule from the build and T's variant from `Status` | A2. `Accept` carries the rule and the variant (ADR-0022 §3's wording) |
| --- | --- | --- |
| Bytes of a model with no rule | unchanged | more, or a second tag |
| A Buffer can describe the unit best for T in its offer under `minimum_setup` | yes: it has T's variant before offering | no: the variant comes too late, so a matching part behind another variant loses to the other Buffer |

**O1 and A1.** A Buffer offering to a station with a rule describes the unit that
would win now, and sends `OfferOf` again when that changes: the entry keeps its
place in the queue with the new description. An on-demand Source with a rule
downstream, or with any route to work out, draws its next part's attributes ahead and
creates the item when accepted, so `OfferOf`'s item is optional.

`control.dispatch` in spec §4's example has a Machine upstream and no queue, so most
of the time there is nothing to choose and no record. That is what the model says.

### 7. Due dates and priorities before order lists

- `due_in`: a duration or distribution from creation, zero allowed, on stream `due_in`.
  `ItemAttrs.due`, carried since M5, gets `creation + draw`. A due time past the end
  of simulated time is a typed run failure.
- `priority_mix`: a list of `{priority, share}`, at least two entries, distinct
  priorities 0 to 255, shares above 0 % summing to 100 % as above. Sorted by priority
  in the normalized form and for the draw, on stream `priority`, by the rule of §4.
  With `priority` it is an error. A list because JSON mapping keys are text, and a
  priority is a number.
- Both once per batch (per part on demand): ADR-0022 §7's rule for variant and
  priority, so one batch stays one order.

Order lists (v0.2) add a key on a Source or a section beside it; a unit from an order
takes the order's due date and priority, and `due_in` keeps its meaning for a Source
without one.

### 8. Loops

ADR-0022 §3 left M6 to lift the rule "and state how such deadlocks are handled".
Finite places in a loop deadlock: `weld → qc → weld` with no buffer, weld holding a
finished part for qc, and qc holding one for weld. Any buffer only delays that, as
long as new parts keep coming in.

| | L1. Keep rejecting loops | L2. Allow; a deadlock is an outcome the run reports | L3. Reject unless the buffers are big enough | L4. A return never waits: a lane at the target, ahead of new parts |
| --- | --- | --- | --- | --- |
| Rework model (spec §4, M7) | impossible | runs until it locks up | no size is enough | runs |
| Deadlock | none | yes, and results up to then look fine | yes | none, by the argument below |
| Memory | | bounded | bounded | bounded, by a number known at load |
| Real plants | can't model | can lock up, too | | a rework rack, which real plants have |
| New machinery | none | outcome and detection | a capacity rule that does not work | one message, one queue |

**L4.** The load-time rule, over the parts of the graph where every component reaches
every other (strongly connected, two or more components):

1. An **exit edge** has all its targets outside the part, or is a `split` with a target
   outside. A **gate** is a component of the part whose leaving edges are all exit
   edges. `when` alone is no exit, since a part's attributes never change: a matching
   part would circle for ever.
2. Every cycle passes a gate.
3. All edges from gates back into the part go to one station T, the **return target**.
4. Every edge entering the part from outside enters at T.

`{from: qc, split: {weld: 5%, pack: 95%}}` satisfies it. So does adding
`{from: qc, to: paint, when: …}` before it. These reject, with the reason and that
example: `{from: qc, to: weld, when: …}, {from: qc, to: pack}`, and a first-free edge
back. A Buffer as return target is rejected with the fix: send it to the machine after
the buffer. A Buffer's fill must stay what its events say (ADR-0022 §9). Nested loops
with two return targets are rejected for now. Every rule here can be relaxed later
without changing a model that loads.

**At run time.** A part routed from a gate to T does not offer and wait. A `Return`
message carries it straight into T's **return lane**, a FIFO queue beside T's places.
T fills a free place from the lane first, before any offer. The lane is not a place,
so `capacity`'s invariant stands, and T's state is what its places imply. T holds lane
parts in `held_items`, so conservation counts them. A down T takes returns and moves
nothing. Taking from the lane is not a choice, so it writes no decision.

**No deadlock.** Over the graph of parts, which has no cycle: a waiting chain inside a
part cannot close, because every cycle passes a gate, and a gate's way back never
waits. So it ends at a station working, at a repair, at T's lane, or at an exit into a
later part. By induction from the end of the flow, it ends at a Sink. M5's argument
covers the rest (ADR-0022 §4).

**Bounded lane.** A new part enters only at T and only when the lane is empty, so it
always takes a place. So parts in the loop never exceed C, the sum of the `capacity`
of the loop's components, and the lane never exceeds C. `lane ≤ C` is T's runtime
invariant, with C computed at load.

**Price.** A rework part overtakes new parts, whatever T's rule says, and T's places
may leave new parts waiting while reworks run. A real rack does the same.

### 9. The decision record

| | K1. A note (`FlowMessage` variant, kind 4) | K2. New kind 5: `by u32`, message | K3. New kind 5 with neutral fields in core: rule id, item, chosen, candidates |
| --- | --- | --- | --- |
| Keeps ADR-0021's plan | no | yes | yes |
| Core change | none | one `Context` method, one `TraceRecord` variant (same shape as `note`) | a public decision type, frozen in the record format |
| Factory words stay out of core (principle 7) | yes | yes | yes, but rule ids need a table from elsewhere |
| A framing reader finds decisions without decoding messages | no | yes | yes |
| A reader can read them without the package | no | no | partly |
| A new rule or candidate field | new `FORMAT` | new `FORMAT` | new record format |

**K2,** which keeps what ADR-0021 chose. `Context::decide(&M)` writes
`TraceRecord::Decision { time, by, message }`, kind 5; `RECORD_FORMAT` becomes
`taktsim-core/3`, as ADR-0021 foresaw. Replay compares it like every other record.
As with a note, the engine never looks inside.

**What writes a record:**

| Decision | When | `by` |
| --- | --- | --- |
| `when`, for a component with a `when` on any leaving edge | when the part is ready | the holder |
| `split` | when the part is ready | the holder |
| `first_free`, `preferred`, `shortest_queue`, `lowest_utilization`, for every part leaving such an edge, including M5's default | when the part leaves | the holder |
| A declared `control.dispatch`, with two or more candidates | at T's inlet, or at the Buffer on T's `Accept` | T or the Buffer |

Nothing is written for a chain, one candidate, a Buffer's own `discipline`, or the
oldest offer at a merge with no rule. Those orders follow from records already in the
trace, and writing them would cost every FIFO queue O(fill) bytes per unit (question 2).

**Format** (`taktsim-components/2`), both fixed-width and both bounded:

| Message | Fields |
| --- | --- |
| `RouteDecision` | rule u8, item u64, edge u32, chosen u16, count u16, then per candidate: id u32, room u8 (0 no, 1 yes, 2 not known), key u64 |
| `DispatchDecision` | rule u8, taker u32, total u64, count u16, then per candidate, best first: from u32, has item u8, item u64, key u64 |
| `EdgeCondition` (note, at start) | edge u32, text |

Route rule codes: 0 `when`, 1 `split`, 2 `first_free`, 3 `shortest_queue`, 4
`lowest_utilization`, 5 `preferred`. Dispatch codes: 0 `fifo`, 1 `earliest_due_date`,
2 `shortest_processing_time`, 3 `minimum_setup`, 4 `priority_class`. `edge` counts the
holder's leaving edges in written order. A `when` candidate's id is such an edge and
its key 1 if it matched; the list ends at the first match, the chosen one. Otherwise
the id is the target's component number, in list order or sorted for `split`, and the
key is:

| Rule | Key |
| --- | --- |
| `split` | the share's `f64` bits |
| `first_free` | 0, room not known |
| `preferred` | position |
| `shortest_queue` | queue, without this part's offer |
| `lowest_utilization` | ns worked per place |

So **a record explains itself:** recomputing the rank from its own candidates gives its
own choice. A test does this for every record of every golden case. `EdgeCondition`
gives each `when`'s canonical text, so a trace reads on its own, as `VariantName` does
for variant ids. A dispatch's `key` is §6's, and M8 and `explain` read all of this
with one fold, as ADR-0022 §9 recommends.

**Long queues.** A record is at most 64 KiB, and an oversized one costs the whole trace
(ADR-0021).

| | C1. Several records per decision | C2. Best K in rule order, plus the total | C3. Only the winner and the total |
| --- | --- | --- | --- |
| Promise of spec §4 | fully, for any size | fully up to K; beyond that, every unlisted unit ranks after the last listed | no list; recoverable by folding the trace |
| Bytes per decision | O(queue), so O(queue²) per queue drained | at most about 21 KiB | constant |
| Answers "why this one, not that one" | yes | yes, and the rest follow | only by rebuilding the queue |

**C2 with K = 1,024.** Ranks are a total order, so the list is the same everywhere.
Route records need none of this: at most 1,024 targets, so at most about 13 KiB.
Recording is opt-in through declaring a rule, and it costs disk, never memory.

### 10. Messages

`FORMAT` becomes `taktsim-components/2`. Existing tags and their bytes do not change, and
neither does their order at one instant.

| Tag | Message | Priority | Fields |
| --- | --- | --- | --- |
| 13 | `Status` | 4, with `Arrive`: a fact lands before `Accept` | room u8, queue u64, running u32, worked (2 × u64), has variant u8, setup variant u16 |
| 14 | `OfferOf` | 7, with `Offer` | has item u8, item u64, variant u16, priority u8, has due u8, due u64 |
| 15 | `Return` + item | 4 | as `Arrive` |
| 16 | `RouteDecision` | 8, never delivered | §9 |
| 17 | `DispatchDecision` | 8 | §9 |
| 18 | `EdgeCondition` | 8 | §9 |

**Proof that M5 keeps its meaning:** a model with no rule, `when`, `split` or loop
writes the same event bytes as under ADR-0022, so `m5_chain`, `m5_capacity2`,
`m5_failures`, `m5_setup_mix` and `m5_same_instant_chain` keep their record lines and
stream hashes. Only their header lines change. `m5_branch_merge` gains `first_free`
records and no other line changes. The commit says so.

### 11. Streams

Added without shifting ADR-0022's table, and only when the key is declared:

| Component | Purpose | Drawn | Taken |
| --- | --- | --- | --- |
| any with an outlet | `route` | once per part reaching a `split` | with a `split` on a leaving edge |
| Source | `priority` | once per batch (per part on demand) | with `priority_mix` |
| Source | `due_in` | once per batch (per part on demand) | with `due_in` |

`when` and the `choose` rules draw nothing.

### 12. Runtime invariants

Added to ADR-0022 §10, on own state: lane ≤ C; a free place and a non-empty lane never
coexist unless down; a part with a route to a full target keeps an offer standing; a
Source's priority and due date are the same across a batch. M5's "a unit to give and no
outstanding offer" becomes per part: a part meant for a target implies an offer to it,
and a part on a status edge implies one offer on that edge. Conservation stays with the
engine at every event.

## One-way doors

Before v0.1 ships, each costs a code change and regenerated files. The cost below is
after v0.1 ships.

| Door | Commits to | Cost to reverse | Test that proves it |
| --- | --- | --- | --- |
| Flow and control words: `when`, `choose`, `split`, `control.dispatch`, rule names, `due_in`, `priority_mix`, `cycle_time_by_variant` | every routed model; later packages | a format migration | `ci/check-schemas.sh`, `schema_contract`, `routing_malformed` corpus, `spec_routing_example_loads` |
| First-match, the route decided once, full means wait, no fallthrough | every routed result | silently different results | `first_match_in_written_order`, `full_branch_waits_and_never_takes_another`, `no_edge_after_an_unconditional_one` |
| Meaning of each choice rule, its rank and ties | every pool result | every pool result | `each_choice_rule_breaks_ties_by_list_order`, `preferred_is_not_first_free_when_room_opens_together`, `part_moves_only_when_a_target_gains_room`, `utilization_rank_is_exact_integers` |
| Queue, room and utilization definitions | results; what users compare with M8's utilization | every pool result | `queue_counts_what_it_says`, `work_per_place_matches_trace_utilization` |
| Meaning of each dispatching rule, no due date last, taker's rule over `discipline` | every dispatched result | every dispatched result | `dispatching.rs` hand-built queues, `no_due_date_sorts_last_then_fifo`, `taker_rule_replaces_discipline` |
| Exact mean per distribution | SPT and minimum setup ranks | a new rule name | `means_are_pinned_per_distribution` |
| Split draw over sorted ids, once per part, at readiness | common random numbers | every split result | `split_converges_to_declared_shares`, `reordering_split_keys_changes_nothing`, `stream_purposes_are_pinned` |
| Loop rule, return lane, returns before offers | every rework model | every rework result | `rework_loop_neither_deadlocks_nor_loses_parts`, `lane_never_exceeds_c`, `loop_without_an_exit_is_rejected`, property test on generated loops |
| `Context::decide`, `TraceRecord::Decision`, kind 5, `taktsim-core/3` | public engine API, record format | a record format version, all traces | `decisions_reach_the_trace_and_no_component`, replay test with decisions, pinned hashes |
| Decision layout, rule codes, 1,024 limit, best first | M8, `explain`, every reader | a new `FORMAT` | `every_routing_choice_has_a_record`, `every_record_recomputes_its_own_choice`, `long_queue_lists_best_1024_and_total`, `messages_round_trip` |
| Message tags and priorities (`taktsim-components/2`) | trace contract | a new `FORMAT`, every golden trace | `priorities_are_pinned`, golden cases on every target, `m5_event_lines_are_unchanged` |
| `control` in the system hash; normalized `when`, `split`, `priority_mix`; written `first_free` stays | hashes and certificates of routed models | every hash of a routed model | `system_hash_changes_with_dispatch`, `spec_example_keeps_its_pinned_hashes`, `when_spelling_changes_no_hash`, `written_first_free_is_part_of_the_hash` |
| Public model API: `ChoiceRule`, `DispatchRule`, `Edge::{when, choose, split}`, `Document::control`, `Distribution::mean` | M7, M12, later packages | every client | doc tests, `a_loaded_model_always_builds` property test |
| No new external crate | bill of materials | none | `cargo deny check`, `Cargo.lock` diff |

## Spec and code disagreements

| # | What | Where | Recommendation | Blocks |
| --- | --- | --- | --- | --- |
| 1 | Spec §4 writes every control decision with its candidates; a queue above 1,024 lists 1,024 and undeclared orders write none | spec §4, §9 here | question 1, 2; add the two limits to the spec's sentence | no |
| 2 | `SYSTEM_SECTIONS` is exactly three sections, and a dispatching rule belongs to the system | ADR-0020 §8, `hash.rs`, §2 here | add `control`, absent left out | no |
| 3 | ADR-0022 §3's table has `Offer` carrying attributes and `Accept` the rule and variant | ADR-0022 §3, §6 here | O1 and A1: the table is a sketch for M6, and bare messages keep M5's bytes | no |
| 4 | ADR-0022 §7 lists quality state among M6's attributes and spec §4 names it, but nothing can set it before Inspection (v0.2) | ADR-0022 §7, §2 here | leave it to v0.2, additive | no |
| 5 | ADR-0022 §7 says finished units leave in finish order, and §4 that room is always promised before a part moves; routes and returns change both | ADR-0022 §4, §7, §3 and §8 here | amend ADR-0022's sentences with this ADR's acceptance; no M5 model is affected | no |
| 6 | Spec §4's example has no edge leaving `paint`, so as written it fails ADR-0022's connection rule; `pack` is also unstated | spec §4 | complete the example in the spec with `[paint, pack]` and its entries; M6's test uses the completed form | no |
| 7 | `first_free` in ADR-0022 §3 says "the spec's first free rule"; the same-instant case follows event order, not list order | ADR-0022 §3 | keep; `preferred` is the rule for list order | no |
| 8 | The spec's Machine row has no time per variant, while its SPT rule needs one | spec §4, §6 here | question 3 | no |

## Consequences

- Rework, pools and dispatching run without a change to `Component` or the engine beyond
  `Context::decide`.
- A model with no routing writes the same events and costs what it did: the plain edge
  keeps M5's path, and the instruction-count gate measures it.
- A routed trace grows by one record per routing choice, plus `Status` events where a rule
  asks. A declared rule on a long queue writes up to about 21 KiB per pick.
- `explain` can answer why a unit went where it did from the trace alone.
- Rules that reject models (loops, the last edge) can be relaxed later without changing any
  model that loads.

## Order of work

1. Model: `Edge` keys, `Control`, enums, `Distribution::mean`, hash, regenerated schema.
2. Core and trace: `Context::decide`, kind 5, reader, writer, replay, golden text.
3. Components: messages, then the route table and helpers in `port.rs` and a new
   `route.rs`, then Source keys, Machine's per-variant time.
4. Whole-document checks in `flow.rs`, replacing `check_one_edge_per_source` and
   `check_no_loops` with the rules of §1 and §8.
5. Behaviour: `when` and `split`, then `Status` and rules, then dispatching, then returns.
6. Decision records, golden cases, property tests, malformed corpus, concept notes.

## Questions for the owner

Implementation proceeds on the recommendations; each can still change before the M6 pull
request merges.

1. **Long queues (§9).** List the best 1,024 and the total above that, instead of the whole
   queue in several records? *Recommended:* yes. Several records keep the letter of the spec
   at a cost that grows with the square of the queue.
2. **Undeclared orders (§9).** No record for a Buffer's `discipline` or the oldest offer at a
   merge with no rule? They follow from records already in the trace, and a record would cost
   every FIFO queue its fill per unit. *Recommended:* yes. Anyone who wants them declares
   `fifo`.
3. **`cycle_time_by_variant` (§6).** Add it, so `shortest_processing_time` can differ from FIFO?
   Without it the rule has no effect in v0.1. *Recommended:* yes. The alternative is dropping
   the rule from v0.1, which the roadmap lists.
4. **`control` in the system hash (§2)?** A calibration then does not survive a change of rule.
   *Recommended:* yes.
5. **The return lane (§8).** Rework parts wait in a lane of bounded size at the target, ahead of
   new parts, and never block? It adds a place the model does not name, in exchange for no
   deadlock, ever. The alternative is to let models lock up and report it, with results up to
   then looking fine, which this milestone's no-deadlock tests rule out.
   *Recommended:* yes.
6. **The loop rule (§8).** One return target per loop, a Machine, new parts entering only there?
   It rejects nested rework for now. *Recommended:* yes; relaxing later breaks nothing.
7. **Status rules move a waiting part (§5).** A part behind a busy machine moves when another
   gains room, instead of keeping its first choice. A real worker would do the same, and spec
   §4's example has no buffers, so it would otherwise leave a machine idle beside a blocked one.
   *Recommended:* yes.
8. **Attributes in conditions (§2).** `variant` and `priority` only, as bare names, in v0.1?
   *Recommended:* yes.
9. **`minimum_setup` (§6).** Same variant first, then the cheapest change by `matrix`, where the
   spec's parenthesis says only "same variant first"? *Recommended:* yes.
