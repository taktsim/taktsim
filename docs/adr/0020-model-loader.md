# ADR-0020: Model loader: parsing, decoding, component entries, canonical hashing and schema

- Status: proposed (accepted when the M4 pull requests are merged)
- Date: 2026-10-26
- Blocks: M4; used by M5 (component entries), M6 (edges), M7 (`validate`,
  errors), M8 (observables), M12 (Python errors)
- Spec: §2 principles 2 and 3, §4 (model document, rules), §5 Contracts,
  §7 Layer 1, §10 (`schemas/`); ADR-0002, ADR-0003, ADR-0008, ADR-0009,
  ADR-0010, ADR-0012, ADR-0013, ADR-0015, ADR-0018 (deviation 5), ADR-0019

## Context

ADR-0002 fixes the model language: a YAML 1.2 subset or JSON, a pipeline
that keeps source spans, strict decoding, `include:` with content hashing,
normalization, and a model hash that is SHA-256 of the RFC 8785 (JCS)
canonical JSON of the normalized document. It leaves the YAML parser crate
to M4. ADR-0003 fixes the distribution syntax. M4 also has to choose how
documents are decoded, how the schema is generated (spec default:
`schemars`), the JCS and SHA-256 implementations and the layout under
`schemas/`, and it must define how a component entry is represented and
checked before the component types exist (they arrive with M5, ADR-0018
deviation 5) without M5 changing the document format or the hash.

Each decision below is a detail of an accepted ADR or a default the spec
leaves to the implementation. None changes the spec or an accepted ADR;
where M4 reads one of them, the reading is stated.

## Decisions

### 1. YAML parser: `saphyr-parser` 0.3.2

| | `saphyr-parser` 0.3.2 | `yaml-rust2` 0.13 | `saphyr` (loader) | serde-based (`serde-saphyr`, `serde_yml`) |
| --- | --- | --- | --- | --- |
| Spans | start and end of every event | start only | none kept | none |
| Duplicate keys | events: the loader detects them | events: same | silently overwritten (hash map) | depends; no location |
| YAML 1.2 | passes the yaml-test-suite | passes it | as `saphyr-parser` | as their parser |
| Anchors, aliases, tags | reported as events, so rejectable | same | resolved | resolved |
| Maintenance | active (successor of `yaml-rust2`) | "basic maintenance" (its README) | active | `serde_yml` deprecated |
| Dependencies | `arraydeque`, `thiserror` | `arraydeque`, `hashlink`, `encoding_rs` | more | more |
| License | MIT OR Apache-2.0 | MIT OR Apache-2.0 | MIT OR Apache-2.0 | various |

`saphyr-parser`'s event API is used in one module,
`taktsim-model/src/syntax/yaml.rs`, which builds the loader's own tree and
enforces the subset; replacing the crate touches only that file.

### 2. JSON has its own parser

YAML 1.2 is nearly a superset of JSON, but `saphyr-parser` rejects valid
JSON (`{"a":\t1}`, a tab after the colon) and accepts much that is not
JSON. `serde_json` keeps the last of duplicate keys and has no spans. So
JSON is read by a strict RFC 8259 parser in `syntax/json.rs` (about 300
lines) that produces the same tree with locations and rejects duplicate
keys. Both formats then share everything else.

### 3. The YAML subset, exactly (detail of ADR-0002)

- One document per file; no anchors, aliases, tags or merge keys (`<<`);
  mapping keys are text; a key appears once per mapping; lists and
  mappings nest at most 64 levels (both formats); a byte order mark is
  ignored.
- Plain scalars follow the YAML 1.2 core schema, restricted so that every
  YAML parser and the JSON form read a model the same way:
  - `true` and `false` are booleans; `null`, `Null`, `NULL`, `~` and an
    empty value are null.
  - Numbers follow the JSON grammar. Integers are within ±(2^53 − 1) (the
    I-JSON range, RFC 7493), so canonical JSON represents each exactly;
    a seed therefore goes up to 2^53 − 1. Other numbers must be finite.
  - Any other plain scalar that a YAML 1.1 or 1.2 parser reads as a
    boolean, number or date is an error that says to quote it: `yes`,
    `No`, `ON`, `y`, `True`, `012`, `0x1F`, `0o17`, `+1`, `.5`, `1.`,
    `1_000`, `1:30`, `.inf`, `.nan`, `2026-10-26`. This is the
    "`no` → false" family ADR-0002 names, rejected rather than resolved,
    in the spirit of ADR-0015 ("when in doubt, reject"); it can be relaxed
    later without breaking a model.
  - Everything else is text: `42s`, `20 parts`, `line-a`.
- Locations are a line and a column, both from 1; the column counts
  characters, not bytes.

