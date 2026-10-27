"""Page 10: takt time, push against pull, and a CONWIP cap on line B."""

from common import DATA, IMG, SHIFT, SIGMA, TABLES, table, write, write_json
from linesim import Dist, Line, Station, estimate, replicate
from p09_longer_lines import MEANS, NAMES
from svgkit import Series, xy_chart

WEEK = 5 * SHIFT
WARMUP = 3600.0
REPS = 10
CAPS = [1, 2, 3, 5, 8, 10, 12, 15, 20, 30, 40]
TAKT = 48.0  # s: a demand of 600 parts per 8-hour shift
SPAN = WEEK - WARMUP


def line_b(conwip: int | None = None, arrivals: Dist | None = None) -> Line:
    """Line B without failures and with unlimited buffers; release by CONWIP or by arrivals."""
    stations = tuple(Station(n, Dist.lognormal(m, SIGMA)) for n, m in zip(NAMES, MEANS, strict=True))
    return Line(stations, (None,) * 4, arrivals=arrivals, conwip=conwip)


def build() -> None:
    t0 = sum(MEANS)  # raw process time: one part alone through an empty line
    rb = 1 / max(MEANS)  # bottleneck rate, parts per second
    w0 = rb * t0  # critical WIP
    per_shift = SHIFT / SPAN
    th, th_ci, rows, data = [], [], [], []
    for w in CAPS:
        rs = replicate(line_b(conwip=w), WEEK, REPS, WARMUP)
        e = estimate([r.completed * per_shift for r in rs])
        lt = estimate([r.lead_time / 60 for r in rs])
        pwc = w / (w0 + w - 1) * rb * SHIFT
        th.append(e.mean)
        th_ci.append(e.half)
        rows.append([w, e.fmt(1), lt.fmt(1), f"{min(w / t0, rb) * SHIFT:.0f}", f"{pwc:.0f}"])
        data.append({"cap": w, "completed": e.mean, "completed_half": e.half})
    write(
        TABLES / "10-conwip.md",
        table(
            ["WIP cap", "Parts per shift", "Lead time (min)", "Best case", "Practical worst case"],
            rows,
            f"Line B without failures, CONWIP release; {REPS} runs of five shifts, first hour discarded; "
            "best and practical worst case from Hopp and Spearman",
        ),
    )
    write_json(DATA / "10-conwip.json", {"horizon": WEEK, "warmup": WARMUP, "reps": REPS, "sigma": SIGMA,
                                         "per_shift": per_shift, "results": data})
    # Push at takt against pull with the smallest cap that meets demand.
    rows = []
    push = replicate(line_b(arrivals=Dist.exp(TAKT)), WEEK, REPS, WARMUP)
    cap = next(w for w, t in zip(CAPS, th, strict=True) if t >= SHIFT / TAKT)
    pull = replicate(line_b(conwip=cap), WEEK, REPS, WARMUP)
    for label, rs in [("Push: release a part every 48 s on average", push), (f"Pull: CONWIP, cap {cap}", pull)]:
        rows.append([label, estimate([r.completed * per_shift for r in rs]).fmt(1),
                     estimate([r.wip for r in rs]).fmt(1), estimate([r.lead_time / 60 for r in rs]).fmt(1)])
    write(
        TABLES / "10-push-pull.md",
        table(
            ["Release", "Parts per shift", "WIP (parts)", "Lead time (min)"],
            rows,
            f"Line B without failures, unlimited buffers; demand 600 parts per shift (takt 48 s); {REPS} runs of five shifts",
        ),
    )
    grid = [1 + i * 0.25 for i in range(157)]
    write(
        IMG / "10-conwip.svg",
        xy_chart(
            "f10a",
            "A small WIP cap already gives most of the output",
            "Parts per shift of line B against the CONWIP cap from 1 to 40, with the best case (dashed) and the practical worst case (dotted) of Hopp and Spearman; a thin line marks the demand of 600 parts per shift (takt 48 s), reached with a cap of 12.",
            [
                Series("simulated", CAPS, th, th_ci),
                Series("best case", grid, [min(w / t0, rb) * SHIFT for w in grid], style="ref"),
                Series("practical worst case", grid, [w / (w0 + w - 1) * rb * SHIFT for w in grid], style="ref2"),
                Series("demand", [0, 40], [SHIFT / TAKT] * 2, style="ref3"),
            ],
            "wip cap (parts)",
            "parts per shift",
            (0, 40),
            (0, 800),
            highlight=(cap, th[CAPS.index(cap)], f"cap {cap} meets demand"),
            label_below=True,
            height=340,
        ),
    )


if __name__ == "__main__":
    build()
