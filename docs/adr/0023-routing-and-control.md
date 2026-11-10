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
                                                   # needs `interarrival` (§7)
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
- At most 1,024 edges leave one component, so a `when` record stays far below
  ADR-0021's limit (§9). Without this, a model that loads could lose its trace.
- `choose` needs at least two targets; `to` and `split` have at most 1,024.
- `split`: at least two targets, known ids, each share above 0 %, summing to 100 %
  within 10⁻⁹ percentage points, as `variants` does. `split` with `to` or `choose`
  is an error.
- A rule on a type that does not state what the rule needs is an error (§13).
- `control.dispatch`: known ids. `control` is in the model file only, like
  `experiment`. `release` stays an unknown key until v0.2.
- `cycle_time_by_variant`: declared variants, and each entry follows the rule on
  `cycle_time`: not a constant zero (ADR-0022). Otherwise an on-demand Source could
  reach a Sink for ever at one instant, past the check that relies on a Machine
  advancing time.
- `due_in` on an on-demand Source is an error (§7).
- A key the catalog does not offer is an unknown key (§2).
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
| Later domain without routing | a catalog offers none of these words (below) | cleaner | clean |

**V1.** `ChoiceRule` and `DispatchRule` are public enums in `taktsim-model`; the
envelope test extends to `Control`, so `Model`, `Experiment` and the rest do not
reach it.

**A catalog says which words it honours.** The words live in one crate, but a domain
without routing must not load a rule it ignores, nor let `control` move a hash with no
change in results. `Catalog::with_routing(RoutingWords)` names four flags: `when`,
`choose`, `split`, `dispatch`. The default is none, so where a flag is absent the loader
reports an unknown key at its path and `model_schema` leaves the key out, as it does for
`Count` and `Distribution` (ADR-0022 question 17). `Domain::ROUTING` supplies the flags,
and `Flow` declares all four, so `schemas/model/1.0.json` offers them as the standard
catalog's. The flags say what a domain honours; §13 says what each type in it provides.

**How a custom rule will fit.** Spec §4 has typed enum values in v1 and "custom rules are
Rust components". No M6 code does this, and nothing here closes it. A later
`Catalog::with_dispatch_rule("critical_ratio", …)` would add a name a package supplies,
in the same place in the same file, so no model changes and no standard word changes
meaning. In Rust that adds an enum variant, which every exhaustive match must then
handle; that is what the repository's no-wildcard rule asks for, so the enums are not
`#[non_exhaustive]`. Rule codes 128 to 255 stay reserved for it (§9).

**Conditions.** `when` is parsed by the loader, which reports syntax errors. It is type-checked in
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
timed Source, about to be offered at an on-demand one, entered in a Buffer, finished
at a Machine. Each holder keeps the result beside the part, and a `split`'s draw for
the record (§9). A `choose` rule acts until the part leaves (§5).

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
  or in a Buffer's places), reserved places, and waiting offers, **other than an offer
  from the upstream receiving this `Status`.** So each receiver gets its own value, and
  neither an offer on its way nor a withdrawal on its way changes what it reads: nothing
  needs subtracting, and nothing can underflow. A Sink is `room`, queue 0, for ever, and
  sends nothing.
- **`worked`, `running`, `follows`:** below, and §6.

Upstream assumes room and queue 0 at the start, which is true then. A holder with a part
on a status edge and no room anywhere stands as one offer, at the best-ranked target, so
it keeps its turn at a merge. Further parts on that edge wait behind it, as in M5.

**When a standing offer moves.** A holder re-ranks at the end of an event that changed
the `room` it knows of any target on the edge, in either direction, and when it is
repaired. A down holder moves nothing (ADR-0022 §4), and catches up at repair. It moves
the offer (`Withdraw`, then `Offer`) when a target with room outranks the one where the
offer stands. Since room comes first in every rank, a part standing where there is no
room leaves for any target with room. Changes of queue or work alone update the ranks and
move nothing.

A gain of room alone would not be enough. A target that has had room all along never
reports a gain: two holders that offer `weld-1` at one instant on the same old `room`
would leave the second standing there beside an empty `weld-2`, and a holder down when a
gain arrives would miss it. So when an instant ends, for every holder that is up:

> A standing offer at a target with no room means no target on that edge has room.

This is a runtime invariant on the holder's own state (§12), and
`no_part_stands_beside_a_free_target` checks it against the trace.

An `Accept` crossing a move gets the part, and the other offer is withdrawn (ADR-0022
question 18).

