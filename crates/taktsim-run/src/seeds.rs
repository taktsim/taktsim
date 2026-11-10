//! Which seeds a command runs (ADR-0024 §5, ADR-0025 §4), in the library, so every
//! client gets the same answer. A range's length comes from checked arithmetic on its
//! two ends before anything is expanded, so a huge range is a refusal and never an
//! allocation failure.

use std::fmt;
use std::str::FromStr;

use taktsim_model::{Document, MAX_INTEGER};
use taktsim_stats::ReplicationPlan;

use crate::RunRequest;
use crate::diagnostic::{Diagnostic, Failure, FailureClass};
use crate::experiment::Budget;

/// A list up to this length is its own budget (ADR-0025 §4).
pub const MAX_SHORT_LIST: u64 = 1_000;
/// No list, and no `--max-runs`, goes above this (ADR-0025 §4).
pub const MAX_SEEDS: u64 = 100_000;

/// The largest seed: the range a run id states exactly.
fn max_seed() -> u64 {
    MAX_INTEGER.unsigned_abs()
}

/// What `--seed` says: one seed, or a range with both ends.
///
/// ```
/// use taktsim_run::SeedSpec;
///
/// assert_eq!("42".parse(), Ok(SeedSpec::One(42)));
/// assert_eq!("1..20".parse(), Ok(SeedSpec::Range { first: 1, last: 20 }));
/// assert!("a..b".parse::<SeedSpec>().is_err());
/// assert_eq!(SeedSpec::Range { first: 1, last: 20 }.to_string(), "1..20");
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeedSpec {
    /// One seed.
    One(u64),
    /// Every seed from `first` to `last`, both included.
    Range {
        /// The first.
        first: u64,
        /// The last.
        last: u64,
    },
}

impl fmt::Display for SeedSpec {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::One(seed) => write!(f, "{seed}"),
            Self::Range { first, last } => write!(f, "{first}..{last}"),
        }
    }
}

/// Text that is not a seed or a range of seeds.
///
/// ```
/// use taktsim_run::{ParseSeedError, SeedSpec};
///
/// assert_eq!("x".parse::<SeedSpec>(), Err(ParseSeedError));
/// assert!(ParseSeedError.to_string().contains("1..20"));
/// ```
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("a seed is a whole number, or a range such as `1..20`")]
pub struct ParseSeedError;

impl FromStr for SeedSpec {
    type Err = ParseSeedError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        match text.split_once("..") {
            Some((first, last)) => Ok(Self::Range {
                first: first.parse().map_err(|_| ParseSeedError)?,
                last: last.parse().map_err(|_| ParseSeedError)?,
            }),
            None => text.parse().map(Self::One).map_err(|_| ParseSeedError),
        }
    }
}

fn invalid(line: Diagnostic) -> Failure {
    Failure::one(FailureClass::Invalid, line)
}

fn out_of_range(seed: u64) -> Failure {
    let max = max_seed();
    invalid(
        Diagnostic::error(
            "run.seed_out_of_range",
            format!("the seed {seed} is above 2^53 - 1, the range a run id states exactly"),
        )
        .at("--seed")
        .found(seed.to_string())
        .allowed(format!("0 to {max}"))
        .example("42"),
    )
}

/// The error line of `run.no_budget` for one missing flag.
fn no_budget(flag: &str, why: &str) -> Diagnostic {
    Diagnostic::error("run.no_budget", format!("{why}, which needs {flag}"))
        .at(flag)
        .found("nothing")
        .allowed(format!("{flag} with a whole number from 1"))
        .example(format!("{flag} 100"))
}

/// What `budget` lacks for `why`: one line per missing flag.
fn missing_flags(budget: Budget, why: &str) -> Vec<Diagnostic> {
    let runs = budget
        .max_runs
        .is_none()
        .then(|| no_budget("--max-runs", why));
    let seconds = budget
        .max_seconds
        .is_none()
        .then(|| no_budget("--max-seconds", why));
    runs.into_iter().chain(seconds).collect()
}

