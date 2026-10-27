"""Page 4: process times with low and high variability."""

import math

from common import CUT, IMG, TABLES, WELD, table, write
from linesim import Dist, Stream
from svgkit import histograms

N = 4000
CASES = [
    ("Lognormal, sigma 0.08", Dist.lognormal(CUT, 0.08)),
    ("Lognormal, sigma 0.5", Dist.lognormal(CUT, 0.5)),
    ("Exponential", Dist.exp(CUT)),
]


def sample(d: Dist, seed: int) -> list[float]:
    rng = Stream(seed, 1)
    return [d.sample(rng) for _ in range(N)]


def build() -> None:
    rows = []
    data = []
    for i, (name, d) in enumerate(CASES):
        xs = sample(d, 100 + i)
        data.append(xs)
        m = sum(xs) / N
        sd = math.sqrt(sum((x - m) ** 2 for x in xs) / (N - 1))
        rows.append([name, f"{d.cv():.2f}", f"{m:.1f} s", f"{sd:.1f} s", f"{sd / m:.2f}",
                     f"{sum(x > WELD for x in xs) / N:.0%}"])
    write(
        TABLES / "04-cv.md",
        table(
            ["Cut time distribution, mean 42 s", "CV (theory)", "Sample mean", "Sample sd", "Sample CV",
             "Slower than 45 s"],
            rows,
            f"{N} samples each",
        ),
    )
    write(
        IMG / "04-histograms.svg",
        histograms(
            "f04a",
            "Same mean, very different spread",
            "Histograms of 4000 cutting times with a mean of 42 seconds: with sigma 0.08 they all fall between about 33 and 54 seconds; with sigma 0.5 they spread from under 15 to over 120 seconds.",
            [("cv 0.08", data[0]), ("cv 0.53", data[1])],
            0,
            150,
            50,
            "cut time (s)",
            mark=(1, WELD, "45 s: weld"),
        ),
    )


if __name__ == "__main__":
    build()
