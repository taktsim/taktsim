"""Page 7: throughput against buffer size; the README example and a low-variability line."""

from common import DATA, IMG, REPS, SHIFT, SIGMA, TABLES, line_a, low_variability_line, table, write, write_json
from linesim import estimate, replicate
from svgkit import Series, xy_chart

SIZES = [0, 1, 2, 3, 5, 8, 10, 15, 20, 30]


def build() -> None:
    rows, data = [], []
    var, var_ci, low = [], [], []
    for b in SIZES:
        rs = replicate(line_a(sigma=SIGMA, buffer=b), SHIFT, REPS)
        out = estimate([float(r.completed) for r in rs])
        wip = estimate([r.wip for r in rs])
        lt = estimate([r.lead_time / 60 for r in rs])
        var.append(out.mean)
        var_ci.append(out.half)
        rr = replicate(low_variability_line(b), SHIFT, REPS)
        ro = estimate([float(r.completed) for r in rr])
        low.append(ro.mean)
        rows.append([b, out.fmt(1), wip.fmt(1), lt.fmt(1), ro.fmt(1)])
        data.append({"buffer": b, "completed": out.mean, "completed_half": out.half,
                     "low_completed": ro.mean, "low_half": ro.half})
    write(
        TABLES / "07-buffers.md",
        table(
            ["Buffer places", "Parts per shift (CV 0.53)", "WIP (parts)", "Lead time (min)",
             "Parts per shift, low variability (cut CV 0.08, weld fixed)"],
            rows,
            f"Line A, 8-hour shift, mean of {REPS} runs; ± is the 95 % confidence interval",
        ),
    )
    write_json(DATA / "07-buffers.json", {"horizon": SHIFT, "reps": REPS, "sigma": SIGMA, "results": data})
    write(
        IMG / "07-buffers.svg",
        xy_chart(
            "f07a",
            "More buffer helps, but less each time",
            "Parts per 8-hour shift against buffer size from 0 to 30 places. With variable process times (CV 0.53) output rises steeply over the first few places and then flattens below the weld's capacity of 640; a line with little variability (cutting CV 0.08, welding fixed) stays near 639 whatever the buffer.",
            [
                Series("cv 0.53", SIZES, var, var_ci),
                Series("low variability", SIZES, low, style="ref"),
            ],
            "buffer places",
            "parts per shift",
            (0, 30),
            (500, 660),
            highlight=(5, var[SIZES.index(5)], "5 places: most of the gain"),
            label_below=True,
        ),
    )


if __name__ == "__main__":
    build()