**An instant still ends.** Every move follows a room change or a repair, a repair is
finite at one instant, and a re-rank moves at most one offer per edge. So it is enough
that room changes are finite:

1. Parts leaving holders at one instant are finite, by ADR-0022's load rules: time
   advances through a station on every path from an on-demand Source, no station's time
   is a constant zero, and §8 sends every loop through a station.
2. An `Accept` either finds its part, and that part leaves (crossing included), or gets
   a `Decline`. A `Decline` answers an offer sent before its part left or before its
   holder went down. Neither sends a new offer for that part. So between two departures
   or failures, the `Decline`s still to come are a known, finite number.
3. A gain of room comes from a departure, a repair, a lane emptying or a `Decline`, all
   finite by 1 and 2. A loss of room needs a reservation, which needs a free place, so
   losses never exceed the places free at the start plus the gains.

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

Lower key wins; ties go to FIFO order. **FIFO order is oldest ready first:** by `since`,
the time the unit became ready in its holder (entered a Buffer, finished at a Machine,
created at a timed Source, offered at an on-demand one), then by place in the station's
inlet. In a Buffer, `since` is entry order.

A Buffer's offer stands for the buffer, not for a unit, and its next offer joins the back
of the inlet's queue. So order of offers alone would take turns between two Buffers
whatever had waited longest, and every rule's tie-break would inherit that. `OfferOf`
therefore carries `since`. Without a declared rule, a merge keeps M5's oldest offer, as
ADR-0022 says; anyone who wants oldest unit declares `fifo` (question 10).

| Rule | Key of a candidate | Hand-built test |
| --- | --- | --- |
| `fifo` | `since` in ns | three units, oldest taken, also behind a `lifo` Buffer; `fifo_is_oldest_ready_across_two_buffers` |
| `earliest_due_date` | due date in ns; no due date is `u64::MAX`, so after all others | due 3h, none, 1h: takes 1h; two with none: older |
| `shortest_processing_time` | the unit's mean time on this machine, in ns | two variants, shorter taken |
| `minimum_setup` | `2 × mean setup ns`, plus 1 if the variant changes | same variant first, then the cheapest change; a free `matrix` change after the same variant; first unit: FIFO |
| `priority_class` | `255 − priority` | classes 0, 2, 1: takes 2; ties older |

`minimum_setup` keeps the spec's "same variant first" and also uses `matrix` costs,
which the spec's parenthesis does not mention. A machine with no `setup` is FIFO.

**What a change is measured against.** Units in a machine start in arrival order, so the
next accepted unit follows the last one committed, not what the machine is set up for
now. `follows` is the variant of the newest reserved place as its offer described it,
otherwise the back of `waiting`, otherwise the changeover's target, otherwise `ready_for`,
otherwise none (first unit). A station uses it, and `Status` carries it, so a Buffer
upstream ranks the same way. With a place free, a changeover to B running and offers of A
and B, the B unit wins, sparing a change to A and back
(`minimum_setup_follows_the_last_committed_unit`).

**Mean times.** `Distribution` gains `mean`, the mean of what the sampler draws. It is the
declared `mean` for `exponential`, `lognormal`, `gamma` and `erlang`, `min + max` over 2,
or `min + mode + max` over 3, rounded down in integer ns. For `weibull` it is
`scale × Γ(1 + 1/shape)` by `libm`, rounded down.

For `normal` it is not the declared `mean`: the sampler rejects negative draws (ADR-0013),
so the drawn mean is `mean + sd × φ(z) / Φ(z)` with `z = mean / sd`, by `libm`, with
`Φ(z) = erfc(−z / √2) / 2`, rounded down, and the declared `mean` if `sd` is 0. Since `mean`
is never negative, `z ≥ 0` and `Φ(z) ≥ 0.5`, so the division is stable. With `mean: 10s, sd:
20s` this is about 20.18 s, so a unit of that variant ranks after one of 15 s, as it
should. Every result is the same on every platform (ADR-0013). A drawn time does not exist
before the cycle starts. Each of these ignores only the rounding of each draw to a
nanosecond.

**Shortest processing time needs times per variant.** With one `cycle_time` per
machine the rule would equal FIFO (ADR-0022 §3). `cycle_time_by_variant` overrides it
per declared variant, drawn from the same `cycle_time` stream, so no draw shifts when
it is absent. A model that declares the rule without it loads and behaves as FIFO.

**Who decides.** A rule on station T acts in two places, so that every waiting unit
can win:

1. **At T's inlet,** when a place is free and two or more offers wait: T picks an
   offer by the key of the unit each describes. An `Accept` is not yet a commitment,
   since upstream may answer `Decline`, so the record waits for the `Arrive` (§9).
   Units offered at one instant all wait at that instant, so T picks only once every
   offer of the instant is in: a free place beside a waiting offer schedules a `Pick`,
   a timer after no time at priority 8, after every `Offer`, `OfferOf` and `Withdraw`
   (§10). Otherwise the first offer delivered would take a free place unranked, and
   which one that is follows the order components are listed in
   (`dispatch_result_does_not_depend_on_listing_order`, disagreement 19).
2. **In a Buffer upstream,** when T's `Accept` arrives: the Buffer applies T's rule
   to the parts meant for T. T's rule replaces the Buffer's `discipline` while it
   is declared, as ADR-0022 recommended. Without a rule, the discipline holds.

A Machine upstream gives its first finished unit meant for T. A Source's batch is
one variant, priority and due date, so its order does not matter.

A station without a rule takes an offer when it comes. So when a `first_free` holder
offers to it and to T at one instant with room at both, it has taken the part and the
holder has withdrawn it from T before T picks
(`a_station_with_a_rule_picks_after_the_offers_of_the_instant`). Event order still
decides that race, as ADR-0022 §3 says for `first_free`; it is only fixed.