/// A list's length against its limits: above 100,000 no budget helps; above 1,000 it
/// needs both flags; and a `--max-runs` below it would be a silent cut.
fn check_list_length(length: u64, budget: Budget) -> Result<(), Failure> {
    if length > MAX_SEEDS {
        return Err(invalid(
            Diagnostic::error(
                "run.budget_too_large",
                format!("{length} seeds are more than an experiment takes"),
            )
            .at("--seed")
            .found(length.to_string())
            .allowed(format!("at most {MAX_SEEDS} seeds"))
            .example("1..20"),
        ));
    }
    if length > MAX_SHORT_LIST {
        let lines = missing_flags(budget, &format!("a list of {length} seeds is long"));
        if !lines.is_empty() {
            return Err(Failure::new(FailureClass::Invalid, lines));
        }
    }
    if let Some(max_runs) = budget.max_runs
        && u64::from(max_runs.get()) < length
    {
        return Err(invalid(
            Diagnostic::error(
                "run.budget_too_small",
                format!(
                    "--max-runs {max_runs} is below the {length} seeds asked for; a list is not cut silently"
                ),
            )
            .at("--max-runs")
            .found(max_runs.to_string())
            .allowed(format!("{length} or more"))
            .example(length.to_string()),
        ));
    }
    Ok(())
}

/// `--max-runs` above 100,000 is refused whatever else is asked.
fn check_max_runs(budget: Budget) -> Result<(), Failure> {
    match budget.max_runs {
        Some(max_runs) if u64::from(max_runs.get()) > MAX_SEEDS => Err(invalid(
            Diagnostic::error(
                "run.budget_too_large",
                format!("--max-runs {max_runs} is more than an experiment takes"),
            )
            .at("--max-runs")
            .found(max_runs.to_string())
            .allowed(format!("at most {MAX_SEEDS}"))
            .example("100"),
        )),
        Some(_) | None => Ok(()),
    }
}

/// How many seeds `spec` names, once it is checked: its length comes from its two
/// ends, never from expanding it.
fn spec_length(spec: SeedSpec) -> Result<u64, Failure> {
    let (first, last) = match spec {
        SeedSpec::One(seed) => (seed, seed),
        SeedSpec::Range { first, last } => (first, last),
    };
    if last > max_seed() {
        return Err(out_of_range(last));
    }
    if first > last {
        return Err(invalid(
            Diagnostic::error(
                "run.seed_out_of_range",
                format!("the range {spec} runs backward"),
            )
            .at("--seed")
            .found(spec.to_string())
            .allowed("a range whose first seed is not above its last")
            .example("1..20"),
        ));
    }
    Ok(last - first + 1)
}

/// The seeds a request names, each checked and all counted before anything is
/// expanded, so what they ask for is decided once the model is known (ADR-0025 §4).
struct Asked<'a> {
    seeds: &'a [u64],
    specs: &'a [SeedSpec],
    /// How many seeds, saturating: at most 2^53 per range, so the sum of up to 2^10 of
    /// them cannot overflow, and the saturating form states it even for more.
    length: u64,
}

impl<'a> Asked<'a> {
    fn of(seeds: &'a [u64], specs: &'a [SeedSpec]) -> Result<Self, Failure> {
        if let Some(seed) = seeds.iter().copied().find(|seed| *seed > max_seed()) {
            return Err(out_of_range(seed));
        }
        let mut length = u64::try_from(seeds.len()).unwrap_or(u64::MAX);
        for spec in specs {
            length = length.saturating_add(spec_length(*spec)?);
        }
        Ok(Self {
            seeds,
            specs,
            length,
        })
    }

    /// The seed, if exactly one is named.
    fn single(&self) -> Option<u64> {
        if self.length != 1 {
            return None;
        }
        self.seeds
            .first()
            .copied()
            .or_else(|| match self.specs.first() {
                Some(SeedSpec::One(seed) | SeedSpec::Range { first: seed, .. }) => Some(*seed),
                None => None,
            })
    }

    /// Every seed, sorted, once [`check_list_length`] has bounded how many; a seed
    /// named twice is refused.
    fn expand(&self) -> Result<Vec<u64>, Failure> {
        let mut seeds = self.seeds.to_vec();
        for spec in self.specs {
            match *spec {
                SeedSpec::One(seed) => seeds.push(seed),
                SeedSpec::Range { first, last } => seeds.extend(first..=last),
            }
        }
        check_sorted_unique(&mut seeds)?;
        Ok(seeds)
    }
}

