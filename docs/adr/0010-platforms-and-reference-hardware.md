# ADR-0010: Supported platforms and reference hardware

- Status: accepted (2026-10-25); amended 2026-10-25: tier 2 dropped
- Date: 2026-10-25
- Blocks: M0 (CI and wheel matrix), M2 (benchmark numbers), M13 (release
  performance report)
- Spec: §7 Tooling baseline (Linux, macOS, Windows), §9 Performance and
  Model size, §14 reference load ("five-year-old laptop with integrated
  graphics")

## Context

Owner requirement (2026-10-25): the software must run well on ordinary
PCs, not only on fast Apple Silicon Macs. Development happens on an M2 Mac,
and the spec's performance numbers (≥ 1 M events/s/core, 1,000
components in ≤ 2 GB) say nothing about which machine they apply to. If
they are measured on the development machine, they are met on hardware
most factory engineers do not have.

Spec §14 already defines a low-end reference for the viewer: a
five-year-old laptop with integrated graphics. This ADR uses the same
machine class for the engine and settles which platforms are built and
tested.

## Decision drivers

- Typical users work on company-issued Windows laptops, x86-64, often 8 GB
  RAM and locked down (no admin rights, no compilers).
- Deterministic replay must hold across CPU architectures, not only
  across OSes: x86-64 and ARM64 (Apple Silicon, ARM Linux servers,
  Windows on ARM laptops).
- Measurements must come from hardware that represents users.

## Proposal

### Platform tiers

| Tier | Targets | What CI does |
| --- | --- | --- |
| 1 | `x86_64-pc-windows-msvc`, `x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`, `aarch64-apple-darwin` | full test suite, cross-platform golden-trace hash comparison, release binaries and Python wheels |
| — | `x86_64-apple-darwin` (Intel Macs), `aarch64-pc-windows-msvc` (Windows on ARM), musl Linux x86-64 | not supported and not built (amended 2026-10-25); added back as tier 1 when a user needs one |

Minimum OS versions: Windows 10, macOS 11, Linux with glibc ≥ 2.28
(`manylinux_2_28`: RHEL 8, Debian 10, Ubuntu 20.04 and later). 32-bit
targets are not supported.

The deterministic-replay criterion (A4) is checked across all tier-1
targets, which covers both CPU architectures.

### Portable builds

- Shipped binaries and wheels use the Rust default CPU baseline for each
  target. No `-C target-cpu=native` or similar in committed
  configuration.
- SIMD or other CPU-specific code only with runtime feature detection and
  a scalar fallback that gives bit-identical results, tested on both
  paths.
- No `f64::mul_add` in engine or sampling code: on x86-64 CPUs without FMA
  it falls back to slow software, and keeping one arithmetic path keeps
  traces simple to reason about. Rust never fuses multiply and add on its
  own, so ordinary arithmetic is the same on x86-64 and ARM64.
- Install without compilers: `pip install taktsim` uses a prebuilt wheel
  on every tier-1 target; the CLI ships as a single static
  binary per target.

### Reference hardware

- **Reference machine** (performance targets apply here): a mid-range
  laptop from around 2021, x86-64, 4 cores / 8 threads (Intel Core
  i5-1135G7 or AMD Ryzen 5 5500U class), 8 GB RAM, integrated graphics,
  SSD, Windows 11. The same machine serves as the viewer reference from
  §14.
- **Minimum machine** (must run, no performance target): 2 cores, 4 GB
  RAM. The 1,000-component reference model must complete there without
  swapping.
- **Parallel replications** are limited by memory as well as cores: the
  default number of parallel replications is the smaller of the core
  count and what fits in the memory budget (estimated per replication
  from a checked warm-up run), so a run sized on a 32 GB workstation
  does not exhaust an 8 GB laptop.

### Measurement

- Regression gate (ADR-0007): instruction counts on Linux CI, independent
  of the runner's speed.
- Absolute numbers (events/s, peak memory, README example time): until a
  physical reference machine is available, measured on a standard
  GitHub-hosted x86-64 Linux runner (4 vCPU), median of at least 10 runs,
  and reported as "CI proxy". From v0.2 on, the release report also
  includes the physical reference machine, which spec §14 already requires
  for the viewer check.
- Numbers from developer machines (including Apple Silicon) are never
  cited as evidence that a target is met. They may be reported next to the
  reference numbers, labelled.
- README example < 1 s (A8) is measured on the slowest tier-1 CI runner.

## Trade-offs

- A Windows x86-64 reference machine is slower than the development Mac,
  so some targets from spec §9 may need to be revised downward in
  ADR-0007 at v0.2. That is the intended outcome: the numbers then
  describe what users get.
- Four tier-1 targets lengthen CI. ARM64 Linux runners are free for public
  repositories on GitHub; Windows is the slowest runner and sets the CI
  time.
- Someone has to own a physical reference laptop by v0.2.

## Resolved questions

The owner accepted this ADR as proposed on 2026-10-25. The questions as
they were asked:

1. Do the tiers match the machines your users have? In particular: is
   Windows on ARM worth tier 2, and are Intel Macs still relevant?
2. Is the reference machine class right (2021 mid-range x86-64 laptop,
   8 GB, Windows 11), or do your users have older or weaker machines?
3. Do you have, or can you get, such a laptop by v0.2?

## Amendment (2026-10-25)

The owner dropped tier 2. Intel Macs are no longer sold, Windows on ARM is
rare in companies, and musl only matters for very small containers; none of
the expected users needs them yet. The three CI jobs are removed. A target
comes back as tier 1, with a must-pass CI job and wheels, as soon as a user
needs it. Cross-architecture determinism stays covered: x86-64 (Linux,
Windows) against ARM64 (Linux, macOS).
