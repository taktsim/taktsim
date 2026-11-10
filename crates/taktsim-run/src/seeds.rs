//! Which seed a run uses (ADR-0024 §5), in the library, so every client gets the
//! same answer.

use taktsim_model::MAX_INTEGER;

use crate::diagnostic::{Diagnostic, Failure, FailureClass};

/// The seed a run uses, and what the caller should know about the others.
#[derive(Debug)]
pub(crate) struct Seeding {
    /// `None` for a model that draws nothing.
    pub(crate) seed: Option<u64>,
    pub(crate) warnings: Vec<Diagnostic>,
}

/// The rule: a model that draws nothing has no seed; otherwise the option, then
/// the model; otherwise refused. Never entropy and never an unstated default.
pub(crate) fn effective(
    draws: bool,
    option: Option<u64>,
    model: Option<u64>,
) -> Result<Seeding, Failure> {
    let max = MAX_INTEGER.unsigned_abs();
    if let Some(seed) = option
        && seed > max
    {
        return Err(Failure::one(
            FailureClass::Invalid,
            Diagnostic::error(
                "run.seed_out_of_range",
                format!("the seed {seed} is above 2^53 - 1, the range a run id states exactly"),
            )
            .at("--seed")
            .found(seed.to_string())
            .allowed(format!("0 to {max}"))
            .example("42"),
        ));
    }
    if draws {
        return match option.or(model) {
            Some(seed) => Ok(Seeding {
                seed: Some(seed),
                warnings: Vec::new(),
            }),
            None => Err(Failure::one(
                FailureClass::Invalid,
                Diagnostic::error(
                    "run.no_seed",
                    "this model draws random numbers and no seed is given; a run never takes one from entropy",
                )
                .at("model.seed")
                .found("no seed")
                .allowed(format!("`--seed` or `model.seed`, from 0 to {max}"))
                .example("--seed 42"),
            )),
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
}
