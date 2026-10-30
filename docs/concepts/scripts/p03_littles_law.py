"""Page 3: Little's Law in three different systems."""

from common import CUT, IMG, SIGMA, TABLES, WELD, line_a, table, write
from linesim import Dist, Line, Station, estimate, replicate, simulate
from svgkit import matrix_chart, time_average

HORIZON = 5 * 24 * 3600.0
WARMUP = 24 * 3600.0
REPS = 10

SYSTEMS = [
    ("Line A, no randomness, buffer 20", line_a()),
    ("Line A, variable (CV 0.53), buffer 5", line_a(sigma=SIGMA, buffer=5)),
    ("Weld alone, random arrivals every 54 s", Line((Station("weld", Dist.lognormal(WELD, SIGMA)),), (),
                                                    arrivals=Dist.exp(54.0))),
]


def build() -> None:
    rows = []
    for name, line in SYSTEMS:
        rs = replicate(line, HORIZON, REPS, WARMUP)
        wip = estimate([r.wip for r in rs])
        th = estimate([r.throughput for r in rs])
        lt = estimate([r.lead_time / 60 for r in rs])
        product = th.mean / 60 * lt.mean
        rows.append([name, wip.fmt(2), th.fmt(1), lt.fmt(2), f"{product:.2f}",
                     f"{(product - wip.mean) / wip.mean:+.1%}"])
    write(
        TABLES / "03-little.md",
        table(
            ["System", "WIP (parts)", "Throughput (parts/h)", "Lead time (min)", "Throughput × lead time",
             "Difference"],
            rows,
            f"{REPS} runs of 5 days each, first day discarded; ± is the 95 % confidence interval",
        ),
    )
    # One shift of the variable line: WIP over time.
    r = simulate(line_a(sigma=SIGMA, buffer=5), 3 * 3600.0, seed=3, trace=True)
    wip = time_average(r.wip_trace, 0.0, 3 * 3600.0, 36)
    mean = sum(wip) / len(wip)
    write(
        IMG / "03-wip.svg",
        matrix_chart(
            "f03a",
            "WIP moves; its average is what Little's Law is about",
            "Work in progress of the variable line A with a buffer of 5 over three hours, averaged over 5 minutes: it moves between 2 and 7 parts. A line marks its average over the three hours.",
            wip,
            8,
            "hours",
            "parts in the line",
            [(0, "0"), (12, "1"), (24, "2"), (35, "3")],
            average=(mean, f"average over the 3 hours: {mean:.1f} parts"),
        ),
    )
    _ = CUT


if __name__ == "__main__":
    build()
