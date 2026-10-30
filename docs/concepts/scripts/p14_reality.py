"""Page 14: from measured data to a validated model (with made-up data)."""

import math

from common import CUT, IMG, SHIFT, SIGMA, TABLES, table, write
from linesim import Dist, Line, Station, Stream, estimate, replicate

TRUE_WELD = Dist.lognormal(45.0, 0.6)  # the "real" welding station, unknown to the modeller
SAMPLES = 300
DAYS = 20  # observed shifts
REPS = 30


def line(weld: Dist) -> Line:
    return Line((Station("cut", Dist.lognormal(CUT, SIGMA)), Station("weld", weld)), (5,))


def ks(xs: list[float], cdf) -> float:
    """Kolmogorov–Smirnov distance between a sample and a distribution."""
    xs = sorted(xs)
    n = len(xs)
    return max(max(abs((i + 1) / n - cdf(x)), abs(i / n - cdf(x))) for i, x in enumerate(xs))


def build() -> None:
    rng = Stream(2026, 1)
    observed = [TRUE_WELD.sample(rng) for _ in range(SAMPLES)]
    m = sum(observed) / SAMPLES
    sd = math.sqrt(sum((x - m) ** 2 for x in observed) / (SAMPLES - 1))
    cv = sd / m
    sigma = math.sqrt(math.log(1 + cv * cv))
    mu = math.log(m) - sigma * sigma / 2

    def ln_cdf(x: float) -> float:
        return 0.5 * (1 + math.erf((math.log(x) - mu) / (sigma * math.sqrt(2))))

    def exp_cdf(x: float) -> float:
        return 1 - math.exp(-x / m)

    def const_cdf(x: float) -> float:
        return 1.0 if x >= m else 0.0

    candidates = [
        ("Constant (mean only)", Dist.const(m), const_cdf, f"{m:.1f} s"),
        ("Exponential", Dist.exp(m), exp_cdf, f"mean {m:.1f} s (CV 1)"),
        ("Lognormal", Dist.lognormal(m, sigma), ln_cdf, f"mean {m:.1f} s, sigma {sigma:.2f} (CV {cv:.2f})"),
    ]
    # The "real" line, observed for 20 shifts.
    real = estimate([float(r.completed) for r in replicate(line(TRUE_WELD), SHIFT, DAYS, seed0=5000)])
    rows = [["Observed on the line", "–", "–", real.fmt(1), "–"]]
    for name, d, cdf, params in candidates:
        e = estimate([float(r.completed) for r in replicate(line(d), SHIFT, REPS)])
        gap = e.mean - real.mean
        verdict = "agrees" if abs(gap) <= math.hypot(e.half, real.half) else "differs"
        rows.append([name, params, f"{ks(observed, cdf):.2f}", e.fmt(1), f"{gap:+.1f} ({verdict})"])
    write(
        TABLES / "14-validation.md",
        table(
            ["Welding time model", "Fitted parameters", "KS distance to the data", "Parts per shift, 95 % CI",
             "Model − observed"],
            rows,
            f"Made-up data: {SAMPLES} measured welding times and {DAYS} observed shifts of a line whose true welding "
            f"time is lognormal (mean 45 s, sigma 0.6); models with 5 buffer places, {REPS} runs each",
        ),
    )
    # Histogram of the measurements with the two fitted densities.
    from svgkit import histogram

    lo, hi, bins = 0.0, 180.0, 36
    width = (hi - lo) / bins
    centres = [lo + (i + 0.5) * width for i in range(bins)]

    def counts(cdf) -> list[float]:
        return [SAMPLES * (cdf(c + width / 2) - cdf(max(c - width / 2, 1e-9))) for c in centres]

    write(
        IMG / "14-fit.svg",
        histogram(
            "f14a",
            "The lognormal follows the data; the exponential does not",
            f"Histogram of {SAMPLES} measured welding times, most between 20 and 80 seconds with a long tail; the fitted lognormal curve follows the bars, the exponential curve puts far too many times near zero.",
            observed,
            lo,
            hi,
            bins,
            "welding time (s)",
            curves=[("lognormal fit", counts(ln_cdf), "ref"), ("exponential fit", counts(exp_cdf), "ref2")],
            mean=(m, f"mean of the data and of both fits: {m:.1f} s"),
        ),
    )


if __name__ == "__main__":
    build()
