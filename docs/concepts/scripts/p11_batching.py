"""Page 11: setups and batch sizes at the welding station."""

from common import DATA, IMG, SIGMA, TABLES, WELD, table, write, write_json
from linesim import Dist, Line, Station, estimate, replicate
from svgkit import Series, xy_chart

HORIZON = 20 * 86400.0
WARMUP = 2 * 86400.0
REPS = 10
SETUP = 240.0  # s
ARRIVAL = 60.0  # s between orders on average
BATCHES = [17, 18, 20, 22, 25, 30, 35, 40, 50, 60, 80]


def station(b: int) -> Line:
    return Line((Station("weld", Dist.lognormal(WELD, SIGMA), setup=SETUP, batch=b),), (), arrivals=Dist.exp(ARRIVAL))


def batch_only(b: int) -> float:
    """Lead time without any queueing, in minutes: waiting for the batch to fill, setup, processing."""
    return ((b - 1) / 2 * ARRIVAL + SETUP + b * WELD) / 60


def build() -> None:
    b_min = SETUP / (ARRIVAL - WELD)
    rows, data, lt, ci = [], [], [], []
    for b in BATCHES:
        rs = replicate(station(b), HORIZON, REPS, WARMUP)
        e = estimate([r.lead_time / 60 for r in rs])
        setup = sum(r.share(0, "setup") for r in rs) / REPS
        u = (WELD + SETUP / b) / ARRIVAL
        lt.append(e.mean)
        ci.append(e.half)
        rows.append([b, f"{u:.0%}", f"{setup:.0%}", e.fmt(1), f"{batch_only(b):.1f}"])
        data.append({"batch": b, "lead_min": e.mean, "lead_half": e.half})
    write(
        TABLES / "11-batching.md",
        table(
            ["Batch size", "Utilization incl. setups", "Time in setup", "Lead time (min)", "Without queueing (min)"],
            rows,
            f"Welding alone: orders every {ARRIVAL:g} s on average (exponential), 45 s per part (sigma 0.5), "
            f"{SETUP / 60:g}-minute setup per batch; {REPS} runs of 20 days, first two discarded",
        ),
    )
    write_json(DATA / "11-batching.json", {"horizon": HORIZON, "warmup": WARMUP, "reps": REPS, "sigma": SIGMA,
                                           "setup": SETUP, "arrival": ARRIVAL, "results": data})
    best = min(range(len(BATCHES)), key=lambda i: lt[i])
    grid = list(range(17, 81))
    write(
        IMG / "11-batching.svg",
        xy_chart(
            "f11a",
            "Too small loses capacity, too large loses time",
            "Lead time at the welding station against batch size from 17 to 80: very high just above the minimum of 16, where setups use almost all spare capacity, lowest around 22, then rising steadily as parts wait for their batch to fill; the dashed line is the lead time without any queueing.",
            [
                Series("simulated", BATCHES, lt, ci),
                Series("without queueing", grid, [batch_only(b) for b in grid], style="ref"),
            ],
            "batch size (parts)",
            "lead time (min)",
            (10, 80),
            (0, 120),
            highlight=(BATCHES[best], lt[best], f"batch {BATCHES[best]}: shortest lead time"),
            label_below=True,
            vlines=[(b_min, "minimum: 16")],
        ),
    )


if __name__ == "__main__":
    build()