/// The seeds `specs` name, sorted, as a list of seeds: every length is checked before
/// anything is expanded. [`run`](crate::run) does these checks itself once it knows the
/// model, on [`RunRequest::seed_specs`], since a model that draws nothing is one run
/// whatever the list, and a precision target refuses any list (ADR-0025 §4); a client
/// passes what `--seed` says there rather than expanding it here first.
///
/// # Errors
///
/// A [`Failure`] of class `Invalid`: `run.seed_out_of_range` above 2^53 − 1 or for a
/// range that runs backward, `run.budget_too_large` above 100,000 seeds,
/// `run.no_budget` for a long list without both budgets, `run.budget_too_small` for
/// `--max-runs` below the list, `run.duplicate_seed` for a seed named twice.
///
/// ```
/// use taktsim_run::{Budget, SeedSpec, expand_seeds};
///
/// let specs = [SeedSpec::Range { first: 3, last: 5 }, SeedSpec::One(1)];
/// assert_eq!(expand_seeds(&specs, Budget::none())?, [1, 3, 4, 5]);
///
/// // Refused before any allocation.
/// let huge = [SeedSpec::Range { first: 0, last: 9007199254740991 }];
/// let refused = expand_seeds(&huge, Budget::none()).unwrap_err();
/// assert_eq!(refused.diagnostics[0].type_name, "run.budget_too_large");
/// # Ok::<(), taktsim_run::Failure>(())
/// ```
pub fn expand_seeds(specs: &[SeedSpec], budget: Budget) -> Result<Vec<u64>, Failure> {
    let asked = Asked::of(&[], specs)?;
    check_max_runs(budget)?;
    check_list_length(asked.length, budget)?;
    asked.expand()
}

/// Sorts `seeds`, and refuses a duplicate. The experiment id is over sorted run ids, and
/// a mean depends in its last bits on push order, so two orders of one set must not
/// give two documents (ADR-0025 §4).
fn check_sorted_unique(seeds: &mut [u64]) -> Result<(), Failure> {
    seeds.sort_unstable();
    match seeds.windows(2).find(|pair| pair[0] == pair[1]) {
        Some(pair) => Err(invalid(
            Diagnostic::error(
                "run.duplicate_seed",
                format!("the seed {} is named twice", pair[0]),
            )
            .at("--seed")
            .found(pair[0].to_string())
            .allowed("each seed once")
            .example("--seed 1 --seed 2"),
        )),
        None => Ok(()),
    }
}

/// The seed a run uses, and what the caller should know about the others.
#[derive(Debug)]
pub(crate) struct Seeding {
    /// `None` for a model that draws nothing.
    pub(crate) seed: Option<u64>,
    pub(crate) warnings: Vec<Diagnostic>,
}

/// What a command runs.
#[derive(Debug)]
pub(crate) enum Mode {
    /// One run, M7's document.
    One(Seeding),
    /// A list of seeds.
    List { seeds: Vec<u64> },
    /// `first, first + 1, …`, until the targets are reached or `max_runs` have run.
    Sequence { first: u64, max_runs: u32 },
}

/// A warning that the flag is not used here.
fn flag_unused(flag: &str, found: String, why: &str) -> Diagnostic {
    Diagnostic::warning(
        "run.flag_unused",
        format!("{why}, so {flag} {found} is not used"),
    )
    .at(flag)
    .found(found)
}

/// What the budget flags and the parallel flags say that is unused by one run.
fn unused_flags(request: &RunRequest<'_>, why: &str) -> Vec<Diagnostic> {
    let budget = request.budget;
    let text = |value: Option<String>, flag: &str| value.map(|found| flag_unused(flag, found, why));
    [
        text(budget.max_runs.map(|n| n.to_string()), "--max-runs"),
        text(budget.max_seconds.map(|n| n.to_string()), "--max-seconds"),
        text(request.threads.map(|n| n.to_string()), "--threads"),
        text(
            request.max_memory_mib.map(|n| n.to_string()),
            "--max-memory-mib",
        ),
    ]
    .into_iter()
    .flatten()
    .collect()
}

