"""Page 6: starving and blocking in the variable line A with a small buffer."""

from common import DATA, IMG, REPS, SHIFT, SIGMA, TABLES, line_a, table, write, write_json
from linesim import estimate, replicate, simulate
from svgkit import Node, dominant, line_diagram, timeline

WINDOW = 30 * 60.0


def build() -> None:
    write(
        IMG / "06-line.svg",
        line_diagram(
            "f06a",
            "Line A, variable, with one buffer place",
            "Line A with variable process times (CV 0.53) at both stations and a single buffer place between them.",
            [
                Node("source"),
                Node("station", "cut", "42 s · cv 0.53"),
                Node("buffer", capacity=1, sub="1 place"),
                Node("station", "weld", "45 s · cv 0.53"),
                Node("sink"),
            ],
        ),
    )
    r = simulate(line_a(sigma=SIGMA, buffer=1), WINDOW, seed=2, trace=True)
    # The first 30-second step that welding spends mostly starved, after the
    # start-up (when it waits for the very first part).
    step = WINDOW / 60
    starve = next(i * step for i in range(60) if i * step > 300 and dominant(r.states[1], i * step, (i + 1) * step) == "starved")
    write(
        IMG / "06-timeline.svg",
        timeline(
            "f06b",
            "Each station stops the other",
            "The first 30 minutes of the variable line A with one buffer place. Each row shows a station's state in 30-second steps: welding goes starved when a long cut leaves the buffer empty; cutting goes blocked when a long weld leaves the buffer full. The bottom chart shows the buffer level.",
            [("cut", r.states[0]), ("weld", r.states[1])],
            0.0,
            WINDOW,
            cells=60,
            levels=[("buffer", r.levels[0], 1)],
            mark=(starve, "weld starved: buffer empty"),
        ),
    )
    rows = []
    data = []
    for sigma in (0.0, 0.25, SIGMA):
        rs = replicate(line_a(sigma=sigma, buffer=1), SHIFT, REPS)
        e = {
            k: estimate([r.share(st, s) for r in rs])
            for k, (st, s) in {
                "cut_working": (0, "working"), "cut_blocked": (0, "blocked"),
                "weld_working": (1, "working"), "weld_starved": (1, "starved"),
            }.items()
        }
        out = estimate([float(r.completed) for r in rs])
        rows.append([f"{sigma:g}", f"{e['cut_working'].mean:.0%}", f"{e['cut_blocked'].mean:.0%}",
                     f"{e['weld_working'].mean:.0%}", f"{e['weld_starved'].mean:.0%}", out.fmt(1)])
        data.append({"sigma": sigma, "buffer": 1, "completed": out.mean, "completed_half": out.half,
                     **{k: v.mean for k, v in e.items()}})
    write(
        TABLES / "06-shares.md",
        table(
            ["Sigma (both stations)", "Cut working", "Cut blocked", "Weld working", "Weld starved", "Parts per shift"],
            rows,
            f"Line A, buffer 1; shares of an 8-hour shift, mean of {REPS} runs; ± is the 95 % confidence interval",
        ),
    )
    write_json(DATA / "06-shares.json", {"horizon": SHIFT, "reps": REPS, "results": data})


if __name__ == "__main__":
    build()
