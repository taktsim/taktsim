# ADR-0019: The CLI is the agent interface; no MCP server

- Status: proposed (needs the owner's explicit acceptance: it changes the
  spec and amends ADR-0003, ADR-0004 and ADR-0005)
- Date: 2026-10-26
- Blocks: M7 (first commands under the contract), v0.2 Agent tier
- Spec: §1, §3, §5 (rewritten), §7, §9, §10, §11, §12, §13, §14, §15;
  §2 principles 1 and 2; issue #14

## Context

The spec gives agents an MCP server whose tools mirror the public Rust API
(§5), next to a CLI for people (§10) and the Python bindings (ADR-0005).
The owner decided on 2026-10-26 (issue #14) to drop the MCP server: agents
use the same `taktsim` CLI that people use. That decision is made. This
ADR records it and proposes the contract the CLI must meet so that an agent
that knows only the binary's name can find every command, its inputs,
outputs and errors, and steer the whole system without reading the docs.

Why:

- Principle 1 ("anything a person can do, an agent can do through the same
  API, and the other way round") holds most directly when both use the
  same interface.
- One client fewer: no `rmcp` dependency, no tool contracts kept in sync
  with the CLI, less async-runtime surface. The Python bindings
  (ADR-0005) stay.
- Agents with a shell already work best with a CLI that describes itself.

What we lose: agents without a shell (desktop and web chat hosts), and
MCP's narrow tool surface, which kept an agent away from anything not
exposed as a tool. Anyone can still wrap the CLI in an MCP server from its
self-description, outside this project. The sharing safeguard (item 8)
has to make up for the narrow surface.

## Options

- **A. MCP server and CLI** (the spec until now). Two agent-facing
  contracts to keep in sync, an async runtime in a client crate.
- **B. CLI only, self-describing** (decided). One contract, used by people,
  scripts, CI and agents.
- **C. CLI only, plus an MCP shim generated from the CLI's
  self-description.** Brings back shell-less agents at the cost of a
  second client to test and release. Not now; possible later as a separate
  ADR, or by a third party, because the self-description makes it
  mechanical.

## Trade-offs

| | A | B | C |
| --- | --- | --- | --- |
| Contracts to keep in sync | 2 | 1 | 1 + a generated shim |
| Shell-less agents | yes | no | yes |
| Async runtime in a client | yes (`tokio` via `rmcp`) | no | yes |
| Agent and person use the same path | no | yes | partly |
| Accidental sharing prevented by | the tool surface | item 8 below | item 8 below |

## Decision

The MCP server and the crate `taktsim-mcp` are dropped. The CLI
`taktsim` is the agent interface. Its contract:

### 1. Self-description

`taktsim describe` prints one JSON document that lists every command and
subcommand with:

- its purpose (one sentence) and whether it reads only, or writes (and
  what: a run in the run store, a new model file, a receipt);
- its arguments and flags, with types, defaults, allowed values and
  whether each is required;
- the JSON Schema of each input document (model, observations, factor
  grid) by reference into `taktsim schema`, and the JSON Schema of its
  result;
- its exit codes (item 3) and the error type of each;
- whether it is long-running (needs a budget, item 4) or needs a person
  (item 8);
- one valid example invocation.

The document carries the package version and its own schema. It is
generated from the same Rust types as the CLI: the argument parser
(`clap` with derive; this ADR accepts it as a CLI dependency) and the
result types (`schemars`, already the schema generator from M4). It is
committed as `schemas/cli/describe.json` and checked for freshness in CI
like the other schemas. A CLI reference in the docs is generated from it.

`taktsim schema <type | all>` prints the JSON Schema of a model type or
component type (the former `schema.get`). `taktsim <command> --help`
stays, for people, rendered from the same parser.

### 2. Machine-readable output

Every command takes `--format text | json`. With `json`, stdout carries
exactly one JSON document of the command's published result type, never
free text; the `text` rendering is produced from the same typed value.
`describe` and `schema` always print JSON. Stdout holds only the result;
everything else goes to stderr.

### 3. Typed errors and stable exit codes

With `--format json`, stderr is JSON Lines: one object per line with a
`kind` (`error`, `warning`, `progress`). An error carries the field path,
what was found, what is allowed and one valid example (§5, CLAUDE.md),
and the error's type name. Argument errors from the parser go through the
same type, so a misspelt flag is also a path-level JSON error.

Exit codes are part of the contract:

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | invalid input: arguments, model, data, unknown run id; nothing ran |
| 2 | run failed: invariant or conservation violation, engine error (ADR-0018) |
| 3 | budget exhausted: partial result printed, with the stop reason |
| 4 | interrupted (SIGINT, SIGTERM, Ctrl+C, Ctrl+Break): partial result printed, with the stop reason |
| 5 | check not passed: `compare` thresholds missed, certificate invalid; the result itself is complete and valid |
| 6 | a person's confirmation is required (item 8) |
| 7 | I/O failure: a file or the run store cannot be read or written |
| 101 | internal bug: the single invariant-violation path of ADR-0007 (Rust's panic exit code) |

Changing the meaning of a code, removing a command or flag, or removing or
renaming a result field is a breaking change of the CLI (semver).

### 4. Non-interactive, bounded

No prompts, except the share confirmation (item 8). Long-running commands
(`calibrate`, `optimize`, `experiment`, and `run` with replication
control) require a budget, wall-clock seconds and maximum simulated runs;
without one they exit 1 and name the missing flag. They report progress
as `progress` lines on stderr. On SIGINT or SIGTERM (Ctrl+C or
Ctrl+Break on Windows) they stop at the next replication boundary and
print the partial result with its stop reason (exit 4). The engine stays
synchronous; the stop reaches it through the cancellation hook of
`RunLimits` (ADR-0018).

### 5. Run store and ids

Runs are kept in a content-addressed store. The run id is the hash of the
canonical model (ADR-0002), seed, horizon and engine version (§5), so
running the same thing twice returns the stored run instead of running
again. A run with several seeds returns one run id per replication and an
experiment id over them. Later commands (`trace query`, `compare`,
`explain`) take an id.

The store is the directory `.taktsim/` in the nearest ancestor of the
current directory that has one, like `.git`; if none has, `.taktsim/` in
the current directory, created on the first write. `--store <dir>`
overrides it. Traces stream into the store (ADR-0004), never into memory.
Nothing is evicted automatically; listing and removing runs are commands
(v0.2), so deleting is always an explicit write.

### 6. Paged trace queries

`taktsim trace query <run-id> --filter <expr> --limit <n> --cursor <c>`
returns at most `n` events and a cursor for the next page, never the
whole trace. The filter uses the expression language (ADR-0009,
ADR-0015).

### 7. The former MCP tools

| MCP tool (spec §5 before) | CLI |
| --- | --- |
| `schema.get` | `taktsim schema <type \| all>` |
| `model.validate` | `taktsim validate <model>` |
| `model.render` | `taktsim render <model>` |
| `run` | `taktsim run <model>` |
| `trace.query` | `taktsim trace query <run-id>` |
| `compare` | `taktsim compare <run-id> <observations>` |
| `calibrate` | `taktsim calibrate <model> <observations>` |
| `optimize` | `taktsim optimize <model>` |
| `experiment` | `taktsim experiment <model> <grid>` |
| `explain` | `taktsim explain <run-id> <metric>` |
| `view.focus` | `taktsim view focus <component> --at <time>`: talks to a running `taktsim view` on the local machine only |
| `share.preview` | `taktsim share --preview <model>` |

### 8. Sharing needs a person

Spec §12 says an agent can never share data on its own. That relied on
"sending needs a human in the CLI", which no longer separates the two.
Sending therefore needs a confirmation that an agent driving the CLI
through ordinary command execution cannot give:

- `taktsim share --send` works only on an interactive terminal: it opens
  the controlling terminal itself (`/dev/tty`, `CONIN$` on Windows), shows
  the payload hash and a one-time code, and sends only after the person
  types that code there. Without a controlling terminal it exits 6
  without sending.
- There is no flag, environment variable or configuration to skip it.
- `describe` marks `share --send` as needing a person, so an agent knows
  to hand the step over instead of trying.

**Limit, stated plainly:** an agent that runs as the same user with a
pseudo-terminal (for example through `expect` or `tmux`) can read and type
the code. No local check can stop a process that has all the user's
rights. A confirmation an agent cannot produce has to come from outside
the machine: the library service keeps a sent payload pending until the
contributor approves it in the service, signed in as themselves. That
belongs to the parameter-library ADR (v0.3); question 5 asks whether to
require it.

### 9. Contracts from spec §5, unchanged

Idempotent reads, explicit writes (`calibrate` and `optimize` write new
model files and never overwrite their input), the uncertainty with every
number (confidence interval, replications, seed list), actionable errors,
mandatory budgets.

## Consequences

- Spec: §1 (agents use the CLI), §3 (thin layers: Python bindings and
  CLI), §5 rewritten as "Agent interface: the CLI", §7 (schema contract
  applies to the CLI), §9 (CLI reference generated from `describe`; the
  agent tutorial from the CLI alone), §10 (no `simcode-mcp`), §11 (v0.2
  tier and its criteria), §12 (sharing confirmation), §13 (open-source
  list), §14 (`view focus`), §15 (async boundary for adapters and the
  viewer only). Changed in the same pull request as this ADR.
- Roadmap: M7 builds `describe`, `schema`, `--format json`, typed errors
  and the exit codes for `validate` and `run`, so v0.1 is already usable
  by an agent; the v0.2 "Agent" tier is the rest of the command table.
- ADR-0003, ADR-0004, ADR-0005: their mentions of the MCP server or its
  tool reference now mean the CLI and its reference; their decisions are
  unchanged. Each carries an "Amended by ADR-0019" note.
- New CLI dependencies in M7: `clap` (derive), and for the tests
  `assert_cmd` or plain `std::process::Command`; named in the M7 pull
  request. `rmcp` is never added. Signal handling for item 4 (for
  example `ctrlc`) comes with the first long-running command.
- The async boundary (§15, v0.2 ADR) now concerns only the network
  adapters and the viewer's local server.
- Shell-less agents are not supported (option C stays possible).

## Questions for the owner

1. **Name of the self-description command:** `taktsim describe`
   (recommended), or `taktsim schema cli`, or a `--describe` flag?
2. **JSON as default when stdout is not a terminal?** Recommended: no.
   The format is chosen only by `--format`, so the same command gives the
   same output in a terminal, a pipe, CI and a snapshot test, and an agent
   learns the flag from `describe` (which always prints JSON).
3. **Exit codes:** the table in item 3, as proposed?
4. **Run-store location:** the nearest `.taktsim/` upward from the
   current directory, else the current directory, with `--store` to
   override (recommended); or a per-user cache directory?
5. **Sharing confirmation:** the terminal code of item 8 in v0.3
   (recommended), and should the parameter-library ADR also require
   approval at the service, which is the only confirmation an agent with
   the user's rights cannot give? Recommended: yes.
6. **The v0.2 acceptance test** "an agent completes the tutorial from the
   CLI alone" needs a language model, which is neither deterministic nor
   offline. Recommended: run it as a nightly job against a fixed model
   version with a budget, recording the transcript, and gate each pull
   request on a scripted walk-through instead, which uses only commands
   and flags it found in `describe`'s output.
