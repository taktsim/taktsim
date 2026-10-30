"""Page 9: a five-station line; blocking and starving travel, the bottleneck moves."""

from common import IMG, SHIFT, SIGMA, TABLES, table, write
from linesim import Dist, Line, Station, estimate, replicate, simulate
from svgkit import Node, line_diagram, timeline

NAMES = ["cut", "deburr", "weld", "paint", "pack"]
MEANS = [42.0, 40.0, 45.0, 43.0, 38.0]
PAINT_FAILS = (7200.0, 900.0)  # MTBF 2 h, MTTR 15 min: availability 88.9 %
WEEK = 5 * SHIFT
REPS = 20


def line_b(buffers: list[int], paint: tuple[float, float] | None = PAINT_FAILS) -> Line:
    stations = []
    for name, mean in zip(NAMES, MEANS, strict=True):
        fails = {"mtbf": paint[0], "mttr": paint[1]} if name == "paint" and paint else {}
        stations.append(Station(name, Dist.lognormal(mean, SIGMA), **fails))
    return Line(tuple(stations), tuple(buffers))


def busy(rs: list, k: int) -> float:
    """Share of time neither starved nor blocked: working, down or in setup."""
    return sum(1 - r.share(k, "starved") - r.share(k, "blocked") for r in rs) / len(rs)


def build() -> None:
    a = PAINT_FAILS[0] / (PAINT_FAILS[0] + PAINT_FAILS[1])
    nodes = [Node("source")]
    for i, (name, mean) in enumerate(zip(NAMES, MEANS, strict=True)):
        # Paint shows why it is the bottleneck: its failures make it slower
        # on average than welding.
        sub = f"{mean:g} s\n{a:.1%} available\n= {mean / a:.1f} s" if name == "paint" else f"{mean:g} s"
        nodes.append(Node("station", name, sub, highlight=name == "paint", fails=name == "paint"))
        if i < len(NAMES) - 1:
            nodes.append(Node("buffer", capacity=5, name="5"))
    nodes.append(Node("sink"))
    write(
        IMG / "09-line.svg",
        line_diagram(
            "f09a",
            "Line B: five stations",
            f"Line B: cut 42 s, deburr 40 s, weld 45 s, paint 43 s with failures (available {a:.1%} of the time, so {MEANS[3] / a:.1f} s per part on average), pack 38 s, all variable, with five buffer places between each pair.",
            nodes,
        ),
    )
    # Who is the bottleneck? Share of time each station is not waiting.
    rows = []
    for label, paint in [("No failures", None), ("Paint fails: MTBF 2 h, MTTR 15 min", PAINT_FAILS)]:
        rs = replicate(line_b([5] * 4, paint), WEEK, REPS)
        shares = [busy(rs, k) for k in range(5)]
        top = max(range(5), key=lambda k: shares[k])
        rows.append([label] + [f"**{s:.0%}**" if k == top else f"{s:.0%}" for k, s in enumerate(shares)]
                    + [estimate([r.completed / 5 for r in rs]).fmt(1)])
    write(
        TABLES / "09-bottleneck.md",
        table(
            ["Line B, 5 places each"] + [n.capitalize() for n in NAMES] + ["Parts per shift"],
            rows,
            f"Share of time each station is neither starved nor blocked (working, or down); the busiest in bold. "
            f"{REPS} runs of five shifts",
        ),
    )
    # Ways to spend effort.
    options = [
        ("As is: 5 places each", [5] * 4, PAINT_FAILS),
        ("Double every buffer: 10 places each", [10] * 4, PAINT_FAILS),
        ("Four times: 20 places each", [20] * 4, PAINT_FAILS),
        ("The same 20 places around paint: 1, 1, 9, 9", [1, 1, 9, 9], PAINT_FAILS),
        ("Same availability, shorter outages: 40 min / 5 min", [5] * 4, (2400.0, 300.0)),
        ("Faster repairs at paint: MTTR 7.5 min", [5] * 4, (7200.0, 450.0)),
    ]
    rows = []
    for label, bufs, paint in options:
        rs = replicate(line_b(bufs, paint), WEEK, REPS)
        rows.append([label, sum(bufs), estimate([r.completed / 5 for r in rs]).fmt(1),
                     f"{sum(r.wip for r in rs) / REPS:.1f}", f"{sum(r.lead_time for r in rs) / REPS / 60:.1f}"])
    write(
        TABLES / "09-options.md",
        table(
            ["Change", "Buffer places in total", "Parts per shift", "WIP (parts)", "Lead time (min)"],
            rows,
            f"Line B with failures at paint unless stated; {REPS} runs of five 8-hour shifts, results per shift",
        ),
    )
    # A paint outage, seen along the whole line.
    window = 90 * 60.0
    for seed in range(1, 300):
        r = simulate(line_b([5] * 4), window, seed, trace=True)
        downs = [i for i, (_, s) in enumerate(r.states[3]) if s == "down"]
        if downs:
            i = downs[0]
            t0 = r.states[3][i][0]
            t1 = r.states[3][i + 1][0] if i + 1 < len(r.states[3]) else window
            if 900 < t0 < 1800 and t1 - t0 > 1500:
                break
    write(
        IMG / "09-timeline.svg",
        timeline(
            "f09b",
            "A stop at paint travels up and down the line",
            "Ninety minutes of line B: painting fails; within minutes welding is blocked, then deburring, then cutting, while packing is starved; after the repair the line restarts from the paint station outwards.",
            [(n, r.states[k]) for k, n in enumerate(NAMES)],
            0.0,
            window,
            cells=60,
            mark=(t0, "paint fails"),
        ),
    )


if __name__ == "__main__":
    build()
