# ADR-0002: Model language and canonical hashing

- Status: accepted (2026-10-25)
- Date: 2026-10-25
- Blocks: M4 (model format and loader)
- Spec: §15 "Model language", §4 Rules, §5 Contracts (run id is a hash)

## Context

Models are YAML or JSON documents validated against a JSON Schema generated
from the Rust types. A run id is the hash of model, seed, horizon and engine
version, so the same model must hash the same whichever syntax it was
written in. The spec's examples are YAML, so YAML is what people will
write. The spec's default recommendation: JSON is canonical; YAML is
accepted and normalized to JSON before hashing.

YAML brings well-known hazards: implicit typing (`no` → false in YAML 1.1,
`1e3` as a number, `012` as octal), anchors and aliases (including
billion-laughs expansion), custom tags, duplicate keys silently overwritten
by many parsers, and multi-document streams. `serde_yaml` is archived and
unmaintained since 2024; the Rust YAML ecosystem is fragmented.

Spec §14 (v0.4) requires parameter edits to round-trip to the model file
"without other changes", so the source text itself must stay editable,
not only its parsed form.

## Options

- **O1. YAML is canonical**, JSON accepted. Hash over parsed YAML. Hashing
  needs a canonical form anyway, which is effectively JSON.
- **O2. JSON is canonical; YAML is a strict subset, normalized to JSON
  before validation and hashing** (spec default).
- **O3. JSON only.** Simplest and safest, but every spec example and every
  human author works in YAML; comments would be impossible.

## Trade-offs

| | O1 | O2 | O3 |
| --- | --- | --- | --- |
| Authoring for people | good | good | poor (no comments) |
| Agents | fine | fine, may emit either | best |
| Hash stability | needs a canonical form anyway | canonical JSON is well defined (RFC 8785) | same as O2 |
| Attack and surprise surface | full YAML | restricted subset | none |
| Parser dependency risk | high | medium (subset only) | low |

## Recommendation

**O2**, with the subset made explicit:

- **YAML subset:** YAML 1.2 core schema; one document per file; no anchors,
  aliases, tags or merge keys; duplicate keys are errors; mapping keys are
  strings. A violation is a load-time error with line and column.
- **Scalars:** quantities are written as strings with units (`42s`), so most
  implicit-typing issues do not arise; plain booleans are only `true` and
  `false`.
- **Pipeline:** text → parse (YAML subset or JSON) → JSON value with source
  spans kept for error messages → resolve `include:` (each included file
  hashed, hash recorded) → strict deserialization into Rust types (unknown
  keys rejected, errors with field path) → normalized value → model hash.
- **Model hash:** SHA-256 of the RFC 8785 (JCS) canonical JSON of the fully
  resolved, normalized document. Normalization expands shorthands (flow
  chain → edges, bare quantity → constant) so equivalent spellings share
  a hash.
- **Parser crate:** chosen in M4 among maintained YAML 1.2 parsers that
  expose spans and reject duplicates (candidates: `saphyr`, `yaml-rust2`);
  wrapped in one module so it can be replaced.
- **Write-back (v0.4):** edits are applied to the source text by span, not
  by re-serializing, so comments and layout survive. That is why the
  pipeline keeps spans from the start.

## Consequences

- Error messages can point to line and column in the file the author
  wrote, plus the field path.
- Two files that differ only in comments, key order or shorthand hash the
  same. Wanted for reproducibility; documented so nobody is surprised.
- `serde`'s `deny_unknown_fields` does not combine with `#[serde(flatten)]`;
  model types avoid `flatten` or use a custom check.

## Resolved questions

The owner accepted every recommendation in this ADR on 2026-10-25. The
questions as they were asked:

1. Confirm the subset, in particular forbidding anchors and aliases.
   Authors of large models may want them; `include:` covers reuse instead.
2. Should YAML be the documented primary format (examples, tutorial) with
   JSON as the agent format? Recommended: yes.
