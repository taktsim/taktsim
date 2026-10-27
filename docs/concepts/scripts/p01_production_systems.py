"""Page 1: line A without randomness, hour by hour."""

from common import IMG, SHIFT, TABLES, line_a, table, write
from linesim import simulate
from svgkit import Node, line_diagram, matrix_chart, time_average


def build() -> None:
    write(
        IMG / "01-line.svg",
        line_diagram(
            "f01a",
            "Line A",
            "A source feeds the cutting station; a buffer of 20 places sits between cutting (42 s) and welding (45 s); welded parts leave to the sink.",
            [
                Node("source"),
                Node("station", "cut", "42 s"),
                Node("buffer", capacity=20, sub="20 places"),
                Node("station", "weld", "45 s"),
                Node("sink"),
            ],
        ),
    )
    r = simulate(line_a(), SHIFT, seed=1, trace=True)
    # Parts out and WIP at the end of each hour; lead time of the last part out.
    rows = []
    out = 0
    for h in range(1, 9):
        rh = simulate(line_a(), h * 3600.0, seed=1, trace=True)
        wip_end = rh.wip_trace[-1][1]
        rows.append([h, rh.completed, rh.completed - out, wip_end])
        out = rh.completed
    write(
        TABLES / "01-shift.md",
        table(
            ["Hour", "Parts out so far", "Parts out this hour", "WIP at end of hour"],
            rows,
            "One simulated shift of line A, no randomness",
        ),
    )
    wip = time_average(r.wip_trace, 0.0, SHIFT, 48)
    full = next(i for i, v in enumerate(wip) if round(v) >= 22)
    write(
        IMG / "01-wip.svg",
        matrix_chart(
            "f01b",
            "Work piles up in front of the slower station",
            "Work in progress of line A over an 8-hour shift, averaged over 10 minutes: it climbs from 1 to 22 parts in the first three and a half hours, then stays at 22 because the buffer is full.",
            wip,
            24,
            "hours into the shift",
            "parts in the line",
            [(i * 6, str(i)) for i in range(8)] + [(47, "8")],
            mark=(full, "buffer full: cut has to wait"),
        ),
    )

if __name__ == "__main__":
    build()