### 4. Decoding: an own typed decoder; serde only for output

| | serde `Deserialize` over the tree | own `Decode` trait |
| --- | --- | --- |
| Errors per run | the first only | all of them (spec §5: `validate` returns a list) |
| Typed errors | parse errors of quantities pass through `de::Error::custom` as text | `UnitError`, `DistributionError`, `ExprError` kept as typed values |
| Spans | lost inside internally tagged and untagged enums (distributions, flow entries), which serde buffers | exact for every value |
| Code per type | a derive | a short `decode` function |
| Drift between decoder and output | none | possible; closed by tests (below) |

**Own decoder.** `Decode` (with `EXPECTED` and `EXAMPLE` for messages), a
`Decoder` that collects errors with their field paths, and `Fields` for a
mapping: each key read once, every key not read reported as unknown with
the keys that were, and a "did you mean" suggestion. The normalized form is
produced by `serde::Serialize` into a `serde_json::Value`; the test that
the canonical form of every model loads back to itself (property-tested)
keeps decoder and serializer in step. `serde` and `serde_json` (both MIT OR
Apache-2.0, already in the lockfile) are dependencies of `taktsim-model`;
`taktsim-units` gains a `serde` feature that serializes each quantity as
its canonical text.

### 5. One error type

- `LoadError`: file (as named in the load call or in `include:`), line and
  column, field path, and a `LoadErrorKind`. `LoadErrors`: every error of
  a load, never empty, in reading order (by file, the model first, then by
  location).
- Every kind gives `found()`, `allowed()` and, where a valid alternative
  exists, `example()`, so the CLI can print them as fields (spec §5,
  ADR-0019 item 3); `is_io()` separates "cannot read a file" (exit code 7)
  from invalid input (exit code 1). Errors of `taktsim-units`, of the
  distribution constructors and of the expression language are wrapped as
  typed values.
- Paths are written `components[0].cycle_time.sigma`; a key that is not an
  identifier is written in brackets (`experiment.precision["throughput(sink)"]`);
  `pointer()` gives the RFC 6901 JSON Pointer.
- An error inside an expression points at its character when the scalar is
  written as is (plain, or quoted without escapes, on one line), otherwise
  at the scalar.
- Checks across sections (references, flow graph, observables) run only
  when every section decoded and every included file was read, so one
  mistake is not reported again as many.

### 6. Component entries: the M4/M5 boundary

**Format.** A component is one flat mapping, as in spec §4's examples:
`{id: cut, type: Machine, cycle_time: 42s}`. `id` has the component-id form
of ADR-0015 (`[A-Za-z_][A-Za-z0-9_]*(-[A-Za-z0-9_]+)*`), so every id can be
named in an expression; ids are unique, also ignoring case (run outputs per
component must not collide on case-insensitive file systems). `type`
selects the component type; every other key is a parameter of that type.

**Rust.** A component type's parameters are its own Rust struct (ADR-0018
deviation 5) implementing `ComponentParams`: `TYPE`, an `EXAMPLE` entry,
`decode(&mut Fields)` and `references()` (ids it names, checked by the
loader), plus `Serialize`. The loader takes a `Catalog` of component types
and observables from its caller; `taktsim-model` registers none. How the
catalog is filled (the `inventory` crate, the spec's default, or an
explicit list) is M5's decision: the loader only needs `&Catalog`. The
observables of `observe` and `experiment.precision` are declared in the
same catalog with their signatures, whose result types M8 decides
(ADR-0015, answer 5).

**Checks.** An unknown `type` is an error listing the registered types.
Hence no model with components loads in M4 alone: by design, the tests use
stand-in types. A type that rejects its parameters without recording why
still produces an error.

**Normalized form.** `{id, type}` plus the type's `Serialize` output, which
must be a mapping without `id` or `type` keys and must leave out absent
optional parameters; quantities and distributions serialize in canonical
form. A type that breaks this gets an error, not a panic.

**Why M5 changes neither format nor hash.** The entry syntax and the hash
function are fixed here; M5 adds types to a catalog. A model's hash exists
only once its types do (before, it does not load), and it is the hash this
ADR defines. Because absent optional values are never written into the
normalized form, adding an optional parameter later leaves every existing
model's hash unchanged.

### 7. Normalization (reading of ADR-0002)

