"""Page 13: replications, confidence intervals and warm-up."""

from common import CUT, DATA, IMG, SHIFT, SIGMA, TABLES, WELD, line_a, table, write, write_json
from linesim import Dist, Line, Station, estimate, replicate
from svgkit import Series, histogram, time_average, xy_chart

N = 100


def line_8() -> Line:
    """Page 8's line: short failures at cutting, 20 buffer places."""
    return Line(
        (Station("cut", Dist.lognormal(CUT, SIGMA), mtbf=3600.0, mttr=180.0), Station("weld", Dist.lognormal(WELD, SIGMA))),
        (20,),
    )


def mser5(series: list[float]) -> int:
    """MSER-5: the number of leading observations to discard (spec §6, ADR-0014)."""
    batches = [sum(series[i:i + 5]) / 5 for i in range(0, len(series) - 4, 5)]
    m = len(batches)
    best, best_d = float("inf"), 0
    for d in range(m // 2 + 1):
        rest = batches[d:]
        mean = sum(rest) / len(rest)
        stat = sum((z - mean) ** 2 for z in rest) / len(rest) ** 2
        if stat < best:
            best, best_d = stat, d
    return best_d * 5


def build() -> None:
    rs = replicate(line_8(), SHIFT, N)
    values = [float(r.completed) for r in rs]
    rows = []
    for n in (3, 5, 10, 30, 100):
        e = estimate(values[:n])
        rows.append([n, e.fmt(1), f"± {e.half / e.mean:.1%}", f"{min(values[:n]):.0f} – {max(values[:n]):.0f}"])
    write(
        TABLES / "13-replications.md",
        table(
            ["Replications", "Parts per shift, 95 % CI", "Relative half-width", "Lowest – highest run"],
            rows,
            "Page 8's line (failures at cut, MTBF 1 h, MTTR 3 min; 20 buffer places), one 8-hour shift per run, seeds 1 to n",
        ),
    )
    write_json(DATA / "13-replications.json", {"horizon": SHIFT, "values": values})
    e = estimate(values)
    write(
        IMG / "13-spread.svg",
        histogram(
            "f13a",
            "One run is one draw; the confidence interval is about the mean",
            f"Parts per shift in 100 runs of the same line with different seeds, spread from {min(values):.0f} to {max(values):.0f}; the bracket above is the 95 % confidence interval of the mean, {e.mean - e.half:.0f} to {e.mean + e.half:.0f}.",
            values,
            560,
            680,
            40,
            "parts per shift",
            band=(e.mean - e.half, e.mean + e.half, "95 % ci of the mean"),
            mean=(e.mean, f"mean {e.mean:.0f} ± {e.half:.1f}"),
        ),
    )
    # Warm-up: the variable line A with 20 places starts empty.
    horizon = 2 * 86400.0
    slices = int(horizon / 600)
    runs = [__import__("linesim").simulate(line_a(sigma=SIGMA, buffer=20), horizon, seed, trace=True) for seed in range(1, 21)]
    series = [time_average(r.wip_trace, 0.0, horizon, slices) for r in runs]
    cuts = sorted(mser5(s) for s in series)
    median = cuts[len(cuts) // 2] * 10 / 60  # hours
    show = 48  # first 8 hours, 10-minute slices
    mean_wip = [sum(s[i] for s in series) / len(series) for i in range(show)]
    hours = [(i + 0.5) / 6 for i in range(show)]
    steady = sum(sum(s[int(median * 6) + 6:]) / len(s[int(median * 6) + 6:]) for s in series) / len(series)
    write(
        TABLES / "13-warmup.md",
        table(
            ["Runs", "Warm-up found by MSER-5 (h): lowest, median, highest", "Average WIP after warm-up"],
            [[len(runs), f"{cuts[0] / 6:.1f}, {median:.1f}, {cuts[-1] / 6:.1f}", f"{steady:.1f}"]],
            "Variable line A, 20 buffer places, starting empty; each run 2 days, WIP averaged over 10 minutes",
        ),
    )
    write(
        IMG / "13-warmup.svg",
        xy_chart(
            "f13b",
            "An empty line is not a typical line: discard the warm-up",
            f"Average work in progress over the first 8 hours of 20 runs that start empty: it climbs for about four hours before it settles around {steady:.0f} parts. MSER-5, applied to each run, ends the warm-up at {median:.1f} hours in the median run, earlier than the eye would.",
            [Series("mean of 20 runs", hours, mean_wip, markers=False)],
            "hours since start",
            "parts in the line",
            (0, 8),
            (0, 25),
            highlight=(median, mean_wip[min(show - 1, int(median * 6))], f"MSER-5, median run: {median:.1f} h"),
            label_below=True,
            vlines=[(4.0, "settled by eye: about 4 h")],
            xticks=[0, 1, 2, 3, 4, 5, 6, 7, 8],
        ),
    )


if __name__ == "__main__":
    build()
