# ADR-0015: Expression language details

- Status: proposed (accepted when the Track B pull request is merged)
- Date: 2026-10-25
- Blocks: M6 (edge conditions); used by M4 (observable references) and
  M8 (KPIs)
- Spec: §4 Routing and control logic, Observables and KPIs; §5 Contracts;
  ADR-0009 (accepted), ADR-0012

## Context

ADR-0009 settles the expression language: an own grammar with a
hand-written parser, keywords `and`/`or`/`not`, division by literals only
in v0.1, and its type rules. It leaves lexical rules and several typing and
evaluation details open. The implementation (`taktsim_model::expr`) has to
fix them; this ADR records each one. None changes a decision of ADR-0009.

One rule guided every choice: when in doubt, reject. Allowing something
later is compatible with existing models; forbidding it later is not.

## Options

Only a few details had real alternatives:

- **Run-time overflow.** (a) Return a typed error at run time; (b) use
  saturating arithmetic; (c) make it impossible by construction with a
  load-time bound. (a) contradicts ADR-0009 ("evaluation is total: no
  errors at run time"); (b) gives wrong answers silently.
- **Exact intermediates.** taktsim-units' checked arithmetic on its
  unsigned types, or exact signed `i128`. The units types fail on a
  negative intermediate, so `unit.waited - 1h < 10min` would be a run-time
  error for a unit that waited 30 minutes, although the condition is
  simply true.
- **Component ids in calls.** Plain identifiers only, or identifiers with
  hyphens. The spec's own ids are `weld-1`, `frame-assembly`.

## Recommendation

### 1. Lexical rules

- Whitespace between tokens: space, tab, LF, CR.
- Identifiers (path segments, function names): ASCII
  `[A-Za-z_][A-Za-z0-9_]*`. The keywords `and or not in true false` are
  reserved and case-sensitive; `AND` gets the hint "keywords are
  lowercase".
- Component ids, in call arguments only:
  `[A-Za-z_][A-Za-z0-9_]*(-[A-Za-z0-9_]+)*`, so `throughput(weld-1)`
  works. A call argument is never an expression, so the hyphen is not a
  minus. Keywords are not reserved there.
- Numbers: digits, optionally `.` and more digits. No sign, no exponent,
  no leading or trailing dot. Without a dot the literal is an `int`
  (0 to 2^64 − 1); with a dot it is a `number` (`f64`, must be finite).
- Quantity literals: a number, at most one space, a unit:
  `d h min s ms`, `parts part`, `parts/s parts/min parts/h` (and
  `part/…`), `%`. The literal text is parsed by `taktsim-units`, so
  `2.5 parts` or `0.0000000001s` fail with the units crate's message.
  `parts/h` is one unit only when `/` directly follows `parts` and a
  letter follows it; `120 parts / h` divides by a name `h`. A keyword
  after a number and one space ends the number (`3 and x`).
- Strings: single quotes, no escapes, any character except `'`. An enum
  value containing `'` cannot be written, so `TypeEnv` rejects one.

### 2. Grammar details

- Precedence, loosest first: `or`, `and`, `not`, comparisons and `in`,
  `+ -`, `* /`, unary `-`, atoms. Binary operators are left-associative.
- `in` has the precedence of the comparisons: `not x + 1 in [2]` is
  `not ((x + 1) in [2])`.
- Comparisons do not chain: `a < b < c` and `a == b in [c]` are rejected
  with the fix "join them with `and`".
- `not` and unary `-` appear once, as ADR-0009 writes them (`[ "not" ]`,
  `[ "-" ]`): `not not a` and `--a` are rejected with the fix
  `not (not a)`.
- List elements are atoms, as ADR-0009 writes them: `[-1]` must be
  written `[(-1)]`, and the error says so.
- Rejected with a fix: empty input, trailing commas, `in` without a list,
  a list outside `in`, a call on a dotted path (`a.b(x)`), `&&`, `||`,
  `!`, `=`, `<>`, `=<`, `=>`, double quotes, `%` as an operator.
- Nesting limit: 64 levels of tree depth and of open brackets
  (`MAX_DEPTH`), so every recursive pass runs in a small, fixed stack.
  `x in [a, b]` replaces long `or` chains.

### 3. Types

- `42` is an `int`, `4.2` a `number`, `30s` a duration (`quantity<time>`).
  A string literal takes the type of what it is compared with: a string,
  or an enum, in which case it must be one of the enum's values (with a
  "did you mean" suggestion by edit distance).
- No implicit conversions anywhere: `1 + 2.5` and `x < 2.5` for an `int`
  `x` are type errors.
