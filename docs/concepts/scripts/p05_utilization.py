"""Page 5: waiting time against utilization, simulated and by formula."""

from common import DATA, IMG, TABLES, WELD, table, write, write_json
from linesim import Dist, Line, Station, estimate, replicate
from svgkit import Series, xy_chart

HORIZON = 1_000_000.0
WARMUP = 50_000.0
REPS = 10
US = [0.5, 0.6, 0.7, 0.8, 0.85, 0.9, 0.95]
CASES = [  # (label, process-time CV, process-time distribution)
    ("CV 1 (exponential)", 1.0, Dist.exp(WELD)),
    ("CV 0.5", 0.5, Dist.lognormal_cv(WELD, 0.5)),
]


def single(u: float, d: Dist) -> Line:
    return Line((Station("weld", d),), (), arrivals=Dist.exp(WELD / u))


def kingman(u: float, ca: float, cs: float) -> float:
    """Kingman's approximation of the mean wait in queue, in seconds."""
    return (ca * ca + cs * cs) / 2 * u / (1 - u) * WELD


def build() -> None:
    rows = []
    series = []
    data = []
    for label, cs, d in CASES:
        ests = []
        for u in US:
            e = estimate([r.queue_wait / 60 for r in replicate(single(u, d), HORIZON, REPS, WARMUP)])
            ests.append(e)
            k = kingman(u, 1.0, cs) / 60
            exact = f"{k:.2f}" if cs == 1.0 else "–"
            rows.append([label, f"{u:.0%}", e.fmt(2), f"{k:.2f}", exact])
            data.append({"cv": cs, "u": u, "wait_min": e.mean, "half_min": e.half})
        grid = [0.3 + i * 0.005 for i in range(int((0.965 - 0.3) / 0.005) + 1)]
        series.append(Series(f"simulated, {label}", US, [e.mean for e in ests], [e.half for e in ests]))
        series.append(Series(f"Kingman, {label}" if cs == 1.0 else "", grid,
                             [kingman(u, 1.0, cs) / 60 for u in grid], style="ref"))
    write(
        TABLES / "05-waiting.md",
        table(
            ["Process time", "Utilization", "Wait in queue, simulated (min)", "Kingman (min)", "Exact M/M/1 (min)"],
            rows,
            f"One station, 45 s mean process time, random (exponential) arrivals; {REPS} runs of "
            f"{HORIZON / 86400:.1f} days, warm-up discarded; ± is the 95 % confidence interval",
        ),
    )
    write_json(DATA / "05-waiting.json", {"horizon": HORIZON, "warmup": WARMUP, "reps": REPS, "process_mean": WELD,
                                          "results": data})
    # Legend: one entry for the simulated points, one for the formula curves.
    series[0].label = "simulated"
    series[1].label = "Kingman"
    series[2].label = ""
    write(
        IMG / "05-hockey.svg",
        xy_chart(
            "f05a",
            "Close to 100 % busy, waiting explodes",
            "Mean wait in queue at one station with a 45-second process time, against its utilization from 50 to 95 percent, for process-time CV 1 (upper curve) and 0.5 (lower curve). Simulated points with confidence bands sit on Kingman's formula, which turns sharply upward above about 85 percent.",
            series,
            "utilization",
            "wait in queue (min)",
            (0.3, 1.0),
            (0, 16),
            highlight=(0.9, kingman(0.9, 1, 1) / 60,
                       f"CV 1, 90 % busy: {kingman(0.9, 1, 1) / 60:.1f} min = 9 × {WELD:g} s"),
            notes=[(0.79, kingman(0.79, 1, 1) / 60 + 0.8, "CV 1", "end"),
                   (0.81, kingman(0.81, 1, 0.5) / 60 - 1.0, "CV 0.5", "start")],
            xticks=[0.3, 0.4, 0.5, 0.6, 0.7, 0.8, 0.9, 1.0],
        ),
    )


if __name__ == "__main__":
    build()