| | O1. Bare `Offer` stays; new `OfferOf(unit)` only to a station with a rule | O2. Every `Offer` carries attributes (ADR-0022 §3's wording) |
| --- | --- | --- |
| Bytes of a model with no rule | unchanged, so M5's traces prove M5's meaning | 21 more per offer |
| Enum tags | one more | none |

| | A1. `Accept` stays bare; a Buffer reads T's rule from the build and T's variant from `Status` | A2. `Accept` carries the rule and the variant (ADR-0022 §3's wording) |
| --- | --- | --- |
| Bytes of a model with no rule | unchanged | more, or a second tag |
| A Buffer can describe the unit best for T in its offer under `minimum_setup` | yes: it has T's `follows` before offering | no: the variant comes too late, so a matching part behind another variant loses to the other Buffer |

**O1 and A1.** A Buffer offering to a station with a rule describes the unit that
would win now, and sends `OfferOf` again when that changes: the entry keeps its
place in the queue with the new description. An on-demand Source with a rule
downstream, or with any route to work out, draws its next part's variant and priority
ahead and creates the item when accepted, so `OfferOf`'s item is optional. Its `since` is
the time it offers, and it has no due date (§7).

`control.dispatch` in spec §4's example has a Machine upstream and no queue, so most
of the time there is nothing to choose and no record. That is what the model says.

### 7. Due dates and priorities before order lists

- `due_in`: a duration or distribution from creation, zero allowed, on stream `due_in`.
  `ItemAttrs.due`, carried since M5, gets `creation + draw`. A due time past the end
  of the clock, a draw too long for a duration or a creation plus draw that passes the
  limit, is no due date: the unit sorts last under `earliest_due_date`, as one without a
  date, and the run goes on. This is ADR-0022 §6's rule for every other draw, so whether
  a run fails never depends on the seed (principle 2). The draw still takes its place in
  the stream. A date exactly at the limit is still a date.
- `due_in` needs `interarrival`. An on-demand part is created when accepted, so a rule
  would rank it by a date it does not yet have, and its later due date would differ from
  the one ranked. Counting from the offer instead would measure time spent blocked, not an
  order's age. So it is rejected with the fix; relaxing it later breaks no model.
- `priority_mix`: a list of `{priority, share}`, at least two entries, distinct
  priorities 0 to 255, shares above 0 % summing to 100 % as above. Sorted by priority
  in the normalized form and for the draw, on stream `priority`, by the rule of §4.
  With `priority` it is an error. A list because JSON mapping keys are text, and a
  priority is a number.
- Both once per batch (`priority_mix` per part on demand): ADR-0022 §7's rule for
  variant and priority, so one batch stays one order.

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
| `when`, for a component with a `when` on any leaving edge | when the part leaves | the holder |
| `split` | when the part leaves | the holder |
| `first_free`, `preferred`, `shortest_queue`, `lowest_utilization`, for every part leaving such an edge, including M5's default | when the part leaves | the holder |
| A declared `control.dispatch`, with two or more candidates, at T's inlet | when the `Arrive` confirms the pick | T |
| The same, on T's `Accept` at a Buffer | at once: the part is leaving | the Buffer |

**Route records wait for the part to leave.** Only then does every part have an item: an
on-demand part is created at `Accept`, so a record at readiness would have no item to name,
and a flag saying so could never be linked to the part afterwards. One writing point also
gives one rule: a record exists exactly when its choice had an effect. A part with both a
`when` and a `split` writes both, in that order, the moment it leaves. The holder keeps
the decision it made earlier, and the record states it.

**Dispatch records wait for the `Arrive`.** A pick at T's inlet may meet a `Decline`,
because the holder is down, or the unit it described has gone (disagreement 14). A record
written with the `Accept` would then state a choice that never happened. So T keeps the
candidates beside the reservation for that instant. An `Arrive` writes them, with the
chosen entry's item and key replaced by those of the unit that came (a Buffer's `Accept`
may give a better one, under the same rule). A `Decline` discards them. The next pick after
a `Decline` at that instant writes a record even with one candidate, because a pick that
failed came first and the trace should show what replaced it
(`declined_pick_writes_no_record_and_the_re_pick_writes_one`).

Nothing is written for a chain, one candidate, a Buffer's own `discipline`, or the
oldest offer at a merge with no rule. Those orders follow from records already in the
trace, and writing them would cost every FIFO queue O(fill) bytes per unit (question 2).

**Format** (`taktsim-components/2`), both fixed-width and both bounded:

| Message | Fields |
| --- | --- |
| `RouteDecision` | rule u8, item u64, edge u32, chosen u16, crossed u8, detail u64, count u16, then per candidate: id u32, room u8 (0 no, 1 yes, 2 not known), key u64 |
| `DispatchDecision` | rule u8, taker u32, total u64, count u16, then per candidate, best first: from u32, has item u8, item u64, key u64, since u64 |
| `EdgeCondition` (note, at start) | edge u32, text |

Route rule codes: 0 `when`, 1 `split`, 2 `first_free`, 3 `shortest_queue`, 4
`lowest_utilization`, 5 `preferred`. Dispatch codes: 0 `fifo`, 1 `earliest_due_date`,
2 `shortest_processing_time`, 3 `minimum_setup`, 4 `priority_class`. In both, 128 to 255 are
reserved for package rules (§2). `edge` counts the holder's leaving edges in written order. A `when` candidate's id is such an edge and
its key 1 if it matched; the list ends at the first match, the chosen one. Otherwise
the id is the target's component number, in list order or sorted for `split`, and the
key is:

| Rule | Key |
| --- | --- |
| `split` | the share's `f64` bits |
| `first_free` | 0, room not known |
| `preferred` | position |
| `shortest_queue` | queue, as `Status` gave it to this holder (§5) |
| `lowest_utilization` | ns worked per place |

`detail` is what a rule needs beyond its candidates: for `split` the draw's `f64` bits, for
a status rule the time of the ranking below, otherwise 0.

**A status record shows the ranking that placed the offer.** It cannot show the ranks when
the part leaves. By then the chosen target's own `Status` (priority 4) has landed ahead of
its `Accept` (priority 5) and already counts this part: room 0, queue one higher. The ranks
would then favour a target the part did not choose, and undoing the reservation from the
record is impossible, since the room flag does not say why it changed. So a holder keeps a
copy of the candidates each time it places or moves a standing offer, with the time in
`detail`. A re-rank that moves nothing leaves the copy alone: the record says why the offer
stands where it does. One copy per status edge, at most 1,024 candidates.

`crossed` is 1 when an `Accept` from the earlier target crossed a move (§5), so the part went
to `chosen` although the ranking favoured the target it had just moved to. That happens only
at the instant of a move, and a test checks so.

**So a record explains itself:** recomputing from its own fields gives its own choice.

| Rule | Recomputed from |
| --- | --- |
| `when` | the list ends at the first match, and it is `chosen` |
| `split` | `detail`, against cumulative shares in list order: §4's rule |
| `preferred`, `shortest_queue`, `lowest_utilization` | the rank of §5 over the candidates, unless `crossed` is 1 |
| `first_free` | **nothing.** Event order chose: every key is 0 and room unknown, so the record states which target took the part, and no more |
| a dispatch | rank `(key, since, position in the list)`; the list is in that order, and the chosen unit is first |

`every_record_recomputes_its_own_choice` does this for every record of every golden case and
exempts only `first_free`. `EdgeCondition` gives each `when`'s canonical text, so a trace reads
on its own, as `VariantName` does for variant ids. A dispatch's `key` is §6's, and M8 and
`explain` read all of this with one fold, as ADR-0022 §9 recommends.

**Long queues.** A record is at most 64 KiB, and an oversized one costs the whole trace
(ADR-0021).

| | C1. Several records per decision | C2. Best K in rule order, plus the total | C3. Only the winner and the total |
| --- | --- | --- | --- |
| Promise of spec §4 | fully, for any size | fully up to K; beyond that, every unlisted unit ranks after the last listed | no list; recoverable by folding the trace |
| Bytes per decision | O(queue), so O(queue²) per queue drained | at most about 30 KiB | constant |
| Answers "why this one, not that one" | yes | yes, and the rest follow | only by rebuilding the queue |

**C2 with K = 1,024.** Ranks are a total order, so the list is the same everywhere.
Route records need none of this: at most 1,024 targets and 1,024 leaving edges (§1), so at
most about 13 KiB.
Recording is opt-in through declaring a rule, and it costs disk, never memory.

### 10. Messages

`FORMAT` becomes `taktsim-components/2`. Existing tags and their bytes do not change, and
neither does their order at one instant.

| Tag | Message | Priority | Fields |
| --- | --- | --- | --- |
| 13 | `Status` | 4, with `Arrive`: a fact lands before `Accept` | room u8 (bit 0: room, bit 1: the receiver's own `Accept` follows; disagreement 13), queue u64, running u32, worked (2 × u64), has follows u8, follows u16 |
| 14 | `OfferOf` | 7, with `Offer` | has item u8, item u64, variant u16, priority u8, has due u8, due u64, since u64 |
| 15 | `Return` + item | 4 | as `Arrive` |
| 16 | `RouteDecision` | 8, never delivered | §9 |
| 17 | `DispatchDecision` | 8 | §9 |
| 18 | `EdgeCondition` | 8 | §9 |
| 19 | `Pick` (timer, after no time) | 8, the only message at 8 that is delivered: after every offer of the instant (§6) | none |

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
| Source | `due_in` | once per batch | with `due_in` |

`when` and the `choose` rules draw nothing.

### 12. Runtime invariants

Added to ADR-0022 §10, on own state: lane ≤ C; a free place and a non-empty lane never
coexist unless down; a part with a route to a full target keeps an offer standing; a
Source's priority and due date are the same across a batch. M5's "a unit to give and no
outstanding offer" becomes per part: a part meant for a target implies an offer to it,
and a part on a status edge implies one offer on that edge. An up holder with an offer at a
target it knows has no room knows of no target on that edge with room (§5). Conservation stays
with the engine at every event.

### 13. What a type states for a rule

`ComponentType` says only `PORTS` and `STATION`, but mean time, setup cost, places, `worked` and
`Status` would all come from Machine's parameters and behaviour. Then every later station
(Inspection with `inspection_time`, a Conveyor whose capacity comes from length and spacing)
would mean editing `flow.rs`, or would silently get a default: a rule acting as FIFO, a target
that upstream believes empty for ever. Principle 2 says such a model does not load.

So `Domain` gains `type Facts: Default`, and `ComponentType<D>` gains `fn facts(&self) -> D::Facts`
with the default as its default. `Roles` gives them to `check` and `prepare`, so the route table
and `TakerPlan` are built from facts, never from a Machine's parameters, and the generic
`run.rs` stays free of factory words. Flow's are public, `StationFacts`:

| Fact | Default | A rule needing it |
| --- | --- | --- |
| `routes`: uses the route table for parts leaving | no | leaving `when`, `split`, or any `choose` but `first_free` |
| `status`: none, always room (Sink), or reports | none | a target of `preferred`, `shortest_queue` |
| `work`: keeps `worked` and `running` | no | a target of `lowest_utilization` |
| `places`: how many, however derived | none | `lowest_utilization`; every component of a loop (C, §8) |
| `takes_rule`: picks among `OfferOf` | no | any `control.dispatch` |
| `mean_time` by variant | none | `shortest_processing_time` |
| `setup` means by variant pair; empty is a stated "no setups" | none | `minimum_setup` |
| `takes_returns`: has a return lane | no | a return target (§8) |

A rule on a type lacking a fact is an error at load, naming the type, the fact and the rule.
`first_free` and a plain edge need nothing, so `a_registered_stand_in_loads_and_builds` stands
and the stand-in simply cannot be given a rule. Machine states all but `status: always room`;
Buffer states `routes`, `status`, `places`; Sink states `status: always room`; Source states
`routes`. The shared parts live in `Inlet` and `Router`, so a new station gets the behaviour by
using them and stating the facts.

## One-way doors

Before v0.1 ships, each costs a code change and regenerated files. The cost below is
after v0.1 ships.

| Door | Commits to | Cost to reverse | Test that proves it |
| --- | --- | --- | --- |
| Flow and control words: `when`, `choose`, `split`, `control.dispatch`, rule names, `due_in`, `priority_mix`, `cycle_time_by_variant`; 1,024 leaving edges; a package's rule is added beside them, never in their place | every routed model; later packages | a format migration | `ci/check-schemas.sh`, `schema_contract`, `routing_malformed` corpus, `spec_routing_example_loads` |
| First-match, the route decided once, full means wait, no fallthrough | every routed result | silently different results | `first_match_in_written_order`, `full_branch_waits_and_never_takes_another`, `no_edge_after_an_unconditional_one` |
| Meaning of each choice rule, its rank and ties; re-rank on any room change or repair | every pool result | every pool result | `each_choice_rule_breaks_ties_by_list_order`, `preferred_is_not_first_free_when_room_opens_together`, `no_part_stands_beside_a_free_target`, `offer_moves_when_its_target_loses_room`, `repaired_holder_re_ranks`, `queue_changes_alone_move_nothing`, `utilization_rank_is_exact_integers` |
| Queue, room and utilization definitions; a `Status`'s queue leaves out the receiver's own offer | results; what users compare with M8's utilization | every pool result | `queue_counts_what_it_says`, `work_per_place_matches_trace_utilization` |
| Meaning of each dispatching rule, FIFO as oldest ready, no due date last, taker's rule over `discipline`; what `minimum_setup` measures against; a rule picks among every offer of the instant, at its `Pick` | every dispatched result | every dispatched result | `dispatching.rs` hand-built queues, `fifo_is_oldest_ready_across_two_buffers`, `no_due_date_sorts_last_then_fifo`, `taker_rule_replaces_discipline`, `minimum_setup_follows_the_last_committed_unit`, `earliest_due_date_takes_the_earliest_and_no_date_last`, `dispatch_result_does_not_depend_on_listing_order`, `a_station_with_a_rule_picks_after_the_offers_of_the_instant` |
| `mean` is what the sampler draws, per distribution, truncated normal included | SPT and minimum setup ranks | a new rule name | `means_are_pinned_per_distribution`, `truncated_normal_mean_matches_its_draws` |
| A due date past the end of the clock is no due date, never a run failure (§7, ADR-0022 §6) | every model with due dates; results of `earliest_due_date` with dates beyond 584 years | silently different results for those models; failing them instead breaks them | `a_due_date_past_the_end_of_time_is_no_due_date`, `a_due_date_drawn_past_the_end_of_time_is_no_due_date_and_keeps_its_draw`, `a_due_date_past_the_clock_sorts_with_no_due_date` |
| `due_in` needs `interarrival` | every model with due dates | none: relaxing it breaks no model | `routing_malformed` corpus, `due_in_on_an_on_demand_source_is_rejected` |
| `StationFacts`, `Domain::Facts`, `ComponentType::facts`, and which rule needs which fact | every later station and package | every package | `a_rule_on_a_type_without_the_facts_is_rejected`, `a_registered_stand_in_loads_and_builds` |
| `Catalog::with_routing`, `Domain::ROUTING`: a catalog offers only the words it honours | later domains, their schemas | every later domain | `routing_words_need_a_catalog_that_offers_them`, `a_catalog_without_routing_does_not_offer_it` |
| Split draw over sorted ids, once per part, at readiness | common random numbers | every split result | `split_converges_to_declared_shares`, `reordering_split_keys_changes_nothing`, `stream_purposes_are_pinned` |
| Loop rule, return lane, returns before offers | every rework model | every rework result | `rework_loop_neither_deadlocks_nor_loses_parts`, `lane_never_exceeds_c`, `loop_without_an_exit_is_rejected`, property test on generated loops |
| `Context::decide`, `TraceRecord::Decision`, kind 5, `taktsim-core/3` | public engine API, record format | a record format version, all traces | `decisions_reach_the_trace_and_no_component`, replay test with decisions, pinned hashes |
| Decision layout, rule codes with 128 and up reserved, 1,024 limits, best first; route records when the part leaves, from the ranking that placed it, dispatch records at `Arrive`; `first_free` states and does not explain | M8, `explain`, every reader | a new `FORMAT` | `every_routing_choice_has_a_record`, `every_record_recomputes_its_own_choice`, `status_record_shows_the_ranking_that_placed_the_offer`, `crossed_only_at_the_instant_of_a_move`, `declined_pick_writes_no_record_and_the_re_pick_writes_one`, `on_demand_route_record_names_its_item`, `long_queue_lists_best_1024_and_total`, `messages_round_trip` |
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
| 9 | ADR-0022 §2 gives a type `PORTS` and `STATION` only; rules need more | ADR-0022 §2, `run.rs`, §13 here | add facts with this ADR's acceptance; no M5 model or stand-in changes | no |
| 10 | Spec §4 says custom rules are Rust components, while rule names are closed enums in the model crate | spec §4, §2 here | keep the enums for v1, as the spec's same sentence says, and reserve the additive path of §2 | no |
| 11 | With no rule a merge takes the oldest offer (ADR-0022 §4), which is not the oldest unit, while a declared `fifo` is the oldest ready unit, so absent and `fifo` differ | ADR-0022 §4, §6 here | question 10 | no |
| 12 | §10 keeps `m5_same_instant_chain`'s record lines, and §9 writes a record for every part leaving an edge with several targets "including M5's default". That model has one (`{from: source, to: [a, b]}`), so it gains `first_free` records, as `m5_branch_merge` does | §9, §10 here | §9 stands: both gain records and no event moves, which `m5_event_lines_are_unchanged` pins with hashes taken from M5's code | no |
| 13 | §5 has a target's `Status` land ahead of its `Accept`, counting the part, and a holder re-rank on any change of a room flag. Taken together, every offer accepted beside another free target would move, while its own `Accept` was on the way: the part leaves crossed, the record shows the ranking that favoured the wrong target, and the other target gets an `Offer`, an `Accept` and a `Decline` for nothing. That contradicts §9's "shows the ranking that placed the offer" and its "only at the instant of a move" | §5, §9 here | a `Status` says so when the receiver's own `Accept` follows it (the `room` byte's second bit, `TargetStatus::accept_follows`), and nothing moves while that `Accept` is due. The ranks and §5's invariant are otherwise as written; the invariant exempts that one event | no |
| 14 | §9 corrects the chosen entry "to the unit that came", on the grounds that a Buffer's `Accept` may give a better one. It may also give a worse one: the unit described left for another target at that instant, a new description is on its way, and the `Accept` answers the old one. The record would then not recompute | §6, §9 here | a holder answers `Decline` when the unit an `Accept` answers has gone. An `Accept` carries no number, but which description it answers is known: the station picks at its `Pick`, when every description sent before has reached it, and one sent after can only follow the station's own `Status` saying its `Accept` follows. So the `Accept` answers the description current when that `Status` came, or the current one if none came; taking it to answer the oldest description of the instant declined picks of units still there. Each is answered once, so an instant ends, and §5's argument covers it: a `Decline` answers an offer sent before its part left | no |
| 15 | §13's table lists `takes_rule` as what a dispatching rule needs, but a station ranks what its upstreams describe, and a type that does not state `routes` sends a bare `Offer` | §13 here | a station with a rule also needs upstreams that state `routes`, an error at load naming type, fact and rule. Relaxing it later breaks no model | no |
| 16 | §10's table lists `Status`, `OfferOf` and the two decisions as messages with their fields; in Rust they are boxed, and `FlowMessage` keeps its size, so a model without rules moves no larger message (`a_model_without_rules_moves_no_larger_message_than_it_did`). The bytes in a trace are as the table says | §10 here | keep; no reader sees it | no |
| 17 | §6 has a Buffer rank by `follows` when `Accept` arrives, and `follows` is the newest reserved place. With two places free in one event, the first `Accept` lands after a `Status` already naming the second pick, so the Buffer gives a unit that ranks worse than what the station passed over | §6, §9 here | `minimum_setup` takes one place at a time, the next when this one's reply has landed at this instant. No other rule reads `follows`, and throughput does not change, since a reservation ends within its instant. The invariant on free places and waiting offers makes that one exception | no |
| 18 | §7 made a due date past the end of the clock a typed run failure, while ADR-0022 §6 (question 19 there) makes every other draw past the clock mean never, so that whether a run fails does not depend on the seed. With a `due_in` distribution, §7 as written fails some runs by seed | §7 here, ADR-0022 §6 | such a date is no due date: it sorts last like a unit without one, and the run goes on (question 16) | no |
| 19 | §6 step 1 said a station picks "when a place frees and two or more offers wait", and the implementation read it per event: offers made at one instant reach a free station one by one, so the first delivered took the place unranked and wrote no record. Which one that is depends on the order components are listed in, so a declared rule had no effect on units offered together, against spec §4 ("decide which waiting unit a resource takes next") and the roadmap's record for every choice | §6, §9, §10 here | a station with a rule picks at a `Pick`, a timer after no time at priority 8, after every offer of the instant; tag 19 in `taktsim-components/2`, which is not yet released. Models with a rule gain one event per pick; models without one are unchanged. The invariant on free places and waiting offers exempts the events before the `Pick` of that instant. The alternative, stating that simultaneous offers follow event order, leaves results depending on listing order | no |

## Consequences

- Rework, pools and dispatching run without a change to `Component` or the engine beyond
  `Context::decide`.
- A model with no routing writes the same events and costs what it did: the plain edge
  keeps M5's path, and the instruction-count gate measures it.
- A routed trace grows by one record per routing choice, plus `Status` events where a rule
  asks. A declared rule on a long queue writes up to about 30 KiB per pick.
- `explain` can answer why a unit went where it did from the trace alone.
- Rules that reject models (loops, the last edge) can be relaxed later without changing any
  model that loads.

## Order of work

1. Model: `Edge` keys, `Control`, enums, `RoutingWords`, `Distribution::mean`, hash, regenerated
   schema.
2. Core and trace: `Context::decide`, kind 5, reader, writer, replay, golden text.
3. Components: messages, `Domain::Facts` and `StationFacts`, then the route table and helpers in
   `port.rs` and a new `route.rs`, then Source keys, Machine's per-variant time.
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
   machine has room, instead of keeping its first choice, including when its target fills or
   fails and when its own holder is repaired. A real worker would do the same, and spec §4's
   example has no buffers, so it would otherwise leave a machine idle beside a blocked one.
   *Recommended:* yes.
8. **Attributes in conditions (§2).** `variant` and `priority` only, as bare names, in v0.1?
   *Recommended:* yes.
9. **`minimum_setup` (§6).** Same variant first, then the cheapest change by `matrix`, where the
   spec's parenthesis says only "same variant first"? *Recommended:* yes.
10. **What `fifo` means (§6).** Oldest ready unit across all upstreams, so a declared `fifo` is not
    the same as declaring nothing, which keeps M5's oldest offer? With only offers, two Buffers
    take turns, whatever has waited longest. *Recommended:* yes.
11. **Due dates on an on-demand Source (§7).** Rejected at load, since the part does not exist until
    accepted? *Recommended:* yes; relaxing later breaks nothing.
12. **What moves a standing offer (disagreement 13).** Implementing §5 as written moved an offer
    every time its own target accepted it. A `Status` now tells the receiver when its own `Accept`
    follows, one bit in the `room` byte, and nothing moves while it is due. The alternative that adds
    nothing to the format is to leave a receiver's own reservation out of what it reads, as its own
    offer is, but then the next part on that edge is offered on stale room. *Recommended:* the bit.
13. **A station told of a unit that has gone (disagreement 14).** The holder answers `Decline`, and
    the station picks again, so a record always recomputes. The alternative keeps the part moving
    and lets a record list a chosen unit that ranks after another. *Recommended:* `Decline`.
14. **One place at a time under `minimum_setup` (disagreement 17).** A station with this rule takes
    one place at a time at an instant, so that what its upstream ranks by is the unit each `Accept`
    follows? *Recommended:* yes; it costs one more event per place taken at one instant.
15. **A rule needs upstreams that describe (disagreement 15).** A station with a rule rejects a
    feeder whose type does not state `routes`, at load? *Recommended:* yes; relaxing later breaks
    nothing.
16. **A due date past the end of the clock (§7, disagreement 18).** It is no due date, so it sorts
    last under `earliest_due_date` and the run goes on, as ADR-0022 §6 does for every other draw?
    The alternative fails the run, depending on the seed for a distribution. *Recommended:* yes.
    Implementation proceeds on this.
17. **Offers of one instant (§6, disagreement 19).** A station with a rule picks only once every
    offer of the instant is in, through a `Pick` timer, so that units offered together are ranked
    by the rule and not by the order components are listed in? *Recommended:* yes; it costs one
    event per pick in models with a rule, and a station with a rule then loses a same-instant
    `first_free` race to one without, which event order decided anyway.
