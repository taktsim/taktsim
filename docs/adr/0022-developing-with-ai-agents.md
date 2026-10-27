# ADR-0022: Developing with AI agents: pace follows understanding

- Status: proposed (needs the owner's explicit acceptance: it changes how
  the project works and CLAUDE.md)
- Date: 2026-10-27
- Blocks: every milestone from M5 on; the open PRs #21, #23 and #24
- Spec: §2 principles 1 and 4; §10 (process); ADR-0007 (gates)

## Context

On 2026-10-26 and 27, five agent tracks ran in parallel (expression
follow-ups, the accepted-regressions gate, the queue benchmark, M3 trace
and replay, M4 model loader). In about a day they produced roughly 25,000
lines in seven pull requests. Each was checked: by CI, by tests derived
from theory, by a review from the lead session. The owner mostly answered
batches of questions with "accept the recommendations".

The work was sound, but the owner's understanding fell behind it. The
owner said so: "I have the feeling that I am developing this tool too
fast without properly understanding."

With agents, writing code is no longer the bottleneck. The owner's
attention is: to understand the system, to decide what should be true, and
to judge whether it is. A project whose owner does not understand it has
an operator, not an owner, and its one-way decisions (formats, hashes,
public interfaces) are then made by whoever wrote the recommendation.

## Options

- **A. Keep the pace.** Maximum throughput; understanding stays with the
  agents and their transcripts.
- **B. Pace follows the owner's understanding** (below). Less throughput;
  the owner can explain and defend every one-way decision.
- **C. Stop using parallel agents.** Simplest, but discards the help
  where it is safe: small, independent, machine-checked work.

## Decision

**B**, with five rules.

1. **Pace.** No milestone or parallel track starts while open or recently
   merged work in one-way-door areas (rule 2) is not yet understood by the
   owner. One milestone at a time by default; a parallel track needs the
   owner's go-ahead, and only for small, independent work.
2. **One-way and two-way doors.** Every question to the owner says which
   kind it is.
   - *One-way doors* are expensive to undo once there are users: file
     formats (model, trace), hash and identity definitions, public
     interfaces (CLI contract, `Component`, `TraceMessage`, the Python
     API), determinism rules, dependency policy. The owner answers these
     in their own words, one decision at a time, after seeing what the
     choice costs and what reversing it would cost.
   - *Two-way doors* are internal structure, test layout, CI details and
     the like. A recommendation that the owner accepts is enough.
3. **Verification over volume.** The owner does not review volume; machines
   do. Prefer checks a machine runs and the owner can trust: tests derived
   independently of the code (theory, published test vectors, hand
   counts), pinned hashes, identical results on every platform, mutation
   testing. A review by another agent supplements these and never replaces
   them.
4. **Understanding is a deliverable.** Each milestone gets one page in
   `docs/guide/`, written for the owner and new contributors: how the
   pieces fit, the main types, why the important decisions went as they
   did, the sharp edges. Before a pull request with a one-way door merges,
   the owner has had a hands-on walkthrough (using it, not reading the
   diff) and its guide page exists.
5. **Readable pull requests.** A pull request is small enough that the
   owner can read its non-test code. Its description has a section "What
   you need to understand" (at most five points) and lists its one-way
   doors, or says "None".

## Consequences

- Throughput drops. That is the point: the pace is set by the owner's
  understanding, not by the agents' capacity.
- The open PRs #21 (trace format) and #23/#24 (model format and hash)
  contain one-way doors. They wait for their walkthroughs and guide pages.
  #18, #19 and #20 need only a short walkthrough.
- CLAUDE.md carries the rules in short form, and the pull request template
  gets the two new sections, so every session and contributor applies
  them.
- The guide pages become the contributor documentation that spec §9 asks
  for.

## Questions for the owner

1. Please state the rule in your own words: this ADR is itself a one-way
   door for how the project is run.
2. Is the list of one-way doors in rule 2 complete? Anything to add or
   remove?
3. Rule 5 gives no number for "small enough". Do you want one, for example
   about 500 lines of non-test code, or is your judgement per PR enough?
4. Apply rule 4 to the open PRs (#21, #23, #24) before they merge, as
   above? Recommended: yes.
