# ADR-0015: Expression language details

- Status: proposed (needs the owner's explicit acceptance: it changes one
  rule of ADR-0009's grammar, see "Change to ADR-0009", and it asks
  questions)
- Date: 2026-10-25, revised 2026-10-26 after the review of PR #10
- Blocks: M6 (edge conditions); used by M4 (observable references) and
  M8 (KPIs)
- Spec: §4 Routing and control logic, Observables and KPIs; §5 Contracts;
  ADR-0009 (accepted), ADR-0012

## Context

ADR-0009 settles the expression language: an own grammar with a
hand-written parser, keywords `and`/`or`/`not`, division by literals only
in v0.1, and its type rules. It leaves lexical rules and several typing and
evaluation details open. The implementation (`taktsim_model::expr`) has to
fix them; this ADR records each one.

All but one only add detail to ADR-0009. One changes its grammar:

### Change to ADR-0009

ADR-0009's grammar writes a call as `call = ident "(" [ ident { ","
ident } ] ")"`: its arguments are identifiers. This ADR makes them
**component ids** instead: `call = ident "(" [ component-id { ","
component-id } ] ")"`, with `component-id =
[A-Za-z_][A-Za-z0-9_]*(-[A-Za-z0-9_]+)*`. That allows hyphens
(`throughput(weld-1)`; the spec's own ids are `weld-1`, `frame-assembly`)
and allows words that are reserved elsewhere (`f(in)`, `f(AND)`). A call
argument is never an expression, so the hyphen cannot be read as a minus.
With plain identifiers, no component whose id contains a hyphen could be
named in an expression.

One rule guided every other choice: when in doubt, reject. Allowing
something later is compatible with existing models; forbidding it later is
not.

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
- **Component ids in calls.** Plain identifiers (ADR-0009 as written) or
  identifiers with hyphens: see "Change to ADR-0009".
- **Depth limit.** Check depth in every recursive pass, or make a deeper
  tree impossible to build. The first cannot keep "every tree prints and
  parses back" true and needs a fallible `Display`; the second costs a
  checked constructor.

## Recommendation

### 1. Lexical rules

- Whitespace between tokens: space, tab, LF, CR.
- Identifiers (path segments, function names): ASCII
  `[A-Za-z_][A-Za-z0-9_]*`, not a keyword. The keywords
  `and or not in true false` are lowercase; the same words in another case
  (`AND`, `True`) are reserved too: an error with the fix "keywords are
  lowercase: write `and`", wherever they appear, never a name.
  `TypeEnv` rejects such names.
- Component ids, in call arguments only: see "Change to ADR-0009".
- Numbers: digits, optionally `.` and more digits. No sign, no exponent,
  no leading or trailing dot. Without a dot the literal is an `int`
  (0 to 2^64 − 1); with a dot it is a `number` (`f64`, must be finite).
- Quantity literals: a number, at most one space, a unit. The units and
  their parsing belong to `taktsim-units`: each of its quantities parses
  the literal in turn, and the list of units in error messages is read
  from its own "unknown unit" errors, so the two cannot drift. Today:
  `d h min s ms`, `parts part`, `parts/s parts/min parts/h` (and
  `part/…`), `%`. `2.5 parts` or `0.0000000001s` fail with the units
  crate's message. A slash belongs to the unit only when `taktsim-units`
  has a unit with that word and a slash (`parts/h`) and a letter follows
  it; `120 parts / h` divides by a name `h`. A keyword in any case after a
  number and one space ends the number (`3 and x`, `3 AND x`).
- `%` between two operands (`10 % 3`) is rejected with "there is no
  remainder operator"; `10% and x` is a percentage.
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
- Depth limit (`MAX_DEPTH` = 64): a syntax tree has at most 64 levels.
  The tree's fields are private and its only constructor, `Expr::new`,
  refuses a deeper node, so a deeper tree cannot exist; checking,
  printing, evaluating and dropping a tree therefore run in a small, fixed
  stack. The parser also counts open parentheses against the limit (the
  only place it recurses); brackets do not count, so the printed text of
  every tree that exists parses back. `x in [a, b]` replaces long `or`
  chains.

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
- A bare number next to a quantity is a type error whose fix names the
  unit (`wip(buf1) <= 30`: "write `30 parts`"), but only where the fixed
  expression then type-checks. For `==`, `!=` and `in` that is a
  duration; next to a count, rate, ratio or number the error says that
  `==` does not apply (`wip(buf1) == 30`), and offers no unit.
- `x in []` is rejected: it is always false.
- Paths are the full dotted names the environment declares; a prefix
  (`unit`) is not a value.
- Calls are the closed set the environment declares, each with an arity,
  optionally the component types it accepts, and a result type. Each
  argument is a component id that exists, is of an accepted type, and is
  listed once. Suggestions and examples use only components the function
  takes.
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
  checks other than of the supplied values. An enum's values are shared
  with the environment, not copied per reference.
- An input (a path or a call) carries its canonical text, computed once
  with the same printer as the syntax tree; a map of values keyed by that
  text is looked up without allocating.
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
output gives the same tree apart from spans, for every tree that can be
built (property-tested, also for trees of the full depth in every shape).
Because a rate displays in parts per second (ADR-0012), `120 parts/h`
prints as `0.03333333333333333 parts/s`.

### 6. Errors

- One error per expression: the first in reading order. An operand's own
  errors come first, then whether it fits its operator, then the next
  operand: in `1 and (x + 2.0)` the error is that `1` is not a bool, not
  the mismatch inside the parentheses. List elements are checked one by
  one.
- An `ExprError` has a byte span and a typed `ErrorKind` whose message
  says what was found, what is allowed and gives an example. The end of
  the input is placed right after the last token, so an error there
  points at the expression's last line, not at trailing whitespace or a
  final newline.
- `replacement()` gives, where there is one, the text that fixes the
  error when it replaces the span: a suggested name, component, enum value
  or unit, the unit for a bare number, a lowercase keyword, `and` for
  `&&`. Applying it never yields the same error again (property-tested),
  and for the cases listed in the tests yields an expression that
  type-checks.
- `render(source)` adds the source line and a caret; a tab is shown as
  four spaces and a carriage return is left out, so the caret lines up.
- A message from `taktsim-units` is part of the expression error's
  message, not also its `source()`, so it is printed once.
- The model loader (M4) adds the field path.

### 7. Verification

Tests cover every grammar production, type rule and operator, with
property tests for printing and parsing back, for arbitrary input, for
total evaluation and against reference interpreters. ADR-0009 also asks
for fuzzing the parser in the nightly job. That job does not exist yet,
so the fuzz target is **pending**; it should be added with the nightly
fuzzing infrastructure (CLAUDE.md, "Fuzz").

## Consequences

- The lexical rules and the operator table are part of format 1.0, like
  the grammar (ADR-0009).
- M4 must give component ids a form the rule in "Change to ADR-0009"
  accepts, or calls cannot name them.
- Every rule rejected now can be allowed later without breaking a model.
- No new dependency: the module uses `taktsim-units` and `thiserror`.

## Questions for the owner

1. Do you accept the change to ADR-0009's call grammar (component ids
   with hyphens as call arguments)?
2. ADR-0009's context and spec §6 write `wip(buf1) <= 30` without a
   unit. ADR-0009's rules (no implicit conversions; a quantity is a
   number with a unit) make that a type error, which is what the code
   does, with the fix `30 parts`. Should the examples read `30 parts`?
3. ADR-0009 and spec §6 write the decision variable
   `buf1.capacity in [5, 50]`, which means the interval 5 to 50. In a
   condition, `x in [5, 50]` means "5 or 50". One syntax with two
   meanings; it needs a decision before v0.4 (for example `in 5..50`).
4. ADR-0009 rejects `==` on every quantity except time. A count from a
   unit attribute is an exact integer; is `==` on counts meant to be
   rejected too? (The code rejects it, as written.)
5. The observables do not all fit the scalar types: `wip(buf1)` is a
   time-weighted average, not a whole `Count`, and
   `state_shares(cut, weld)` is one share per state and resource. The
   environment lets M8 declare any result type; the types of the
   built-in observables need a decision there.
6. ADR-0009's grammar makes list elements atoms, so a negative element
   needs parentheses: `[(-1)]`. Should list elements be `unary` instead?
