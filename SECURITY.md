# Security policy

## Reporting a vulnerability

Please report vulnerabilities privately through GitHub:
**Security → Report a vulnerability** on
[taktsim/taktsim](https://github.com/taktsim/taktsim/security/advisories/new).
Do not open a public issue.

You get an answer within 7 days. Fixes are released as soon as they are ready,
and the advisory is published with the fix.

## Scope

taktsim reads models and data; it never writes to machines or controllers.
Things we especially want to hear about:

- a model file or data file that makes the loader or engine crash, hang or use
  unbounded memory;
- any network access from the engine crates, or any way data leaves the
  machine without a deliberate action by a person;
- a model that leads to code execution (models are data, never code).

## Supported versions

Before v1.0, only the latest release gets fixes.