/// The table of ADR-0025 §4. Every check of the seeds named comes after the model is
/// known, so each client gets this answer for the same request.
pub(crate) fn plan(
    draws: bool,
    request: &RunRequest<'_>,
    document: &Document,
) -> Result<Mode, Failure> {
    let budget = request.budget;
    let model_seed = document.model().seed();
    let asked = Asked::of(request.seeds, request.seed_specs)?;
    check_max_runs(budget)?;
    if !draws {
        return Ok(Mode::One(no_draw(request, &asked, model_seed)?));
    }
    if !document.experiment().precision().is_empty() {
        return sequence(request, &asked, model_seed)
            .map(|(first, max_runs)| Mode::Sequence { first, max_runs });
    }
    if asked.length >= 2 {
        check_list_length(asked.length, budget)?;
        return Ok(Mode::List {
            seeds: asked.expand()?,
        });
    }
    let mut seeding = effective(true, asked.single(), model_seed)?;
    seeding
        .warnings
        .extend(unused_flags(request, "one seed is one run"));
    Ok(Mode::One(seeding))
}

/// A model that draws nothing is one run, whatever the flags ask for.
fn no_draw(
    request: &RunRequest<'_>,
    asked: &Asked<'_>,
    model_seed: Option<u64>,
) -> Result<Seeding, Failure> {
    let mut seeding = effective(false, asked.single(), model_seed)?;
    if asked.length >= 2 {
        seeding.warnings.insert(
            0,
            Diagnostic::warning(
                "run.seed_unused",
                format!(
                    "this model draws nothing, so the {} seeds from --seed are not used",
                    asked.length
                ),
            )
            .at("--seed")
            .found(format!("{} seeds", asked.length)),
        );
    }
    seeding
        .warnings
        .extend(unused_flags(request, "this model draws nothing"));
    Ok(seeding)
}

/// A sequential experiment: its first seed and how many at most.
fn sequence(
    request: &RunRequest<'_>,
    asked: &Asked<'_>,
    model_seed: Option<u64>,
) -> Result<(u64, u32), Failure> {
    if asked.length >= 2 {
        return Err(invalid(
            Diagnostic::error(
                "run.seed_list_with_precision",
                "a list of seeds and `experiment.precision` ask for two different things",
            )
            .at("--seed")
            .found(format!("{} seeds", asked.length))
            .allowed("one first seed with `experiment.precision`, or a list without it")
            .example("--seed 1"),
        ));
    }
    let budget = request.budget;
    let lines = missing_flags(
        budget,
        "`experiment.precision` runs until a target is reached",
    );
    let (Some(max_runs), true) = (budget.max_runs, lines.is_empty()) else {
        return Err(Failure::new(FailureClass::Invalid, lines));
    };
    let least = ReplicationPlan::DEFAULT_MIN_REPLICATIONS;
    if max_runs.get() < least {
        return Err(invalid(
            Diagnostic::error(
                "run.budget_too_small",
                format!(
                    "--max-runs {max_runs} is below the {least} replications a precision target needs first"
                ),
            )
            .at("--max-runs")
            .found(max_runs.to_string())
            .allowed(format!("{least} or more"))
            .example("100"),
        ));
    }
    let Some(first) = asked.single().or(model_seed) else {
        return Err(no_seed());
    };
    let final_seed = first.saturating_add(u64::from(max_runs.get()) - 1);
    if final_seed > max_seed() {
        return Err(out_of_range(final_seed));
    }
    Ok((first, max_runs.get()))
}

/// A model that draws, and no seed: refused, never entropy.
fn no_seed() -> Failure {
    invalid(
        Diagnostic::error(
            "run.no_seed",
            "this model draws random numbers and no seed is given; a run never takes one from entropy",
        )
        .at("model.seed")
        .found("no seed")
        .allowed(format!("`--seed` or `model.seed`, from 0 to {}", max_seed()))
        .example("--seed 42"),
    )
}

