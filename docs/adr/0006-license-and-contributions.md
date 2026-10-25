# ADR-0006: License and contributor agreement

- Status: accepted (2026-10-25)
- Date: 2026-10-25
- Blocks: M0 (LICENSE file, `cargo-deny` policy, crate metadata)
- Spec: §15 "License", §9 Licensing, §13

This ADR lays out options and trade-offs; it is not legal advice. Spec §12
already plans a lawyer's review for the contribution terms of the
parameter library; the license and contribution model can go to the same
review.

## Context

All code is open source (default: Apache-2.0); docs and the public
parameter library are CC-BY-4.0. Nothing is ever paid (§13). Two open
questions: Apache-2.0 alone or MIT/Apache-2.0 dual licensing, and whether
contributors sign a CLA (which keeps a later license change possible but
deters some contributors). The license must be fixed before the first
crate is published, and a CLA (if any) before the first external PR.

## Options

### License

- **L1. Apache-2.0 only.** Explicit patent grant and patent-retaliation
  clause; incompatible with GPL-2.0-only projects.
- **L2. MIT OR Apache-2.0 (dual).** The Rust ecosystem convention (Rust
  itself, most core crates). Users pick: the Apache-2.0 patent grant, or
  MIT for GPL-2.0 compatibility and simplicity.

### Contributions

- **C1. CLA** (assignment or broad license grant to the project). Allows
  relicensing; needs a signing bot, a legal entity to hold the rights, and
  usually deters some contributors, especially those at companies.
- **C2. DCO** (`Signed-off-by:` on each commit, checked by a bot). Certifies
  the right to contribute; no relicensing rights; little friction.
- **C3. Nothing beyond the license** ("inbound = outbound").

## Trade-offs

| | L1 | L2 |
| --- | --- | --- |
| Patent protection for users | yes | yes if they choose Apache-2.0 |
| GPL-2.0 compatibility | no | yes (via MIT) |
| Matches Rust ecosystem | partly | yes |
| Simplicity | one text | two texts, standard pattern |

| | C1 CLA | C2 DCO | C3 none |
| --- | --- | --- | --- |
| Relicensing later | possible | needs every contributor's consent | same as DCO |
| Contributor friction | high | low | none |
| Provenance record | yes | yes, per commit | no |
| Needs a legal entity | yes | no | no |

## Recommendation

- **L2, MIT OR Apache-2.0** for all code, including the viewer. It is what
  Rust and Python users expect, and it keeps the patent grant available.
- **C2, DCO**, enforced by a CI check. The project commits to never
  charging (§13), so the main reason for a CLA, relicensing, carries
  little weight here, while the v1.0 criterion needs external
  contributors.
- Docs and benchmark data: CC-BY-4.0. Code examples inside the docs are
  also covered by the code license, so users can copy them.
- `cargo-deny` allow-list: MIT, Apache-2.0, BSD-2/3-Clause, ISC, Zlib,
  Unicode-3.0, MPL-2.0 (file-level copyleft, compatible with
  distribution); everything else is denied until reviewed.

## Consequences

- `LICENSE-MIT`, `LICENSE-APACHE`, crate metadata
  `license = "MIT OR Apache-2.0"`, an SPDX header policy, and a DCO check
  in M0.
- A later license change would need every contributor's consent.

## Resolved questions

The owner accepted every recommendation in this ADR on 2026-10-25. The
questions as they were asked:

1. Dual MIT/Apache-2.0, or Apache-2.0 only as the spec default?
2. DCO, CLA, or nothing?
3. Will a legal entity (association or foundation) hold the project?
   That matters for a CLA, trademarks (ADR-0008) and the EU-operated
   library service.

Decided: MIT OR Apache-2.0, DCO. Question 3 is still open; with a DCO
instead of a CLA it does not block v0.1, but it must be answered before
the trademark registration and the library service (v0.3).

`LICENSE-MIT` (copyright "The taktsim contributors") and `LICENSE-APACHE`
(text from apache.org) are in the repository root.
