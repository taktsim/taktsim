# ADR-0009: Expression language

- Status: accepted (2026-10-25); call arguments and the `30 parts`
  example amended by ADR-0015 (2026-10-26)
- Date: 2026-10-25
- Blocks: M6 (routing: edge conditions)
- Spec: §15 "Expression language", §4 Routing and Observables, §6
  objectives and constraints

## Context

Typed expressions appear in several places:

- **Edge conditions** (v0.1): `when: "variant == 'premium'"`, over unit
  attributes.
- **Observable references** (v0.1): `throughput(sink)`, `wip(buf1)` in
  `observe` and `precision`.
- **User-defined KPIs** (v0.2+), **objectives and constraints**
  (`wip(buf1) <= 30`, v0.4), **decision variables** (`buf1.capacity in
  [5, 50]`, v0.4).

The spec requires: parsed and type-checked at load time; no loops, no I/O,
no user functions; small enough to verify exhaustively. Errors must name
the field and the fix like every other load error.

## Options

- **E1. Own minimal language**, hand-written recursive-descent parser (no
  dependency) or a parser-combinator crate (`winnow`, `chumsky`).
- **E2. CEL (Common Expression Language)** through a Rust implementation.
  Specified, non-Turing-complete, used in Kubernetes and Envoy.
- **E3. Structured expressions in the document** (JSON-Logic style:
  `{eq: [variant, premium]}`), no string parsing.

## Trade-offs

| | E1 own | E2 CEL | E3 structured |
| --- | --- | --- | --- |
| Matches spec examples | yes | mostly | no |
| Units and quantity types | built in | not native; would need extensions | possible but verbose |
| Exhaustive verification | feasible: small grammar | large spec, Rust implementations do not fully cover type checking | feasible |
| Error messages | our spans and fix hints | from the library | field paths |
| Schema describes it | as a string with a documented grammar | same | fully |
| Dependency risk | none or small | medium | none |

## Recommendation

**E1**, one grammar shared by all uses, with a different typing
environment per context (edge conditions see unit attributes; KPIs and
objectives see observables). The v0.1 implementation covers edge
conditions and observable references; the rest of the grammar is
specified now so later tiers do not change it.

Draft grammar (to be refined in M6, then frozen with format 1.0):

```ebnf
expr      = or ;
or        = and { "or" and } ;
and       = not { "and" not } ;
not       = [ "not" ] cmp ;
cmp       = sum [ ( "==" | "!=" | "<" | "<=" | ">" | ">=" ) sum
                | "in" list ] ;
sum       = term { ( "+" | "-" ) term } ;
term      = unary { ( "*" | "/" ) unary } ;
unary     = [ "-" ] atom ;
atom      = quantity | number | string | bool | call | path
          | "(" expr ")" ;
call      = ident "(" [ ident { "," ident } ] ")" ;   (* fixed built-ins only *)
path      = ident { "." ident } ;
list      = "[" [ atom { "," atom } ] "]" ;
quantity  = number unit ;                            (* 30s, 40 parts *)
string    = "'" { char } "'" ;
```

Type rules:

- Types: `bool`, `int`, `number`, `quantity<D>` (dimension from
  `taktsim-units`), `string`, and enums (product, variant, quality
  state), whose values are checked against the model at load time.
- No implicit conversions. Arithmetic and comparison on quantities need
  matching dimensions; `2 parts / 1h` gives a rate. `and`/`or`/`not` take
  `bool` only.
- `==` on `number` or `quantity` is rejected (compare with `<=`/`>=`
  instead); on integer-tick time it is allowed.
- Calls are a closed set of built-in observables (`throughput`, `wip`,
  `state_shares`, `lead_time`, ...); arguments must be component ids that
  exist.
- Evaluation is total: no errors at run time. Division by a quantity that
  can be zero is a load-time error unless it is guarded, or, simpler, `/`
  is allowed only by literals in v0.1.

Verification: a test per grammar production and per type rule; proptest
that pretty-print → parse is the identity on generated ASTs; fuzzing of
the parser in the nightly job.

## Consequences

- The grammar is part of the model format version.
- Custom logic beyond this language stays in Rust components, as the spec
  intends.

## Resolved questions

The owner accepted every recommendation in this ADR on 2026-10-25. The
questions as they were asked:

1. Own language (E1) rather than CEL: agreed?
2. Keep `and`/`or`/`not` keywords, or use `&&`/`||`/`!`? Recommended:
   keywords, easier to read in YAML.
3. Division restricted to literals in v0.1: acceptable?