/// The rule: a model that draws nothing has no seed; otherwise the option, then
/// the model; otherwise refused. Never entropy and never an unstated default.
pub(crate) fn effective(
    draws: bool,
    option: Option<u64>,
    model: Option<u64>,
) -> Result<Seeding, Failure> {
    let max = max_seed();
    if let Some(seed) = option
        && seed > max
    {
        return Err(out_of_range(seed));
    }
    if draws {
        return match option.or(model) {
            Some(seed) => Ok(Seeding {
                seed: Some(seed),
                warnings: Vec::new(),
            }),
            None => Err(no_seed()),
        };
    }
    let unused = |seed: u64, path: &str| {
        Diagnostic::warning(
            "run.seed_unused",
            format!("this model draws nothing, so the seed {seed} from {path} is not used"),
        )
        .at(path)
        .found(seed.to_string())
    };
    let warnings = option
        .map(|seed| unused(seed, "--seed"))
        .into_iter()
        .chain(model.map(|seed| unused(seed, "model.seed")))
        .collect();
    Ok(Seeding {
        seed: None,
        warnings,
    })
}

#[cfg(test)]
mod tests {
    use std::num::{NonZeroU32, NonZeroU64};

    use super::*;

    #[test]
    fn the_rule_of_adr_0024() {
        assert_eq!(effective(true, Some(7), Some(1)).unwrap().seed, Some(7));
        assert_eq!(effective(true, None, Some(1)).unwrap().seed, Some(1));
        let none = effective(true, None, None).unwrap_err();
        assert_eq!(none.class, FailureClass::Invalid);
        assert_eq!(none.diagnostics[0].path.as_deref(), Some("model.seed"));
        assert_eq!(none.diagnostics[0].example.as_deref(), Some("--seed 42"));

        let exact = effective(false, Some(7), Some(1)).unwrap();
        assert_eq!(exact.seed, None);
        let paths: Vec<_> = exact.warnings.iter().map(|w| w.path.as_deref()).collect();
        assert_eq!(paths, [Some("--seed"), Some("model.seed")]);
        assert_eq!(effective(false, None, None).unwrap().warnings.len(), 0);
    }

    #[test]
    fn a_seed_beyond_the_exact_range_is_refused_with_or_without_draws() {
        let max = MAX_INTEGER.unsigned_abs();
        assert_eq!(effective(true, Some(max), None).unwrap().seed, Some(max));
        for draws in [true, false] {
            let e = effective(draws, Some(max + 1), None).unwrap_err();
            assert_eq!(e.diagnostics[0].type_name, "run.seed_out_of_range");
        }
    }

    fn budget(max_runs: Option<u32>, max_seconds: Option<u64>) -> Budget {
        Budget {
            max_runs: max_runs.and_then(NonZeroU32::new),
            max_seconds: max_seconds.and_then(NonZeroU64::new),
        }
    }

    #[test]
    fn specs_parse_and_print() {
        assert_eq!("0".parse(), Ok(SeedSpec::One(0)));
        assert_eq!("5..5".parse(), Ok(SeedSpec::Range { first: 5, last: 5 }));
        for bad in ["", "x", "1..", "..2", "-1", "1...2", "1..2..3"] {
            assert_eq!(bad.parse::<SeedSpec>(), Err(ParseSeedError), "{bad}");
        }
        assert_eq!(SeedSpec::One(3).to_string(), "3");
    }

    /// The length comes from the two ends: this returns at once, with no allocation of
    /// 2^53 seeds.
    #[test]
    fn huge_range_is_refused_before_allocating() {
        let max = max_seed();
        let huge = [SeedSpec::Range {
            first: 0,
            last: max,
        }];
        let e = expand_seeds(&huge, budget(Some(100_000), Some(1))).unwrap_err();
        assert_eq!(e.diagnostics[0].type_name, "run.budget_too_large");
        assert_eq!(e.diagnostics[0].path.as_deref(), Some("--seed"));
        assert_eq!(e.class, FailureClass::Invalid);

        // Above the bound, whole ranges and single seeds alike.
        let above = [SeedSpec::Range {
            first: 0,
            last: u64::MAX,
        }];
        let e = expand_seeds(&above, Budget::none()).unwrap_err();
        assert_eq!(e.diagnostics[0].type_name, "run.seed_out_of_range");
        let e = expand_seeds(&[SeedSpec::One(max + 1)], Budget::none()).unwrap_err();
        assert_eq!(e.diagnostics[0].type_name, "run.seed_out_of_range");

        // Many ranges together count too.
        let many = vec![
            SeedSpec::Range {
                first: 0,
                last: 99_999
            };
            3
        ];
        let e = expand_seeds(&many, Budget::none()).unwrap_err();
        assert_eq!(e.diagnostics[0].type_name, "run.budget_too_large");
    }