- Quantities are written in canonical text (`taktsim-units`' `Display`):
  `0.5h` and `30min` normalize to `30min`, `20 part` to `20 parts`; a rate
  is written in `parts/s` (ADR-0012).
- ADR-0002's "bare quantity → constant": ADR-0003 has no second spelling of
  a constant (no `{dist: constant}`), so the expansion is the identity and
  a constant stays the bare duration. Should another spelling be added, it
  normalizes to the bare duration.
- Flow: every entry becomes edges `{from: [ids], to: [ids]}`; a chain
  `[a, b, c]` is `a → b`, `b → c`; a single id is a one-element list. The
  order of edges and of ids is kept (M6's choice rules may give list order
  a meaning, such as preference).
- Components keep their order: the engine starts components in the order
  added (ADR-0018), so the order is part of the model.
- Observables and precision keys are written in the expression language's
  canonical text (ADR-0015 §5): `throughput( sink )` is `throughput(sink)`.
- `include:` is resolved and left out; absent or empty optional sections
  are left out (`observe: []` is no `observe`).
- The normalized form is itself a valid model, in JSON, with the same hash.

### 8. Canonical JSON and the hash

**JCS: own code** (`taktsim_model::jcs`, about 150 lines), not a crate: the
crates found are small and unmaintained (`serde_jcs` 0.1) or do not state
ECMAScript's tie rule, and byte exactness is the whole point. Keys are
sorted by UTF-16 code units, independent of `serde_json`'s map type (so
feature unification of `preserve_order` cannot change a hash). Numbers use
Rust's shortest round-trip digits, laid out as ECMAScript's
`Number.prototype.toString` does. One trap: on an exact tie between two
shortest candidates Rust rounds up, ECMAScript picks the even digit (RFC
8785 Appendix B: `1424953923781206.25` must be written
`1424953923781206.2`); the code checks ties against the exact decimal
expansion. Verified against RFC 8785 Appendix B, the examples of §3.2.2 and
§3.2.3, and 3,500 doubles written by Node.js 26.12.0 (1,000 of them exact
ties), with the generator committed next to the vectors.

**SHA-256: `sha2` 0.11** (RustCrypto, MIT OR Apache-2.0), default features
off. It detects SHA instructions at run time (`cpufeatures`, already in the
lockfile) and falls back to portable code; a hash is the same either way,
which is ADR-0010's rule for CPU-specific code. The hash is displayed as 64
lowercase hex digits. The canonical form and hash of spec §4's example are
pinned in a test that runs on every tier-1 target.

### 9. `include:` (detail of spec §4 and ADR-0002)

```yaml
include:
  - cells/welding.yaml
  - {path: cells/paint.json, sha256: 7a4bada22af401f3ddb7c1416ebf85875bf5192061775e00038aedca29870a33}
```

- A list in the model file only; an included file holds any of
  `components`, `flow` and `observe`, and cannot include further files.
  Its entries come before the model file's own, in `include:` order.
- Paths are relative to the model file, use `/`, contain only letters,
  digits, `.`, `_` and `-`, have no `..` or empty segments, and name a
  `.yaml`, `.yml` or `.json` file; each file is listed once, ignoring case;
  a model cannot include itself. So a model directory reads the same on
  every platform.
- Every included file is hashed (SHA-256 of its bytes); the hashes are
  returned with the model (`Model::includes`) for provenance. A `sha256`
  pin makes a changed file an error that shows the actual hash.
- The model hash covers the resolved content, not file names: moving
  entries into an included file (and keeping their order) keeps the hash.
- Files are read through a `Resolver`: the model's directory for
  `Loader::load_file`, or a map of path to text for models held in memory
  (tests, the browser viewer).

### 10. Document details settled here

- Required: `format: "1.0"`, `model.name` (component-id form),
  `experiment.type`, `experiment.horizon` (longer than zero), `components`
  and `flow` (each at least one entry, from the model file or its includes).
  Optional: `model.time_unit` (`d h min s ms`), `model.seed` (0 to
  2^53 − 1), `experiment.precision` (observable → ratio above 0 % and at
  most 100 %, ADR-0014), `observe`, `include`. When in doubt a field is
  required; making it optional later breaks no model.
- `type: steady_state` is rejected with the reason: spec §6 requires a
  warm-up rule for it, and that rule's format belongs to M9.
- An edge has only `from` and `to`; `when`, `choose` and split ratios
  arrive with M6. An edge from a component to itself and an edge declared
  twice are errors (relaxable in M6 if conditional edges need repeats).
- `observe` and `experiment.precision` take observables only: calls of a
  catalog function on component ids. User-defined KPIs are v0.2.
- A different `format` version stops decoding with that one error, since
  its sections may mean something else.

### 11. JSON Schema (second M4 pull request)

- **Generator: `schemars` 1.x** (MIT), the spec's default, JSON Schema
  draft 2020-12. Quantities get a `schemars` feature in `taktsim-units`;
  distributions, component references and the document sections implement
  `JsonSchema` next to their decoders, describing the input format with its
  shorthands; component parameter structs derive it (M5).
