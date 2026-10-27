"""Page 2: the slowest station sets the pace (no randomness)."""

from common import IMG, SHIFT, TABLES, WELD, line_a, table, write
from linesim import simulate
from svgkit import Node, Series, line_diagram, xy_chart

CUTS = [30, 35, 40, 42, 45, 48, 50, 55, 60]


def build() -> None:
    write(
        IMG / "02-line.svg",
        line_diagram(
            "f02a",
            "Line A with its bottleneck",
            "Line A: cut at 42 s, a buffer of 20, weld at 45 s. Welding is the slowest station, so it is the bottleneck.",
            [
                Node("source"),
                Node("station", "cut", "42 s · 86/h"),
                Node("buffer", capacity=20, sub="20 places"),
                Node("station", "weld", "45 s · 80/h", highlight=True),
                Node("sink"),
            ],
        ),
    )
    rows, sim, formula = [], [], []
    for c in CUTS:
        r = simulate(line_a(cut=float(c)), SHIFT, seed=1)
        bound = SHIFT / max(c, WELD)
        sim.append(r.completed)
        formula.append(bound)
        rows.append([f"{c} s", f"{3600 / c:.1f}", r.completed, f"{bound:.0f}",
                     f"{r.share(0, 'working'):.0%}", f"{r.share(1, 'working'):.0%}"])
    write(
        TABLES / "02-sweep.md",
        table(
            ["Cut time", "Cut capacity per hour", "Parts per shift (simulated)", "8 h ÷ slowest time",
             "Cut busy", "Weld busy"],
            rows,
            "Line A without randomness, weld at 45 s, buffer 20, one 8-hour shift each",
        ),
    )
    write(
        IMG / "02-sweep.svg",
        xy_chart(
            "f02b",
            "Output follows the slowest station",
            "Parts per shift as the cutting time goes from 30 to 60 seconds with welding fixed at 45 seconds: flat at 640 while cutting is faster, then falling as 28,800 divided by the cutting time.",
            [
                Series("simulated", CUTS, sim),
                Series("8 h ÷ slowest time", CUTS, formula, style="ref"),
            ],
            "cut time (s)",
            "parts per shift",
            (30, 60),
            (400, 700),
            highlight=(45, sim[CUTS.index(45)], "cut = weld: both are bottlenecks"),
        ),
    )


if __name__ == "__main__":
    build()