- The operators and the only operand types they take:

  | Operator | Operands | Result |
  | --- | --- | --- |
  | `and` `or` `not` | bool | bool |
  | `+` `-` | T and T, T one of int, number, duration, count, rate, ratio | T |
  | `*` | int · int, number · number | the same |
  | `*` | duration · int, count · int, either order | the quantity |
  | `*` | rate · number, ratio · number, either order | the quantity |
  | `/` | number, rate or ratio / a number literal | the dividend's type |
  | `/` | count / a duration literal | rate |
  | unary `-` | int, number | the same |
  | `<` `<=` `>` `>=` | T and T, T one of int, number, duration, count, rate, ratio | bool |
  | `==` `!=` | T and T, T one of int, duration, bool, string, one enum | bool |
  | `x in [a, …]` | as `x == a` for each element | bool |

  An exact quantity (duration, count) is scaled by an int, so it stays
  exact; a floating-point quantity by a number. Not in v0.1, each
  addable later without breaking a model: a quantity times a quantity,
  dividing a duration or a count by a number, `int / int` (it would
  truncate), unary minus on quantities (they are never negative),
  ordering of bools, strings and enums.
- `!=` and `in` follow the rule ADR-0009 states for `==`.
- "Division by literals only": the divisor is a literal atom (parentheses
  allowed, a sign not), and a zero divisor is a load error.
- A bare number next to a quantity (`wip(buf1) <= 30`) is a type error
  whose fix names the unit: "write it with a unit, e.g. `30 parts`".
- `x in []` is rejected: it is always false.
- Paths are the full dotted names the environment declares; a prefix
  (`unit`) is not a value.
- Calls are the closed set the environment declares, each with an arity,
  optionally the component types it accepts, and a result type. Each
  argument is a component id that exists, listed once.
- The context says which result types it takes: `Bool` (conditions,
  constraints), `Numeric` (int, number or a quantity: observables, KPIs)
  or `Any`.

### 4. Evaluation

- Exact types (int, duration in nanoseconds, count) are evaluated in
  `i128`. The checker bounds every subexpression: inputs by their type
  (|int| ≤ 2^63, durations and counts ≤ 2^64 − 1), literals by their
  value; `+` and `-` add bounds, `*` multiplies them. An expression whose
  bound could exceed 2^127 − 1 is a load error (`MayOverflow`), so
  overflow at run time is impossible by construction and negative
  intermediate results are exact. Only absurd expressions are rejected:
  `unit.priority * unit.rank` is accepted, a third int factor is not.
- Floating-point types (number; rate in parts per second; ratio in
  percent) are evaluated in IEEE 754 binary64 with `+ - * /` only, which
  is identical on every tier-1 platform. They cannot fail: a result too
  large becomes infinite, and NaN (only from ∞ − ∞ or 0 · ∞) makes every
  ordering comparison false. `count / duration` is computed as
  `Count::per` does.
- The checker lowers an expression to a typed form in which an ill-typed
  expression cannot be represented, so evaluation has no run-time type
  checks other than of the supplied values.
- The only evaluation errors concern the supplied values: missing, of
  another type, a non-finite number, or an enum value outside its
  enumeration. Every input is read on every evaluation (no
  short-circuit), so such an error is always reported, deterministically.
- Evaluation is public for `bool` expressions (`Condition`). KPIs, which
  are evaluated over statistics, come with M8.

### 5. Canonical text

`Display` prints single spaces around binary operators, `, ` between
list elements and arguments, parentheses only where the grammar needs
them, numbers with a decimal point (`2.0`), and quantities as
`taktsim-units` displays them (`90min`, `40 parts`, `2%`). Parsing the
output gives the same tree apart from spans, for every tree within the
nesting limit (property-tested). Because a rate displays in parts per
second (ADR-0012), `120 parts/h` prints as
`0.03333333333333333 parts/s`.

### 6. Errors

One error per expression, the first in source order: an `ExprError` with
a byte span and a typed `ErrorKind` whose message says what was found,
what is allowed and the fix. `render(source)` adds the source line and a
caret. The model loader (M4) adds the field path.

## Consequences

- The lexical rules and the operator table are part of format 1.0, like
  the grammar (ADR-0009).
- M4 must give component ids a form the rule in §1 accepts, or calls
  cannot name them.
- Every rule rejected now can be allowed later without breaking a model.
- No new dependency: the module uses `taktsim-units` and `thiserror`.

## Questions for the owner

Found while implementing. ADR-0009 is accepted, so none is decided here;
the implementation follows ADR-0009 as written.

1. ADR-0009's context and spec §6 write `wip(buf1) <= 30` without a
   unit. ADR-0009's rules (no implicit conversions; a quantity is a
   number with a unit) make that a type error, which is what the code
   does, with the fix `30 parts`. Should the examples read `30 parts`?
2. ADR-0009 and spec §6 write the decision variable
   `buf1.capacity in [5, 50]`, which means the interval 5 to 50. In a
   condition, `x in [5, 50]` means "5 or 50". One syntax with two
   meanings; it needs a decision before v0.4 (for example `in 5..50`).
3. ADR-0009 rejects `==` on every quantity except time. A count from a
   unit attribute is an exact integer; is `==` on counts meant to be
   rejected too? (The code rejects it, as written.)
4. The observables do not all fit the scalar types: `wip(buf1)` is a
   time-weighted average, not a whole `Count`, and
   `state_shares(cut, weld)` is one share per state and resource. The
   environment lets M8 declare any result type; the types of the
   built-in observables need a decision there.
5. ADR-0009's grammar makes list elements atoms, so a negative element
   needs parentheses: `[(-1)]`. Should list elements be `unary` instead?