    #[test]
    fn long_list_needs_both_budgets() {
        let long = [SeedSpec::Range {
            first: 1,
            last: 1_001,
        }];
        let e = expand_seeds(&long, Budget::none()).unwrap_err();
        let types: Vec<_> = e.diagnostics.iter().map(|d| d.type_name.as_str()).collect();
        assert_eq!(types, ["run.no_budget", "run.no_budget"]);
        let paths: Vec<_> = e.diagnostics.iter().map(|d| d.path.as_deref()).collect();
        assert_eq!(paths, [Some("--max-runs"), Some("--max-seconds")]);
        let e = expand_seeds(&long, budget(Some(2_000), None)).unwrap_err();
        assert_eq!(e.diagnostics.len(), 1);
        assert_eq!(e.diagnostics[0].path.as_deref(), Some("--max-seconds"));
        let e = expand_seeds(&long, budget(None, Some(60))).unwrap_err();
        assert_eq!(e.diagnostics[0].path.as_deref(), Some("--max-runs"));
        assert_eq!(
            expand_seeds(&long, budget(Some(1_001), Some(60)))
                .unwrap()
                .len(),
            1_001
        );

        // 1,000 is short; 100,000 is the last that any budget allows.
        let short = [SeedSpec::Range {
            first: 1,
            last: 1_000,
        }];
        assert_eq!(expand_seeds(&short, Budget::none()).unwrap().len(), 1_000);
        let top = [SeedSpec::Range {
            first: 1,
            last: 100_000,
        }];
        assert_eq!(
            expand_seeds(&top, budget(Some(100_000), Some(60)))
                .unwrap()
                .len(),
            100_000
        );
        let over = [SeedSpec::Range {
            first: 1,
            last: 100_001,
        }];
        let e = expand_seeds(&over, budget(Some(100_000), Some(60))).unwrap_err();
        assert_eq!(e.diagnostics[0].type_name, "run.budget_too_large");
    }

    #[test]
    fn max_runs_below_list_is_refused() {
        let list = [SeedSpec::Range { first: 1, last: 20 }];
        let e = expand_seeds(&list, budget(Some(19), None)).unwrap_err();
        assert_eq!(e.diagnostics[0].type_name, "run.budget_too_small");
        assert_eq!(e.diagnostics[0].path.as_deref(), Some("--max-runs"));
        assert_eq!(e.diagnostics[0].allowed.as_deref(), Some("20 or more"));
        assert_eq!(
            expand_seeds(&list, budget(Some(20), None)).unwrap().len(),
            20
        );
        let e = expand_seeds(&list, budget(Some(100_001), None)).unwrap_err();
        assert_eq!(e.diagnostics[0].type_name, "run.budget_too_large");
        assert_eq!(e.diagnostics[0].path.as_deref(), Some("--max-runs"));
    }

    #[test]
    fn seed_order_is_sorted() {
        let specs = [SeedSpec::One(9), SeedSpec::Range { first: 2, last: 3 }];
        assert_eq!(expand_seeds(&specs, Budget::none()).unwrap(), [2, 3, 9]);
    }

    #[test]
    fn duplicate_seed_is_refused() {
        let e = expand_seeds(&[SeedSpec::One(4), SeedSpec::One(4)], Budget::none()).unwrap_err();
        assert_eq!(e.diagnostics[0].type_name, "run.duplicate_seed");
        assert_eq!(e.diagnostics[0].found.as_deref(), Some("4"));
        let e = expand_seeds(
            &[SeedSpec::Range { first: 1, last: 5 }, SeedSpec::One(3)],
            Budget::none(),
        )
        .unwrap_err();
        assert_eq!(e.diagnostics[0].type_name, "run.duplicate_seed");
    }

    #[test]
    fn a_range_that_runs_backward_is_refused() {
        let e = expand_seeds(&[SeedSpec::Range { first: 5, last: 1 }], Budget::none()).unwrap_err();
        assert_eq!(e.diagnostics[0].type_name, "run.seed_out_of_range");
        assert_eq!(e.diagnostics[0].found.as_deref(), Some("5..1"));
    }
}