- **Layout:** `schemas/model/1.0.json`, one file per format version (spec
  §10); `schemas/cli/describe.json` follows with M7 (ADR-0019). Generated
  by a binary in the repository and checked by `ci/check-schemas.sh` in CI
  (a tightening of the gates), which has its own test that a stale schema
  fails.
- The schema's component entries come from the catalog the generator is
  given. In M4 the catalog is empty, so the committed schema admits no
  component type, exactly as the loader does; M5 generates it with its
  catalog.
- A contract test checks the corpus against the schema: every valid model
  and included file is accepted, and every malformed model whose error a
  schema can state is rejected (51 of the 86 that parse; the rest need
  the whole document, such as unknown references). The JSON Schema
  validator crates (`jsonschema`, `boon`) depend on `borrow-or-share`,
  licensed MIT-0, which `deny.toml` does not allow (dev-dependencies are
  not license-checked, but the allow-list is the policy). So the test has
  its own checker for exactly the keywords the generator emits, and a
  test fails if the schema uses any other keyword. The patterns use
  `regex` (already in the lockfile) and are property-tested against the
  units parser.
- `null` is never valid: derived schemas let optional fields be null, so
  the generator removes `null` from every type (an optional parameter is
  left out, not written as null).
- No `$id` until the owner chooses one (question 9).

## Consequences

- `validate` (M7) gets every error of a model in one run, each with file,
  line, column, path, what was found, what is allowed and an example.
- A model's hash is the same for every spelling and on every platform:
  property tests write generated models in YAML block and flow style and in
  JSON, with any key order, quoting, quantity spelling, chain or edge form
  and spacing inside expressions, and compare hashes.
- M5 implements `ComponentParams` for Source, Machine, Buffer and Sink and
  fills a catalog; it changes neither the document format nor the hash.
- New dependencies of `taktsim-model`: `saphyr-parser` (with `arraydeque`)
  and `sha2` (with `digest`, `block-buffer`, `crypto-common`,
  `hybrid-array`, `typenum`), all MIT OR Apache-2.0 (`typenum`: MIT OR
  Apache-2.0); `serde` and `serde_json` were already in the lockfile.
  `taktsim-model` now depends on `taktsim-core` for the distribution
  constructors (allowed: every crate depends on core). The schema adds
  `schemars` and `schemars_derive` (MIT) with `dyn-clone`, `ref-cast`,
  `ref-cast-impl` and `serde_derive_internals` (MIT OR Apache-2.0), and
  `regex` (already in the lockfile) for tests.

## What the spec's examples need (found while testing them)

The tests read the examples from `docs/spec.md` and `README.md` and state
exactly why each does not load as written:

1. Spec §4's model document and the README name `source` and `sink` in
   `flow` and `observe` without declaring them. With references checked
   (spec §4 Rules) they are unknown components. `data/valid/spec-line-a.yaml`
   declares them.
2. Spec §4's model document has `shifts:`, a v0.2 feature (ShiftCalendar),
   in a document that is otherwise v0.1.
3. Spec §4's routing example uses `choose:` and `when:` (M6) and `control:`
   and `orders:` (v0.2); its materials example uses `materials:`,
   `products:` and Assembly (v0.2).
4. Spec §4's materials example writes `scrap_rate: 0.02`, a bare number for
   a share, while spec §4 Rules say every quantity carries a unit (`2%`).

## Questions for the owner

1. Are `source` and `sink` in spec §4 and the README meant to be implicit
   components (for example an unlimited supply and demand), or should the
   examples declare them? M4 requires them declared, which either answer
   can relax.
2. Should spec §4's v0.1 example keep `shifts:`, or mark it as v0.2?
3. Is it right that a constant has no `{dist: constant}` spelling, so its
   normalized form is the bare duration (section 7)?
4. The ambiguous-scalar rule (section 3) rejects YAML 1.1 booleans
   including `y` and `n`, and dates. Strict enough, or too strict?
5. Ids unique ignoring case (section 6): agreed?
6. `include:` semantics (section 9): included entries first, included files
   hold only `components`, `flow` and `observe`, no nesting. Agreed?
7. The required and optional fields of section 10, and rejecting
   `steady_state` until M9.
8. What `model.time_unit` means: every quantity carries its unit, so it
   does not change how a model reads. The unit of reported results?
9. A `$id` for the schemas: `https://github.com/taktsim/taktsim/…`, a
   domain, or none?
10. Shares such as `scrap_rate`: a ratio with a unit (`2%`) or a bare
    number (spec §4's example)? Needed by v0.2's Inspection; recorded here
    because the rule is a model-format rule.
11. MIT-0 (`borrow-or-share`, used by the JSON Schema validator crates):
    add it to the license allow-list (a gate change, its own pull
    request), so a standard validator can replace the test's own checker?
