"""Shared pieces of the concept-page scripts: the running example, output
paths and table formatting.

Every script writes its figures to docs/concepts/img/, its tables to
docs/concepts/tables/ (included into the pages, so no number is typed by
hand) and, for pages with an interactive figure, its results to
docs/concepts/data/ (checked by the in-browser simulator's tests).
"""

from __future__ import annotations

import json
from collections.abc import Sequence
from pathlib import Path

from linesim import Dist, Line, Station

HERE = Path(__file__).resolve().parent
CONCEPTS = HERE.parent
IMG = CONCEPTS / "img"
TABLES = CONCEPTS / "tables"
DATA = CONCEPTS / "data"

SHIFT = 8 * 3600.0  # s
CUT, WELD = 42.0, 45.0  # mean process times of the running example, s
SIGMA = 0.5  # lognormal shape of the variable line (CV about 0.53)
REPS = 30  # replications per shift-level result


def line_a(
    sigma: float = 0.0,
    buffer: int | None = 20,
    cut: float = CUT,
    weld: float = WELD,
    weld_sigma: float | None = None,
) -> Line:
    """Line A: source → cut → buffer → weld → sink.

    `sigma` is the lognormal shape of both stations unless `weld_sigma`
    sets the weld's own.
    """
    ws = sigma if weld_sigma is None else weld_sigma
    return Line(
        (Station("cut", Dist.lognormal(cut, sigma)), Station("weld", Dist.lognormal(weld, ws))),
        (buffer,),
    )


def low_variability_line(buffer: int | None = 20) -> Line:
    """Line A with little variability: cut lognormal(42 s, 0.08), constant 45 s weld."""
    return line_a(sigma=0.08, buffer=buffer, weld_sigma=0.0)


def write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8", newline="\n")


def write_json(path: Path, obj: object) -> None:
    write(path, json.dumps(obj, indent=1, sort_keys=True) + "\n")


def table(header: Sequence[str], rows: Sequence[Sequence[object]], note: str = "") -> str:
    """A Markdown table; `note` is a line under it (method, replications)."""
    out = ["| " + " | ".join(header) + " |", "| " + " | ".join("---" for _ in header) + " |"]
    out += ["| " + " | ".join(str(c) for c in r) + " |" for r in rows]
    if note:
        out += ["", f'<p class="ts-label">{note}</p>']
    return "\n".join(out) + "\n"
