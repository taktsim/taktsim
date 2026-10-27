"""Page 12: inspection with scrap and rework after welding."""

from common import CUT, IMG, SHIFT, SIGMA, TABLES, WELD, table, write
from linesim import Dist, Line, Station, estimate, replicate
from svgkit import Node, line_diagram, stacked_bars

WEEK = 5 * SHIFT
REPS = 20
INSPECT = 40.0  # s
CASES = [(0.0, 0.0), (0.02, 0.0), (0.0, 0.05), (0.02, 0.05), (0.05, 0.10)]  # scrap, rework


def line_c(scrap: float, rework: float) -> Line:
    """cut → 5 → weld → 5 → inspect; reworked parts go back to welding."""
    return Line(
        (
            Station("cut", Dist.lognormal(CUT, SIGMA)),
            Station("weld", Dist.lognormal(WELD, SIGMA)),
            Station("inspect", Dist.lognormal(INSPECT, 0.3), scrap=scrap, rework=rework, rework_to=1),
        ),
        (5, 5),
    )


def build() -> None:
    write(
        IMG / "12-line.svg",
        line_diagram(
            "f12a",
            "Line C: welding with an inspection after it",
            "Line C: cut, a buffer of 5, weld, a buffer of 5, and an inspection of 40 seconds; reworked parts go back to welding, scrapped parts leave the line.",
            [
                Node("source"),
                Node("station", "cut", "42 s"),
                Node("buffer", capacity=5, sub="5 places"),
                Node("station", "weld", "45 s", highlight=True),
                Node("buffer", capacity=5, sub="5 places"),
                Node("station", "inspect", "40 s"),
                Node("sink"),
            ],
            loops=[(5, 3, "rework")],
            exits=[(5, "scrap")],
        ),
    )
    rows, bars = [], []
    for scrap, rework in CASES:
        rs = replicate(line_c(scrap, rework), WEEK, REPS)
        good = estimate([r.completed / 5 for r in rs])
        scrapped = sum(r.scrapped for r in rs) / REPS / 5
        reworked = sum(r.reworked for r in rs) / REPS / 5
        weld_busy = sum(r.share(1, "working") for r in rs) / REPS
        bound = SHIFT / WELD * (1 - scrap - rework)
        label = f"scrap {scrap:.0%}, rework {rework:.0%}"
        rows.append([f"{scrap:.0%}", f"{rework:.0%}", good.fmt(1), f"{scrapped:.1f}", f"{reworked:.1f}",
                     f"{bound:.0f}", f"{weld_busy:.0%}"])
        passes = SHIFT / WELD  # what welding could do in a shift
        lost = max(0.0, passes - good.mean - scrapped - reworked)
        bars.append((label, [(good.mean, "good"), (reworked, "rework"), (scrapped, "scrap"), (lost, "lost")]))
    write(
        TABLES / "12-quality.md",
        table(
            ["Scrap", "Rework", "Good parts per shift", "Scrapped", "Reworked", "640 × (1 − scrap − rework)",
             "Weld working"],
            rows,
            f"Line C, variable (sigma 0.5 at cut and weld); {REPS} runs of five 8-hour shifts, results per shift",
        ),
    )
    write(
        IMG / "12-bars.svg",
        stacked_bars(
            "f12b",
            "Every scrapped or reworked part takes welding time from a good one",
            "For each case, welding's 640 possible operations per shift split into good parts, rework, scrapped parts and time lost to waiting; with 5 % scrap and 10 % rework only about 540 good parts remain.",
            bars,
            640,
            "welding operations per shift",
            [("good", "good"), ("rework", "rework"), ("scrap", "scrap"), ("waiting", "lost")],
            mark=3,
        ),
    )


if __name__ == "__main__":
    build()
