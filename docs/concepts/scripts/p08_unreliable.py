"""Page 8: failures and repairs at equal availability, short against long."""

import math

from common import CUT, DATA, IMG, SHIFT, SIGMA, TABLES, WELD, table, write, write_json
from linesim import Dist, Line, Station, estimate, replicate, simulate
from svgkit import Node, dominant, line_diagram, timeline

WEEK = 5 * SHIFT  # five shifts in a row; results are per 8-hour shift
REPS = 30
SCENARIOS = [("short", 3600.0, 180.0), ("long", 36000.0, 1800.0)]  # name, MTBF, MTTR in s
BUFFERS = [5, 20, 50]


def line(mtbf: float | None, mttr: float | None, buffer: int) -> Line:
    return Line(
        (Station("cut", Dist.lognormal(CUT, SIGMA), mtbf=mtbf, mttr=mttr), Station("weld", Dist.lognormal(WELD, SIGMA))),
        (buffer,),
    )


def effective_cv2(c0: float, mtbf: float, mttr: float, t0: float) -> float:
    """Hopp and Spearman's effective CV² of a station with exponential repairs."""
    a = mtbf / (mtbf + mttr)
    return c0 * c0 + 2 * a * (1 - a) * mttr / t0


def build() -> None:
    a = SCENARIOS[0][1] / (SCENARIOS[0][1] + SCENARIOS[0][2])
    write(
        IMG / "08-line.svg",
        line_diagram(
            "f08a",
            "Line A with failures at cutting",
            "Line A, variable, with failures at the cutting station and a buffer between cutting and welding.",
            [
                Node("source"),
                Node("station", "cut", f"42 s · cv 0.53\navailability {a:.1%}", fails=True),
                Node("buffer", name="buffer", capacity=20, sub="5 to 50 places"),
                Node("station", "weld", "45 s · cv 0.53"),
                Node("sink"),
            ],
        ),
    )
    c0 = Dist.lognormal(CUT, SIGMA).cv()
    rows, data = [], []
    base = estimate([r.completed / 5 for r in replicate(line(None, None, 20), WEEK, REPS)])
    for name, mtbf, mttr in SCENARIOS:
        ce = math.sqrt(effective_cv2(c0, mtbf, mttr, CUT))
        for b in BUFFERS:
            rs = replicate(line(mtbf, mttr, b), WEEK, REPS)
            e = estimate([r.completed / 5 for r in rs])
            down = sum(r.share(0, "down") for r in rs) / REPS
            rows.append([f"{mtbf / 3600:g} h / {mttr / 60:g} min", f"{ce:.2f}", b, f"{down:.1%}", e.fmt(1)])
            data.append({"mtbf": mtbf, "mttr": mttr, "buffer": b, "completed": e.mean, "completed_half": e.half})
    write(
        TABLES / "08-failures.md",
        table(
            ["Cut: MTBF / MTTR", "Cut effective CV", "Buffer places", "Cut down", "Parts per shift"],
            rows,
            f"Line A, variable (sigma 0.5); {REPS} runs of five 8-hour shifts each, results per shift; "
            f"without failures and with 20 places: {base.fmt(1)}",
        ),
    )
    write_json(DATA / "08-failures.json", {"horizon": WEEK, "reps": REPS, "sigma": SIGMA, "per": 5,
                                           "results": data})
    # A two-hour window with one long outage at cutting.
    window = 2 * 3600.0
    for seed in range(1, 200):
        r = simulate(line(36000.0, 1800.0, 20), window, seed, trace=True)
        downs = [i for i, (t, s) in enumerate(r.states[0]) if s == "down"]
        if downs:
            i = downs[0]
            t0 = r.states[0][i][0]
            t1 = r.states[0][i + 1][0] if i + 1 < len(r.states[0]) else window
            if 900 < t0 < 2400 and t1 - t0 > 1800:
                break
    step = window / 60
    starve = next((i * step for i in range(60) if i * step > t0 and dominant(r.states[1], i * step, (i + 1) * step) == "starved"), t0)
    write(
        IMG / "08-timeline.svg",
        timeline(
            "f08b",
            "A long outage empties the buffer, then welding stops too",
            "Two hours of line A with a 20-place buffer: cutting fails and is down for almost an hour; the few parts in the buffer last only minutes, after which welding is starved until cutting is repaired.",
            [("cut", r.states[0]), ("weld", r.states[1])],
            0.0,
            window,
            cells=60,
            levels=[("buffer", r.levels[0], 20)],
            mark=(starve, "buffer empty: weld starved"),
        ),
    )


if __name__ == "__main__":
    build()
